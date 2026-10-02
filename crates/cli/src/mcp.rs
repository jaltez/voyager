//! `vygr serve`: MCP server over stdio (ADR-0011, M3.1).
//!
//! Hand-rolled JSON-RPC 2.0 with newline-delimited messages: the protocol
//! surface we need (initialize / ping / tools/list / tools/call) is tiny
//! and avoids a heavy SDK dependency. Token safety follows Librarium's
//! pattern: `research` returns the answer plus the run directory, and
//! `get_artifact` pages any run artifact with a hard size cap instead of
//! dumping whole files into the agent's context.

use std::path::{Component, PathBuf};
use std::sync::Arc;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use vygr_core::provider::FetchProvider;
use vygr_core::{Config, VygrError};

/// Hard cap for `get_artifact` pages.
const PAGE_MAX: usize = 32_000;
const PAGE_DEFAULT: usize = 8_000;

/// Sink for server-initiated notifications (written to stdout as they
/// happen, interleaved with request replies).
pub type Notify = Arc<dyn Fn(Value) + Send + Sync>;

pub struct McpServer {
    pub config: Config,
    pub http: reqwest::Client,
}

pub async fn serve(http: reqwest::Client) -> Result<(), VygrError> {
    let server = McpServer {
        config: Config::load(),
        http,
    };
    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin);
    let stdout = Arc::new(tokio::sync::Mutex::new(tokio::io::stdout()));
    let notify: Notify = {
        let stdout = Arc::clone(&stdout);
        Arc::new(move |v: Value| {
            let stdout = stdout.clone();
            // Tiny line writes; a short blocking task is fine here.
            tokio::spawn(async move {
                let mut out = stdout.lock().await;
                let _ = out.write_all(format!("{v}\n").as_bytes()).await;
                let _ = out.flush().await;
            });
        })
    };
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            return Ok(()); // client closed the pipe
        }
        let reply = match serde_json::from_str::<Value>(line.trim()) {
            Ok(msg) => handle_message(&server, &msg, &notify).await,
            Err(e) => Some(json!({
                "jsonrpc": "2.0",
                "id": Value::Null,
                "error": { "code": -32700, "message": format!("parse error: {e}") }
            })),
        };
        if let Some(reply) = reply {
            let mut out = stdout.lock().await;
            out.write_all(format!("{reply}\n").as_bytes())
                .await
                .map_err(VygrError::Io)?;
            out.flush().await.map_err(VygrError::Io)?;
        }
    }
}

/// Handle one JSON-RPC message. Returns `None` for notifications (no id).
pub async fn handle_message(server: &McpServer, msg: &Value, notify: &Notify) -> Option<Value> {
    let id = msg.get("id")?.clone();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": msg
                .pointer("/params/protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or("2024-11-05"),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "vygr", "version": env!("CARGO_PKG_VERSION") }
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_definitions() })),
        "tools/call" => call_tool(server, msg, notify).await,
        _ => Err((-32601_i64, format!("method not found: {method}"))),
    };
    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => {
            json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
        }
    })
}

fn tool_definitions() -> Vec<Value> {
    fn schema(props: Value, required: &[&str]) -> Value {
        let mut s = json!({ "type": "object", "properties": props });
        if !required.is_empty() {
            s["required"] = json!(required);
        }
        s
    }
    vec![
        json!({
            "name": "search",
            "description": "Web search through a provider chain (ddgs keyless, brave, tavily, searxng, exa, serper, jina, kagi). Returns normalized JSON results.",
            "inputSchema": schema(json!({
                "query": { "type": "string" },
                "max_results": { "type": "integer", "minimum": 1, "maximum": 20 },
                "provider": { "type": "string", "description": "provider or comma-separated fallback chain; defaults to config" }
            }), &["query"])
        }),
        json!({
            "name": "extract",
            "description": "Fetch URLs and reduce them to plain text (boilerplate removed).",
            "inputSchema": schema(json!({
                "urls": { "type": "array", "items": { "type": "string" } },
                "max_chars": { "type": "integer" }
            }), &["urls"])
        }),
        json!({
            "name": "research",
            "description": "Full deep-research run: plan, iterative search levels with reflections, synthesis. Returns the answer and the run directory (use get_artifact to page sources.json / plan.json). Requires an LLM backend configured (--llm equivalent via arguments.llm or the server config).",
            "inputSchema": schema(json!({
                "query": { "type": "string" },
                "depth": { "type": "string", "description": "fixed '3' or range '2..4'" },
                "breadth": { "type": "integer", "minimum": 1, "maximum": 10 },
                "budget_usd": { "type": "number" },
                "llm": { "type": "string", "description": "LLM spec: pi | claude | codex | ollama:<model> | <provider>:<model>" }
            }), &["query"])
        }),
        json!({
            "name": "get_artifact",
            "description": "Page a run artifact (answer.md, sources.json, plan.json, reflections.json) relative to the working directory. Token-safe: results are capped and cursorless-paged by offset.",
            "inputSchema": schema(json!({
                "path": { "type": "string" },
                "offset": { "type": "integer", "minimum": 0 },
                "length": { "type": "integer", "maximum": 32000 }
            }), &["path"])
        }),
    ]
}

