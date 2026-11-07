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

/// `vygr serve`: stdio by default, Streamable HTTP with `--http`. The
/// token (`--token`, else `VGR_MCP_TOKEN`) protects the HTTP transport
/// only — stdio inherits the harness's own isolation.
pub async fn run(
    http: reqwest::Client,
    http_addr: Option<String>,
    token: Option<String>,
) -> Result<(), VygrError> {
    let token = token.or_else(|| std::env::var("VGR_MCP_TOKEN").ok());
    match http_addr {
        None => {
            if token.is_some() {
                eprintln!("vygr: warning: --token / VGR_MCP_TOKEN applies to --http mode only");
            }
            serve(http).await
        }
        Some(addr) => crate::mcp_http::serve_http(http, &addr, token).await,
    }
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
            "capabilities": { "tools": {}, "resources": {}, "prompts": {} },
            "serverInfo": { "name": "vygr", "version": env!("CARGO_PKG_VERSION") }
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_definitions() })),
        "tools/call" => call_tool(server, msg, notify).await,
        "resources/list" => list_resources(server),
        "resources/read" => read_resource(server, msg),
        "prompts/list" => Ok(json!({ "prompts": prompt_definitions() })),
        "prompts/get" => get_prompt(msg),
        _ => Err((-32601_i64, format!("method not found: {method}"))),
    };
    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => {
            json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
        }
    })
}

/// Output schemas for the tools whose responses are pure JSON: richer
/// clients can consume the payload as `structuredContent`. `research`
/// (markdown answer) and `get_artifact` (paged text) stay text-only.
/// MCP requires `structuredContent` to be a JSON object, so array-shaped
/// results ride in a wrapper key (`extract` -> `{"pages": [...]}`).
fn tool_output_schema(name: &str) -> Option<Value> {
    match name {
        "search" => Some(json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" },
                "provider": { "type": "string" },
                "warnings": { "type": "array", "items": { "type": "string" } },
                "cache": {
                    "type": "object",
                    "properties": {
                        "hits": { "type": "integer" },
                        "misses": { "type": "integer" }
                    }
                },
                "results": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "title": { "type": "string" },
                            "url": { "type": "string" },
                            "snippet": { "type": "string" },
                            "provider": { "type": "string" },
                            "providers": { "type": "array", "items": { "type": "string" } },
                            "content": { "type": "string" }
                        }
                    }
                }
            }
        })),
        "extract" => Some(json!({
            "type": "object",
            "properties": {
                "pages": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "url": { "type": "string" },
                            "final_url": { "type": "string" },
                            "title": { "type": ["string", "null"] },
                            "text": { "type": "string" },
                            "content_type": { "type": "string" },
                            "error": { "type": "string", "description": "set when the fetch failed" }
                        }
                    }
                }
            }
        })),
        "list_runs" => Some(json!({
            "type": "object",
            "properties": {
                "runs": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string" },
                            "query": { "type": ["string", "null"] },
                            "has_answer": { "type": "boolean" },
                            "sources": { "type": "integer" }
                        }
                    }
                }
            }
        })),
        "get_run" => Some(json!({
            "type": "object",
            "properties": {
                "run": { "type": "string" },
                "artifact": { "type": "string" },
                "total_bytes": { "type": "integer" },
                "offset": { "type": "integer" },
                "page": { "type": "string" },
                "has_more": { "type": "boolean" }
            }
        })),
        _ => None,
    }
}

fn tool_definitions() -> Vec<Value> {
    fn schema(props: Value, required: &[&str]) -> Value {
        let mut s = json!({ "type": "object", "properties": props });
        if !required.is_empty() {
            s["required"] = json!(required);
        }
        s
    }
    let mut tools = vec![
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
        json!({
            "name": "list_runs",
            "description": "List past research runs (newest first) with query, source count and answer availability.",
            "inputSchema": schema(json!({
                "limit": { "type": "integer", "minimum": 1, "maximum": 100 }
            }), &[])
        }),
        json!({
            "name": "get_run",
            "description": "Read one past run by name prefix without knowing paths: returns the chosen artifact (answer by default) paged for token safety.",
            "inputSchema": schema(json!({
                "name": { "type": "string", "description": "run name or unambiguous prefix" },
                "artifact": { "type": "string", "enum": ["answer", "sources", "claims", "plan", "reflections", "manifest"] },
                "offset": { "type": "integer", "minimum": 0 },
                "length": { "type": "integer", "maximum": 32000 }
            }), &["name"])
        }),
    ];
    for tool in &mut tools {
        if let Some(out) = tool_output_schema(tool["name"].as_str().unwrap_or("")) {
            tool["outputSchema"] = out;
        }
    }
    tools
}

