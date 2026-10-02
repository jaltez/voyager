//! End-to-end integration test of the research loop (M2.2/M2.3) with a
//! scripted LLM and a local fake searxng + page server, with no external
//! network required.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use vygr_llm::{CompletionRequest, CompletionResponse, LlmClient};
use vygr_research::{run, DepthSpec, ResearchRequest};

/// LLM that answers the planner, then one reflection, then synthesis.
struct ScriptedLlm {
    calls: AtomicU32,
    costs: [f64; 3],
}

const PLAN_JSON: &str =
    r#"{"subqueries":["rust wasm bench","zig wasm bench","rust wasm criticism"],"depth":2}"#;
const REFLECT_JSON: &str = r#"{"notes":["benchmark numbers differ across engines"],"followups":["wasm gc engine support"],"satisfied":false}"#;

#[async_trait]
impl LlmClient for ScriptedLlm {
    async fn complete(
        &self,
        _req: &CompletionRequest,
    ) -> Result<CompletionResponse, vygr_core::VygrError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        let content = match n {
            0 => PLAN_JSON.to_string(),
            1 => REFLECT_JSON.to_string(),
            _ => "Final answer citing [1] and [2].".to_string(),
        };
        let cost = self.costs.get(n as usize).copied();
        Ok(CompletionResponse {
            content,
            model: "scripted".into(),
            usage: None,
            cost_usd: cost,
        })
    }

    fn describe(&self) -> String {
        "scripted".to_string()
    }
}

/// Wrapper letting an Arc<ScriptedLlm> be boxed as dyn LlmClient while the
/// test keeps a handle to inspect call counts.
struct OwnedLlm(Arc<ScriptedLlm>);

#[async_trait]
impl LlmClient for OwnedLlm {
    async fn complete(
        &self,
        req: &CompletionRequest,
    ) -> Result<CompletionResponse, vygr_core::VygrError> {
        self.0.complete(req).await
    }

    fn describe(&self) -> String {
        self.0.describe()
    }
}

/// Minimal HTTP server: `/search` answers searxng JSON, `/page<i>` answers
/// a small HTML document. Returns the bound port.
async fn spawn_fake_server() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let _ = sock.read_buf(&mut buf).await;
                let req = String::from_utf8_lossy(&buf);
                // The reflection follow-up ("wasm gc engine support") hits
                // a different result set so level 1 finds fresh URLs.
                let (ctype, body) = if req.starts_with("GET /search") && req.contains("gc%20engine")
                {
                    (
                        "application/json",
                        r#"{"results":[
                            {"title":"Wasm GC engines","url":"http://127.0.0.1:PORT/page3","content":"engine support for wasm garbage collection"},
                            {"title":"GC roadmap","url":"http://127.0.0.1:PORT/page4","content":"wasm gc shipping status per browser"}
                        ]}"#
                        .replace("PORT", &port.to_string()),
                    )
                } else if req.starts_with("GET /search") {
                    (
                        "application/json",
                        r#"{"results":[
                            {"title":"Rust wasm bench","url":"http://127.0.0.1:PORT/page1","content":"rust webassembly benchmark numbers"},
                            {"title":"Zig wasm","url":"http://127.0.0.1:PORT/page2","content":"zig webassembly notes"}
                        ]}"#
                        .replace("PORT", &port.to_string()),
                    )
                } else if req.starts_with("GET /page1") {
                    (
                        "text/html",
                        "<html><body><h1>Bench</h1><p>rust webassembly benchmark numbers for the engine</p></body></html>"
                            .to_string(),
                    )
                } else if req.starts_with("GET /page2") {
                    (
                        "text/html",
                        "<html><body><h1>Zig</h1><p>zig webassembly notes about tooling</p></body></html>"
                            .to_string(),
                    )
                } else if req.starts_with("GET /page3") || req.starts_with("GET /page4") {
                    (
                        "text/html",
                        "<html><body><p>wasm gc engine support details and shipping status</p></body></html>"
                            .to_string(),
                    )
                } else {
                    ("text/plain", "not found".to_string())
                };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            });
        }
    });
    port
}

fn request(port: u16, tmp: &tempfile::TempDir) -> ResearchRequest {
    use vygr_core::config::{CacheConf, PolitenessConf, ProviderConf, SearchStackConf};
    let mut providers = std::collections::BTreeMap::new();
    providers.insert(
        "searxng".to_string(),
        ProviderConf {
            enabled: None,
            base_url: Some(format!("http://127.0.0.1:{port}")),
        },
    );
    ResearchRequest {
        query: "compare rust and zig webassembly benchmarks".into(),
        depth: DepthSpec { min: 2, max: 2 },
        breadth: 3,
        budget_usd: Some(10.0),
        max_results_per_query: 5,
        fetch_top: 4,
        context_max_chars: 24_000,
        provider_spec: "searxng".into(),
        run_dir_base: Some(tmp.path().display().to_string()),
        stack: SearchStackConf {
            politeness: PolitenessConf::default(),
            cache: CacheConf::default(),
            providers,
        },
        time_range: None,
        include_domains: vec![],
        exclude_domains: vec![],
        output_schema: None,
    }
}

#[tokio::test]
async fn full_loop_runs_two_levels_with_reflection_and_cost() {
    let port = spawn_fake_server().await;
    let tmp = tempfile::TempDir::new().unwrap();
    let scripted = Arc::new(ScriptedLlm {
        calls: AtomicU32::new(0),
        costs: [0.01, 0.02, 0.04],
    });
    let counter = Arc::clone(&scripted);

    let report = run(
        request(port, &tmp),
        Box::new(OwnedLlm(Arc::clone(&scripted))),
        reqwest::Client::new(),
    )
    .await
    .unwrap();

    // Planner + reflection + synthesis all ran; costs accumulated.
    assert_eq!(counter.calls.load(Ordering::SeqCst), 3);
    assert_eq!(report.iterations, 2);
    assert_eq!(report.depth_used, 2);
    assert!(!report.reflections.is_empty());
    assert_eq!(
        report.reflections[0],
        "benchmark numbers differ across engines"
    );
    assert!(report.answer.as_deref().unwrap().contains("Final answer"));
    assert_eq!(report.cost_usd, Some(0.07));
    assert!(!report.budget_exhausted);
    assert!(report.schema_valid.is_none());
    // Level 1 sources recorded their level.
    assert!(report.sources.iter().any(|s| s.level == 1));
    // Artifacts landed in the run directory.
    let dir = report.run_dir.as_ref().unwrap();
    for file in [
        "prompt.md",
        "plan.json",
        "sources.json",
        "answer.md",
        "reflections.json",
    ] {
        assert!(
            std::path::Path::new(dir).join(file).exists(),
            "missing {file}"
        );
    }
}

#[tokio::test]
async fn budget_exhaustion_skips_synthesis_but_keeps_sources() {
    let port = spawn_fake_server().await;
    let tmp = tempfile::TempDir::new().unwrap();
    let scripted = Arc::new(ScriptedLlm {
        calls: AtomicU32::new(0),
        costs: [5.0, 5.0, 5.0],
    });

    let mut req = request(port, &tmp);
    req.budget_usd = Some(6.0); // planner (5.0) + reflection (5.0) blow it
    let report = run(
        req,
        Box::new(OwnedLlm(Arc::clone(&scripted))),
        reqwest::Client::new(),
    )
    .await
    .unwrap();

    assert!(report.budget_exhausted);
    assert!(report.answer.is_none());
    assert!(!report.sources.is_empty());
    assert_eq!(report.cost_usd, Some(10.0));
}
