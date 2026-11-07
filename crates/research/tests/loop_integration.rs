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
            _ => "rust webassembly benchmark numbers support the comparison [1]. zig webassembly notes complete the picture [2].".to_string(),
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
        language: None,
        output_schema: None,
        parent: None,
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
    let events = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let sink_events = Arc::clone(&events);
    let progress: vygr_research::ProgressSink = Arc::new(move |msg: &str| {
        sink_events.lock().unwrap().push(msg.to_string());
    });

    let report = run(
        request(port, &tmp),
        Box::new(OwnedLlm(Arc::clone(&scripted))),
        reqwest::Client::new(),
        Some(progress),
        None,
    )
    .await
    .unwrap();

    // Progress fired for plan, level, reflection, synthesis.
    let events = events.lock().unwrap();
    assert!(events.iter().any(|e| e.starts_with("plan ready")));
    assert!(events.iter().any(|e| e.starts_with("level 1/2")));
    assert!(events.iter().any(|e| e.starts_with("reflection:")));
    assert!(events.iter().any(|e| e.starts_with("synthesizing")));
    assert!(events.iter().any(|e| e.starts_with("verification:")));

    // Planner + reflection + synthesis all ran; costs accumulated.
    assert_eq!(counter.calls.load(Ordering::SeqCst), 3);
    assert_eq!(report.iterations, 2);
    assert_eq!(report.depth_used, 2);
    assert!(!report.reflections.is_empty());
    assert_eq!(
        report.reflections[0],
        "benchmark numbers differ across engines"
    );
    assert!(report
        .answer
        .as_deref()
        .unwrap()
        .contains("benchmark numbers"));
    assert_eq!(report.cost_usd, Some(0.07));
    assert!(!report.budget_exhausted);
    assert!(!report.degraded);
    assert!(report.schema_valid.is_none());
    // The scripted answer cites real source vocabulary: clean check.
    let verification = report.verification.as_ref().unwrap();
    assert!(verification.checked >= 2);
    assert_eq!(verification.weak_count(), 0);
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
        None,
        None,
    )
    .await
    .unwrap();

    assert!(report.budget_exhausted);
    // Degraded deterministic report instead of no answer at all.
    assert!(report.degraded);
    let answer = report.answer.as_deref().unwrap();
    assert!(answer.contains("(degraded)"));
    assert!(!report.sources.is_empty());
    assert_eq!(report.cost_usd, Some(10.0));
    assert!(report.verification.is_none());
}

#[tokio::test]
async fn event_sink_receives_structured_lifecycle() {
    let port = spawn_fake_server().await;
    let tmp = tempfile::TempDir::new().unwrap();
    let scripted = Arc::new(ScriptedLlm {
        calls: AtomicU32::new(0),
        costs: [0.01, 0.02, 0.04],
    });
    let collected = Arc::new(std::sync::Mutex::new(
        Vec::<vygr_research::ResearchEvent>::new(),
    ));
    let sink_events = Arc::clone(&collected);
    let events: vygr_research::EventSink = Arc::new(move |ev: &vygr_research::ResearchEvent| {
        sink_events.lock().unwrap().push(ev.clone());
    });

    let _report = run(
        request(port, &tmp),
        Box::new(OwnedLlm(Arc::clone(&scripted))),
        reqwest::Client::new(),
        None,
        Some(events),
    )
    .await
    .unwrap();

    let evs = collected.lock().unwrap();
    let names: Vec<String> = evs
        .iter()
        .map(|e| {
            serde_json::to_value(e).unwrap()["event"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(names.first().map(String::as_str), Some("run_start"));
    for expected in [
        "plan",
        "level_start",
        "sources_found",
        "reflection",
        "synthesis_start",
        "verified",
    ] {
        assert!(
            names.iter().any(|n| n == expected),
            "missing event `{expected}` in {names:?}"
        );
    }
    assert_eq!(names.last().map(String::as_str), Some("done"));
    // Every event serializes to a single NDJSON line tagged with its name.
    for ev in evs.iter() {
        let line = serde_json::to_string(ev).unwrap();
        assert!(line.starts_with("{\"event\":\""), "bad NDJSON: {line}");
    }
    // The done event carries cost and the run directory.
    let done = serde_json::to_value(evs.last().unwrap()).unwrap();
    assert_eq!(done["cost_usd"], serde_json::json!(0.07));
    assert!(done["run_dir"].as_str().is_some());
}

#[tokio::test]
async fn followup_run_seeds_parent_context_and_skips_visited_urls() {
    let port = spawn_fake_server().await;
    let tmp = tempfile::TempDir::new().unwrap();
    let scripted = Arc::new(ScriptedLlm {
        calls: AtomicU32::new(0),
        costs: [0.01, 0.02, 0.04],
    });
    let parent = run(
        request(port, &tmp),
        Box::new(OwnedLlm(Arc::clone(&scripted))),
        reqwest::Client::new(),
        None,
        None,
    )
    .await
    .unwrap();
    assert!(!parent.sources.is_empty());

    // Follow-up with the parent's evidence: level 0 finds no fresh URLs
    // (all parent URLs are already visited), so it stops immediately and
    // synthesizes from the inherited evidence.
    let scripted2 = Arc::new(ScriptedLlm {
        calls: AtomicU32::new(0),
        costs: [0.01, 0.02, 0.04],
    });
    let parent_name = std::path::Path::new(parent.run_dir.as_ref().unwrap())
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let parent_ctx = vygr_research::ParentContext {
        run_name: parent_name.clone(),
        notes: parent.reflections.clone(),
        sources: parent.sources.clone(),
    };
    let mut req = request(port, &tmp);
    req.query = "what about wasm gc engine support?".into();
    req.parent = Some(parent_ctx);
    let report = run(
        req,
        Box::new(OwnedLlm(Arc::clone(&scripted2))),
        reqwest::Client::new(),
        None,
        None,
    )
    .await
    .unwrap();

    // Every parent source is inherited into the follow-up's evidence.
    for s in &parent.sources {
        assert!(
            report.sources.iter().any(|r| r.url == s.url),
            "parent source {} missing from follow-up",
            s.url
        );
    }
    // The manifest records the parent link; a parent.json marker exists.
    let dir = report.run_dir.as_ref().unwrap();
    let manifest: vygr_research::RunManifest = serde_json::from_str(
        &std::fs::read_to_string(std::path::Path::new(dir).join("run.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        manifest.parent.as_deref(),
        Some(parent_name.as_str()),
        "parent field was: {:?}",
        manifest.parent
    );
    assert!(std::path::Path::new(dir).join("parent.json").exists());
    assert!(report.answer.as_deref().unwrap().contains("benchmark"));
}