/// Run artifacts exposed over MCP resources and `get_run`.
const RUN_ARTIFACTS: &[(&str, &str)] = &[
    ("answer.md", "synthesized report with [n] citations"),
    (
        "sources.json",
        "scored sources incl. content; citations index into this order",
    ),
    ("claims.json", "claim-level verification table"),
    ("plan.json", "planned sub-queries and depth"),
    (
        "reflections.json",
        "distilled notes, follow-ups, disagreements",
    ),
    (
        "run.json",
        "manifest: query, depth, iterations, cost, parent",
    ),
];

fn run_base(server: &McpServer) -> PathBuf {
    crate::commands::run_dir_base(&server.config, None)
}

/// MCP resources: one entry per existing artifact across recent runs.
fn list_resources(server: &McpServer) -> Result<Value, (i64, String)> {
    let base = run_base(server);
    let mut resources = Vec::new();
    for run in vygr_research::list_runs(&base).into_iter().take(50) {
        for (file, desc) in RUN_ARTIFACTS {
            if base.join(&run.name).join(file).exists() {
                resources.push(json!({
                    "uri": format!("vygr://run/{}/{}", run.name, file),
                    "name": format!("{} · {}", run.name, file),
                    "description": desc,
                    "mimeType": if file.ends_with(".md") { "text/markdown" } else { "application/json" },
                }));
            }
        }
    }
    Ok(json!({ "resources": resources }))
}

/// Read a `vygr://run/<name>/<artifact>` resource (single size-capped page).
fn read_resource(server: &McpServer, msg: &Value) -> Result<Value, (i64, String)> {
    let uri = msg
        .pointer("/params/uri")
        .and_then(Value::as_str)
        .ok_or((-32602_i64, "resources/read requires a 'uri'".to_string()))?;
    let rest = uri
        .strip_prefix("vygr://run/")
        .ok_or((-32602_i64, format!("unsupported resource uri: {uri}")))?;
    let (name, file) = rest
        .split_once('/')
        .ok_or((-32602_i64, format!("malformed run uri: {uri}")))?;
    if !RUN_ARTIFACTS.iter().any(|(f, _)| *f == file) {
        return Err((-32602_i64, format!("unknown artifact '{file}'")));
    }
    let dir = vygr_research::resolve_run(&run_base(server), name)
        .map_err(|e| (-32602_i64, e.to_string()))?;
    let mut content = std::fs::read_to_string(dir.join(file))
        .map_err(|e| (-32602_i64, format!("reading {uri}: {e}")))?;
    let truncated = content.len() > PAGE_MAX;
    if truncated {
        // Cut on a char boundary: the byte cap can land mid-UTF-8
        // character in non-ASCII artifacts (CJK, curly quotes, ...).
        let mut cut = PAGE_MAX;
        while !content.is_char_boundary(cut) {
            cut -= 1;
        }
        content.truncate(cut);
        content.push_str("\n... (truncated; use get_artifact/get_run for paged reads)");
    }
    Ok(json!({
        "contents": [{
            "uri": uri,
            "mimeType": if file.ends_with(".md") { "text/markdown" } else { "application/json" },
            "text": content,
        }]
    }))
}

/// Packaged research prompts (MCP `prompts/`): the host agent fills the
/// arguments and receives a ready-to-run research instruction.
fn prompt_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "compare",
            "description": "Structured comparison research between two options",
            "arguments": [
                { "name": "a", "description": "First option", "required": true },
                { "name": "b", "description": "Second option", "required": true }
            ]
        }),
        json!({
            "name": "landscape",
            "description": "Map a technology or market landscape",
            "arguments": [
                { "name": "topic", "description": "The topic to map", "required": true }
            ]
        }),
    ]
}