async fn call_tool(
    server: &McpServer,
    msg: &Value,
    notify: &Notify,
) -> Result<Value, (i64, String)> {
    let name = msg
        .pointer("/params/name")
        .and_then(Value::as_str)
        .unwrap_or("");
    let args = msg
        .pointer("/params/arguments")
        .cloned()
        .unwrap_or(json!({}));
    let arg = |key: &str| args.get(key);

    let text = match name {
        "search" => {
            let query = arg("query")
                .and_then(Value::as_str)
                .ok_or((-32602_i64, "search requires a 'query' string".to_string()))?;
            let max_results = arg("max_results")
                .and_then(Value::as_u64)
                .unwrap_or(5)
                .clamp(1, 20) as usize;
            let provider = arg("provider")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| server.config.default_provider.clone())
                .unwrap_or_else(|| "ddgs".to_string());
            let stack = vygr_core::config::SearchStackConf::from_config(&server.config);
            let handle = vygr_providers::build_chain(
                &provider,
                server.http.clone(),
                &stack,
                &vygr_providers::CacheOptions::default(),
            )
            .map_err(|e| (-32602_i64, e.to_string()))?;
            let (results, warnings) = vygr_providers::search_chain(
                &handle.providers,
                &vygr_core::provider::SearchQuery::new(query, max_results),
            )
            .await;
            let (hits, misses) = handle.cache.snapshot();
            json!({
                "query": query,
                "provider": provider,
                "warnings": warnings,
                "cache": { "hits": hits, "misses": misses },
                "results": results,
            })
            .to_string()
        }
        "extract" => {
            let urls: Vec<String> = arg("urls")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .ok_or((-32602_i64, "extract requires a 'urls' array".to_string()))?;
            let max_chars = arg("max_chars")
                .and_then(Value::as_u64)
                .unwrap_or(8_000)
                .clamp(200, 100_000) as usize;
            let fetcher = vygr_providers::EscalatingFetch::new(
                vygr_providers::HttpFetch::new(server.http.clone()),
                server.http.clone(),
            );
            let mut pages: Vec<Value> = Vec::new();
            for url in urls {
                match fetcher.fetch(&url, max_chars).await {
                    Ok(p) => match serde_json::to_value(&p) {
                        Ok(v) => pages.push(v),
                        Err(e) => pages.push(json!({ "url": url, "error": e.to_string() })),
                    },
                    Err(e) => pages.push(json!({ "url": url, "error": e.to_string() })),
                }
            }
            serde_json::to_string(&pages).unwrap_or_default()
        }
        "research" => {
            let query = arg("query")
                .and_then(Value::as_str)
                .ok_or((-32602_i64, "research requires a 'query' string".to_string()))?;
            let llm = vygr_llm::resolve(
                arg("llm").and_then(Value::as_str),
                &server.config.llm,
                &server.http,
            )
            .await
            .map_err(|e| (-32602_i64, e.to_string()))?;
            let depth = match arg("depth").and_then(Value::as_str) {
                Some(d) => {
                    vygr_research::DepthSpec::parse(d).map_err(|e| (-32602_i64, e.to_string()))?
                }
                None => vygr_research::DepthSpec {
                    min: server.config.research.depth_min,
                    max: server.config.research.depth_max,
                },
            };
            let request = vygr_research::ResearchRequest {
                query: query.to_string(),
                depth,
                breadth: arg("breadth")
                    .and_then(Value::as_u64)
                    .unwrap_or(server.config.research.breadth as u64)
                    .clamp(1, 10) as u32,
                budget_usd: arg("budget_usd")
                    .and_then(Value::as_f64)
                    .or(server.config.research.budget_usd),
                max_results_per_query: server.config.research.max_results_per_query,
                fetch_top: server.config.research.fetch_top,
                context_max_chars: server.config.research.context_max_chars,
                provider_spec: server
                    .config
                    .default_provider
                    .clone()
                    .unwrap_or_else(|| "ddgs".to_string()),
                run_dir_base: server.config.research.run_dir.clone(),
                stack: vygr_core::config::SearchStackConf::from_config(&server.config),
                time_range: None,
                include_domains: vec![],
                exclude_domains: vec![],
                language: None,
                output_schema: None,
            };
            // Progress as notifications/progress: pi's MCP client kills
            // requests after 60s unless progress notifications arrive.
            let sink: vygr_research::ProgressSink = match progress_token(msg) {
                Some(token) => {
                    let notify = Arc::clone(notify);
                    Arc::new(move |message: &str| {
                        notify(progress_notification(&token, message));
                    })
                }
                None => Arc::new(|_| {}),
            };
            match vygr_research::run(request, llm, server.http.clone(), Some(sink)).await {
                Ok(report) => {
                    let mut text = String::new();
                    if let Some(answer) = &report.answer {
                        text.push_str(answer);
                        text.push_str("\n\n---\n");
                    }
                    text.push_str(&format!(
                        "run directory: {}\nsources: {} | iterations: {} | cost: {}",
                        report.run_dir.as_deref().unwrap_or("-"),
                        report.sources.len(),
                        report.iterations,
                        report
                            .cost_usd
                            .map(|c| format!("${c:.4}"))
                            .unwrap_or_else(|| "unknown (free or untracked)".into()),
                    ));
                    for w in &report.warnings {
                        text.push_str(&format!("\nwarning: {w}"));
                    }
                    text
                }
                Err(e) => return Ok(tool_error(e.to_string())),
            }
        }
        "get_artifact" => {
            let path = arg("path").and_then(Value::as_str).ok_or((
                -32602_i64,
                "get_artifact requires a 'path' string".to_string(),
            ))?;
            let resolved = safe_path(path).map_err(|e| (-32602_i64, e))?;
            let content = std::fs::read_to_string(&resolved)
                .map_err(|e| (-32602_i64, format!("reading {path}: {e}")))?;
            let total = content.len();
            let offset = arg("offset")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                .min(total as u64) as usize;
            let length = arg("length")
                .and_then(Value::as_u64)
                .unwrap_or(PAGE_DEFAULT as u64)
                .min(PAGE_MAX as u64) as usize;
            let end = (offset + length).min(total);
            // Slice on a char boundary; ASCII byte offsets are fine for
            // paging because callers page by returned lengths.
            let page: String = content
                .get(offset..end)
                .unwrap_or_default()
                .chars()
                .collect();
            json!({
                "path": path,
                "total_bytes": total,
                "offset": offset,
                "page": page,
                "has_more": end < total,
            })
            .to_string()
        }
        other => {
            return Err((
                -32602_i64,
                format!(
                    "unknown tool '{other}' (available: search, extract, research, get_artifact)"
                ),
            ))
        }
    };
    Ok(json!({ "content": [{ "type": "text", "text": text }], "isError": false }))
}

