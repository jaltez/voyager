//! `vygr schema`: machine-readable self-description so LLM agents can
//! discover the full interface at runtime (hsearch-inspired).

use vygr_core::VygrError;

pub fn run() -> Result<(), VygrError> {
    let schema = serde_json::json!({
        "name": "vygr",
        "version": env!("CARGO_PKG_VERSION"),
        "description": "Configurable deep research CLI: provider-chain web search, page extraction, and full research runs over any LLM backend.",
        "usage_hint": "Run `vygr <command> --help` for authoritative flags. Machine output: pass --format json. Diagnostics always go to stderr.",
        "commands": {
            "search": "web search; --provider 'a,b' fallback chain, --all concurrent fan-out, --extract-top N inlines page content, --format table|md|json|urls",
            "extract": "fetch URLs and reduce to plain text; --format text|md|json, --max-chars N",
            "research": "deep-research run; --depth '3' or '2..4', --breadth N, --budget-usd X, --llm <spec>, leaves artifacts under agents/voyager/<run>/",
            "plan": "offline preflight estimate for a research run (always JSON)",
            "providers": "list search providers, required env vars and readiness",
            "models": "browse the models.dev LLM catalog; `vygr models <provider>` lists models and pricing",
            "schema": "this self-description",
            "config": "show effective configuration and paths",
            "init": "install the voyager skill into an agent harness",
            "serve": "run as an MCP server over stdio; tools: search, extract, research, get_artifact (paged reads of run artifacts)",
            "update": "self-update from crates.io; --check only reports (source-checkout builds are refused)"
        },
        "provider_chains": "comma-separated fallback; first provider with results wins; --all queries every provider concurrently and dedups by URL",
        "llm_specs": [
            "pi", "claude", "codex",
            "ollama:<model>",
            "openai-compat (with [llm] base_url + model)",
            "<models.dev provider>:<model>  e.g. openrouter:anthropic/claude-sonnet-4.5"
        ],
        "exit_codes": {
            "0": "success",
            "1": "internal error",
            "2": "usage error",
            "3": "configuration or auth error",
            "4": "provider or network failure"
        }
    });
    println!("{}", serde_json::to_string_pretty(&schema)?);
    Ok(())
}