fn get_prompt(msg: &Value) -> Result<Value, (i64, String)> {
    let name = msg
        .pointer("/params/name")
        .and_then(Value::as_str)
        .ok_or((-32602_i64, "prompts/get requires a 'name'".to_string()))?;
    let arg = |k: &str| {
        msg.pointer(&format!("/params/arguments/{k}"))
            .and_then(Value::as_str)
            .unwrap_or("")
    };
    let text = match name {
        "compare" => {
            let (a, b) = (arg("a"), arg("b"));
            if a.is_empty() || b.is_empty() {
                return Err((-32602_i64, "compare requires 'a' and 'b'".to_string()));
            }
            format!(
                "Research {a} vs {b}: performance, maturity, ecosystem, and risks. \
Run `vygr research \"{a} vs {b}: performance, maturity, ecosystem, risks. Include evidence \
against both sides\" --llm <spec>`. Check plan.json for redundant framings, treat syndicated \
copies as one evidence chain, keep the [n] citations in your summary, and reconcile any \
disagreements the report flags instead of averaging."
            )
        }
        "landscape" => {
            let topic = arg("topic");
            if topic.is_empty() {
                return Err((-32602_i64, "landscape requires 'topic'".to_string()));
            }
            format!(
                "Map the landscape of {topic}: key players, dominant approaches, shifts of the \
last 12 months, and open problems. Use `vygr plan` first, then `vygr research \"{topic} \
landscape: key players, approaches, recent shifts, open problems\" --llm <spec> --mode deep`. \
Cite sources.json entries by number and note where evidence disagrees."
            )
        }
        other => return Err((-32602_i64, format!("unknown prompt '{other}'"))),
    };
    Ok(json!({
        "description": format!("vygr research prompt: {name}"),
        "messages": [{ "role": "user", "content": { "type": "text", "text": text } }]
    }))
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

    // Tools with an `outputSchema` build their payload `Value` once and
    // hand it to `tool_result` alongside the serialized text, so text and
    // `structuredContent` are the same data by construction. Text-only
    // tools leave it `None`.
    let mut structured: Option<Value> = None;
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
                &handle,
                &vygr_core::provider::SearchQuery::new(query, max_results),
            )
            .await;
            let (hits, misses) = handle.cache.snapshot();
            let payload = json!({
                "query": query,
                "provider": provider,
                "warnings": warnings,
                "cache": { "hits": hits, "misses": misses },
                "results": results,
            });
            structured = Some(payload.clone());
            payload.to_string()
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
            // MCP requires structuredContent to be a JSON object, never a
            // bare array: the page list rides in a "pages" key.
            let payload = json!({ "pages": pages });
            structured = Some(payload.clone());
            payload.to_string()
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
                parent: None,
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
            match vygr_research::run(request, llm, server.http.clone(), Some(sink), None).await {
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
        "list_runs" => {
            let limit = arg("limit")
                .and_then(Value::as_u64)
                .unwrap_or(20)
                .clamp(1, 100) as usize;
            let runs: Vec<Value> = vygr_research::list_runs(&run_base(server))
                .into_iter()
                .take(limit)
                .map(|r| {
                    json!({
                        "name": r.name,
                        "query": r.query,
                        "has_answer": r.has_answer,
                        "sources": r.sources,
                    })
                })
                .collect();
            let payload = json!({ "runs": runs });
            structured = Some(payload.clone());
            payload.to_string()
        }
        "get_run" => {
            let name = arg("name").and_then(Value::as_str).ok_or((
                -32602_i64,
                "get_run requires a 'name' string".to_string(),
            ))?;
            let artifact = arg("artifact").and_then(Value::as_str).unwrap_or("answer");
            let file = match artifact {
                "answer" => "answer.md",
                "sources" => "sources.json",
                "claims" => "claims.json",
                "plan" => "plan.json",
                "reflections" => "reflections.json",
                "manifest" => "run.json",
                other => {
                    return Err((
                        -32602_i64,
                        format!("unknown artifact '{other}' (answer|sources|claims|plan|reflections|manifest)"),
                    ))
                }
            };
            let dir = vygr_research::resolve_run(&run_base(server), name)
                .map_err(|e| (-32602_i64, e.to_string()))?;
            let path = dir.join(file);
            if !path.exists() {
                return Ok(tool_error(format!(
                    "run '{name}' has no {file} (pre-0.5.0 runs lack newer artifacts)"
                )));
            }
            let content = std::fs::read_to_string(&path)
                .map_err(|e| (-32602_i64, format!("reading {file}: {e}")))?;
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
            let page: String = content.get(offset..end).unwrap_or_default().to_string();
            let payload = json!({
                "run": dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                "artifact": file,
                "total_bytes": total,
                "offset": offset,
                "page": page,
                "has_more": end < total,
            });
            structured = Some(payload.clone());
            payload.to_string()
        }
        other => {
            return Err((
                -32602_i64,
                format!(
                    "unknown tool '{other}' (available: search, extract, research, get_artifact, list_runs, get_run)"
                ),
            ))
        }
    };
    Ok(tool_result(text, structured))
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