/// The `progressToken` a client attached to a `tools/call` request
/// (number or string), used to address progress notifications back.
fn progress_token(msg: &Value) -> Option<Value> {
    let token = msg.pointer("/params/_meta/progressToken")?;
    if token.is_null() {
        return None;
    }
    Some(token.clone())
}

/// A `notifications/progress` JSON-RPC notification.
fn progress_notification(token: &Value, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "notifications/progress",
        "params": {
            "progressToken": token,
            "message": message,
        }
    })
}

fn tool_error(message: String) -> Value {
    json!({ "content": [{ "type": "text", "text": message }], "isError": true })
}

/// Resolve a relative artifact path under the current directory, refusing
/// absolute paths and traversal; the MCP surface is read-only but should
/// still not expose the whole filesystem.
fn safe_path(path: &str) -> Result<PathBuf, String> {
    let p = PathBuf::from(path);
    if p.is_absolute() {
        return Err("path must be relative to the working directory".to_string());
    }
    for component in p.components() {
        if matches!(component, Component::ParentDir) {
            return Err("path must not contain '..'".to_string());
        }
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noop_notify() -> Notify {
        Arc::new(|_| {})
    }

    #[test]
    fn progress_token_extraction() {
        let with_number = json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": "research", "_meta": { "progressToken": 42 } }
        });
        assert_eq!(progress_token(&with_number), Some(json!(42)));
        let with_string = json!({
            "params": { "_meta": { "progressToken": "run-7" } }
        });
        assert_eq!(progress_token(&with_string), Some(json!("run-7")));
        let without = json!({"params": {"name": "search"}});
        assert_eq!(progress_token(&without), None);
        let null_token = json!({"params": {"_meta": {"progressToken": null}}});
        assert_eq!(progress_token(&null_token), None);
    }

    #[test]
    fn progress_notification_shape() {
        let n = progress_notification(&json!(42), "level 1/2: 5 new sources");
        assert_eq!(n["jsonrpc"], "2.0");
        assert_eq!(n["method"], "notifications/progress");
        assert_eq!(n["params"]["progressToken"], 42);
        assert_eq!(n["params"]["message"], "level 1/2: 5 new sources");
        assert!(n.get("id").is_none());
    }

    fn server() -> McpServer {
        McpServer {
            config: Config::default(),
            http: reqwest::Client::new(),
        }
    }

    #[tokio::test]
    async fn initialize_handshake_and_ping() {
        let s = server();
        let init = handle_message(
            &s,
            &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}),
            &noop_notify(),
        )
        .await
        .unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(init["result"]["serverInfo"]["name"], "vygr");

        let ping = handle_message(
            &s,
            &json!({"jsonrpc":"2.0","id":2,"method":"ping"}),
            &noop_notify(),
        )
        .await
        .unwrap();
        assert_eq!(ping["result"], json!({}));
    }

    #[tokio::test]
    async fn notifications_and_unknown_methods() {
        let s = server();
        // No id => notification => no reply.
        assert!(handle_message(
            &s,
            &json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            &noop_notify()
        )
        .await
        .is_none());
        let err = handle_message(
            &s,
            &json!({"jsonrpc":"2.0","id":3,"method":"bogus"}),
            &noop_notify(),
        )
        .await
        .unwrap();
        assert_eq!(err["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn tools_list_exposes_four_tools() {
        let s = server();
        let reply = handle_message(
            &s,
            &json!({"jsonrpc":"2.0","id":4,"method":"tools/list"}),
            &noop_notify(),
        )
        .await
        .unwrap();
        let names: Vec<&str> = reply["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        assert_eq!(names, vec!["search", "extract", "research", "get_artifact"]);
    }

    #[tokio::test]
    async fn get_artifact_pages_safely() {
        let s = server();
        let tmp = tempfile::TempDir::new().unwrap();
        let rel = "agents/test/answer.md";
        let path = tmp.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "x".repeat(20_000)).unwrap();

        // Tests run with cwd = crate dir; chdir into the temp dir root for
        // the sandbox check.
        let old = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();
        let call = |args: Value| {
            json!({"jsonrpc":"2.0","id":5,"method":"tools/call",
                   "params":{"name":"get_artifact","arguments":args}})
        };
        let first = handle_message(&s, &call(json!({"path": rel})), &noop_notify())
            .await
            .unwrap();
        let page = first["result"]["content"][0]["text"].as_str().unwrap();
        let parsed: Value = serde_json::from_str(page).unwrap();
        assert_eq!(parsed["total_bytes"], 20_000);
        assert_eq!(parsed["page"].as_str().unwrap().len(), PAGE_DEFAULT);
        assert_eq!(parsed["has_more"], true);

        // Traversal refused.
        let bad = handle_message(
            &s,
            &call(json!({"path": "../../etc/passwd"})),
            &noop_notify(),
        )
        .await
        .unwrap();
        assert!(bad["result"]["isError"].as_bool().unwrap_or(false) || bad["error"].is_object());
        std::env::set_current_dir(old).unwrap();
    }

    #[tokio::test]
    async fn unknown_tool_is_a_jsonrpc_error() {
        let s = server();
        let reply = handle_message(
            &s,
            &json!({"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"nope"}}),
            &noop_notify(),
        )
        .await
        .unwrap();
        assert_eq!(reply["error"]["code"], -32602);
    }
}