/// A successful `tools/call` result. Tools with an `outputSchema` pass
/// their payload twice — serialized as text and as the same `Value` for
/// `structuredContent` — so the two can never drift. Text-only tools
/// pass `None`; clients that ignore `structuredContent` keep working.
fn tool_result(text: String, structured: Option<Value>) -> Value {
    let mut result = json!({ "content": [{ "type": "text", "text": text }], "isError": false });
    if let Some(payload) = structured {
        result["structuredContent"] = payload;
    }
    result
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

    /// `set_current_dir` is process-global, and the harness runs tests in
    /// parallel threads: tests that chdir serialize on this lock or their
    /// assertions (and TempDir restores) race each other's cwd.
    static CWD_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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

    #[test]
    fn structured_content_is_additive() {
        // Schema'd tools mirror their payload beside the serialized text.
        let payload = json!({ "runs": [] });
        let ok = tool_result(payload.to_string(), Some(payload.clone()));
        assert_eq!(ok["structuredContent"], json!({ "runs": [] }));
        assert_eq!(ok["content"][0]["text"], payload.to_string());
        assert_eq!(ok["isError"], false);

        // Text-only tools never gain structuredContent.
        let plain = tool_result("not json".to_string(), None);
        assert!(plain.get("structuredContent").is_none());
        assert_eq!(plain["content"][0]["text"], "not json");
        assert_eq!(plain["isError"], false);

        let text_only = tool_result(json!({ "page": "x" }).to_string(), None);
        assert!(text_only.get("structuredContent").is_none());
        assert_eq!(text_only["isError"], false);
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
    async fn tools_list_exposes_six_tools() {
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
        assert_eq!(
            names,
            vec![
                "search",
                "extract",
                "research",
                "get_artifact",
                "list_runs",
                "get_run"
            ]
        );
    }

    #[tokio::test]
    async fn tools_list_declares_output_schemas() {
        let s = server();
        let reply = handle_message(
            &s,
            &json!({"jsonrpc":"2.0","id":13,"method":"tools/list"}),
            &noop_notify(),
        )
        .await
        .unwrap();
        let tools = reply["result"]["tools"].as_array().unwrap();
        // Exactly the pure-JSON tools advertise a schema.
        let with_schema: Vec<&str> = tools
            .iter()
            .filter(|t| t.get("outputSchema").is_some())
            .filter_map(|t| t["name"].as_str())
            .collect();
        assert_eq!(
            with_schema,
            vec!["search", "extract", "list_runs", "get_run"]
        );
        for name in ["research", "get_artifact"] {
            let tool = tools.iter().find(|t| t["name"] == json!(name)).unwrap();
            assert!(
                tool.get("outputSchema").is_none(),
                "{name} must stay text-only"
            );
        }
        // The schemas describe the returned shapes.
        let schema_of =
            |name: &str| &tools.iter().find(|t| t["name"] == json!(name)).unwrap()["outputSchema"];
        for key in ["query", "provider", "warnings", "cache", "results"] {
            assert!(schema_of("search")["properties"].get(key).is_some());
        }
        // MCP structuredContent must be a JSON object: the page array
        // rides in a "pages" key.
        assert_eq!(schema_of("extract")["type"], "object");
        assert!(schema_of("extract")["properties"].get("pages").is_some());
        assert!(schema_of("list_runs")["properties"].get("runs").is_some());
        for key in [
            "run",
            "artifact",
            "total_bytes",
            "offset",
            "page",
            "has_more",
        ] {
            assert!(schema_of("get_run")["properties"].get(key).is_some());
        }
    }

    #[tokio::test]
    async fn runs_discovery_and_paging_over_tools() {
        let s = server();
        let tmp = tempfile::TempDir::new().unwrap();
        let run_dir = tmp.path().join("agents/voyager/20261106-120000-demo-run");
        std::fs::create_dir_all(&run_dir).unwrap();
        std::fs::write(
            run_dir.join("answer.md"),
            format!("{}tail", "x".repeat(PAGE_DEFAULT + 10)),
        )
        .unwrap();
        std::fs::write(
            run_dir.join("run.json"),
            r#"{"query":"demo","depth_used":1,"iterations":1,"subqueries":["a"],"reflections":[],"provider_spec":"ddgs","llm":"t"}"#,
        )
        .unwrap();

        let _cwd = CWD_LOCK.lock().await;
        let old = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();
        let call = |name: &str, args: Value| {
            json!({"jsonrpc":"2.0","id":7,"method":"tools/call",
                   "params":{"name":name,"arguments":args}})
        };
        let runs = handle_message(&s, &call("list_runs", json!({})), &noop_notify())
            .await
            .unwrap();
        let parsed: Value =
            serde_json::from_str(runs["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(parsed["runs"][0]["name"], "20261106-120000-demo-run");
        assert_eq!(parsed["runs"][0]["has_answer"], true);
        // The JSON payload is mirrored as structuredContent (outputSchema tools).
        assert_eq!(runs["result"]["structuredContent"], parsed);

        let show = handle_message(
            &s,
            &call(
                "get_run",
                json!({"name":"20261106-120000-demo","length":100}),
            ),
            &noop_notify(),
        )
        .await
        .unwrap();
        let page: Value =
            serde_json::from_str(show["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(page["artifact"], "answer.md");
        assert_eq!(page["has_more"], true);
        assert_eq!(page["page"].as_str().unwrap().len(), 100);
        assert_eq!(show["result"]["structuredContent"], page);
        std::env::set_current_dir(old).unwrap();
    }

    #[tokio::test]
    async fn resources_list_and_read_run_artifacts() {
        let s = server();
        let tmp = tempfile::TempDir::new().unwrap();
        let run_dir = tmp.path().join("agents/voyager/20261106-130000-res-run");
        std::fs::create_dir_all(&run_dir).unwrap();
        std::fs::write(run_dir.join("answer.md"), "# hello\n").unwrap();
        std::fs::write(run_dir.join("claims.json"), "[]").unwrap();

        let _cwd = CWD_LOCK.lock().await;
        let old = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();
        let listed = handle_message(
            &s,
            &json!({"jsonrpc":"2.0","id":8,"method":"resources/list"}),
            &noop_notify(),
        )
        .await
        .unwrap();
        let resources = listed["result"]["resources"].as_array().unwrap();
        assert_eq!(resources.len(), 2);
        let uri = resources[0]["uri"].as_str().unwrap();
        assert!(uri.starts_with("vygr://run/20261106-130000-res-run/"));

        let read = handle_message(
            &s,
            &json!({"jsonrpc":"2.0","id":9,"method":"resources/read","params":{"uri":uri}}),
            &noop_notify(),
        )
        .await
        .unwrap();
        assert_eq!(read["result"]["contents"][0]["text"], "# hello\n");

        // Unknown artifact refused.
        let bad = handle_message(
            &s,
            &json!({"jsonrpc":"2.0","id":10,"method":"resources/read",
                    "params":{"uri":"vygr://run/20261106-130000-res-run/../etc/passwd"}}),
            &noop_notify(),
        )
        .await
        .unwrap();
        assert!(bad["error"].is_object() || bad["result"]["isError"] == json!(true));
        std::env::set_current_dir(old).unwrap();
    }

    #[tokio::test]
    async fn resources_read_truncates_on_a_char_boundary() {
        let s = server();
        let tmp = tempfile::TempDir::new().unwrap();
        let run_dir = tmp.path().join("agents/voyager/20261107-010000-utf8-run");
        std::fs::create_dir_all(&run_dir).unwrap();
        // One 3-byte CJK character straddles the PAGE_MAX byte boundary:
        // truncating at the raw byte cap would panic.
        let mut body = "x".repeat(PAGE_MAX - 1);
        body.push('界');
        assert_eq!(body.len(), PAGE_MAX + 2);
        std::fs::write(run_dir.join("answer.md"), body).unwrap();

        let _cwd = CWD_LOCK.lock().await;
        let old = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();
        let read = handle_message(
            &s,
            &json!({"jsonrpc":"2.0","id":14,"method":"resources/read",
                    "params":{"uri":"vygr://run/20261107-010000-utf8-run/answer.md"}}),
            &noop_notify(),
        )
        .await
        .unwrap();
        let text = read["result"]["contents"][0]["text"].as_str().unwrap();
        assert!(text.starts_with('x'));
        // The cut landed before the CJK character, not inside it, and the
        // truncation note is appended.
        assert!(!text.contains('界'));
        assert!(text.ends_with("... (truncated; use get_artifact/get_run for paged reads)"));
        std::env::set_current_dir(old).unwrap();
    }

    /// Minimal searxng-compatible HTTP server so `search` and `extract`
    /// run without network. Returns the base URL.
    async fn spawn_fake_searxng() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buf = Vec::new();
                    let _ = sock.read_buf(&mut buf).await;
                    let body =
                        r#"{"results":[{"title":"t","url":"http://example.com/a","content":"c"}]}"#;
                    let _ = sock
                        .write_all(
                            format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                                body.len()
                            )
                            .as_bytes(),
                        )
                        .await;
                });
            }
        });
        format!("http://{addr}")
    }

    /// The ADR-0020 promise, enforced: every schema'd tool returns a
    /// `structuredContent` that is a JSON object (MCP requirement) whose
    /// top-level keys are exactly the keys its `outputSchema` declares.
    #[tokio::test]
    async fn structured_content_conforms_to_output_schemas() {
        let mut s = server();
        let base = spawn_fake_searxng().await;
        let mut providers = std::collections::BTreeMap::new();
        providers.insert(
            "searxng".to_string(),
            vygr_core::config::ProviderConf {
                enabled: None,
                base_url: Some(base.clone()),
            },
        );
        s.config.providers = providers;

        let tmp = tempfile::TempDir::new().unwrap();
        let run_dir = tmp.path().join("agents/voyager/20261107-020000-conf-run");
        std::fs::create_dir_all(&run_dir).unwrap();
        std::fs::write(run_dir.join("answer.md"), "# hi\n").unwrap();
        std::fs::write(
            run_dir.join("run.json"),
            r#"{"query":"conf","depth_used":1,"iterations":1,"subqueries":["a"],"reflections":[],"provider_spec":"searxng","llm":"t"}"#,
        )
        .unwrap();

        let _cwd = CWD_LOCK.lock().await;
        let old = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();
        async fn call(s: &McpServer, name: &str, args: Value) -> Value {
            let notify: Notify = Arc::new(|_: Value| {});
            handle_message(
                s,
                &json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
                        "params":{"name":name,"arguments":args}}),
                &notify,
            )
            .await
            .unwrap()
        }
        let search = call(&s, "search", json!({"query":"t","provider":"searxng"})).await;
        let extract = call(&s, "extract", json!({"urls":[format!("{base}/page")]})).await;
        let list = call(&s, "list_runs", json!({})).await;
        let show = call(&s, "get_run", json!({"name":"20261107-020000-conf"})).await;
        std::env::set_current_dir(old).unwrap();

        for (name, reply) in [
            ("search", &search),
            ("extract", &extract),
            ("list_runs", &list),
            ("get_run", &show),
        ] {
            let schema = tool_output_schema(name).unwrap();
            let sc = &reply["result"]["structuredContent"];
            let props = schema["properties"].as_object().unwrap();
            assert!(
                sc.is_object(),
                "{name}: structuredContent must be an object"
            );
            assert_eq!(
                sc.as_object().unwrap().len(),
                props.len(),
                "{name}: structuredContent keys must match the outputSchema"
            );
            for key in props.keys() {
                assert!(sc.get(key).is_some(), "{name}: missing schema key '{key}'");
            }
        }
        // And the payloads carry the real data end to end.
        assert_eq!(
            search["result"]["structuredContent"]["results"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            extract["result"]["structuredContent"]["pages"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            list["result"]["structuredContent"]["runs"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(show["result"]["structuredContent"]["artifact"], "answer.md");
    }

    #[tokio::test]
    async fn prompts_render_templates() {
        let s = server();
        let list = handle_message(
            &s,
            &json!({"jsonrpc":"2.0","id":11,"method":"prompts/list"}),
            &noop_notify(),
        )
        .await
        .unwrap();
        let names: Vec<&str> = list["result"]["prompts"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|p| p["name"].as_str())
            .collect();
        assert_eq!(names, vec!["compare", "landscape"]);

        let got = handle_message(
            &s,
            &json!({"jsonrpc":"2.0","id":12,"method":"prompts/get",
                    "params":{"name":"compare","arguments":{"a":"redis","b":"postgres"}}}),
            &noop_notify(),
        )
        .await
        .unwrap();
        let text = got["result"]["messages"][0]["content"]["text"]
            .as_str()
            .unwrap();
        assert!(text.contains("redis vs postgres"));
        assert!(text.contains("vygr research"));
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
        // the sandbox check (serialized against the other chdir tests).
        let _cwd = CWD_LOCK.lock().await;
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
