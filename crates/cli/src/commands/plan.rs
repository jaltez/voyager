//! `vygr plan` — offline preflight (Librarium-inspired): show what a
//! research run would do without touching the network.

use clap::Args as ClapArgs;
use vygr_core::config::Config;
use vygr_core::VygrError;

use crate::commands::read_query;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Research question
    pub query: String,

    #[arg(long)]
    pub depth: Option<String>,

    #[arg(long)]
    pub breadth: Option<u32>,
}

pub fn run(args: Args, cfg: &Config) -> Result<(), VygrError> {
    let query = read_query(&args.query)?;
    let (depth_min, depth_max) = match &args.depth {
        Some(d) => {
            let spec = vygr_research::DepthSpec::parse(d)?;
            (spec.min, spec.max)
        }
        None => (cfg.research.depth_min, cfg.research.depth_max),
    };
    let breadth = args.breadth.unwrap_or(cfg.research.breadth);
    let provider = cfg
        .default_provider
        .clone()
        .unwrap_or_else(|| "ddgs".to_string());

    let plan = serde_json::json!({
        "query": query,
        "provider_chain": provider,
        "depth": { "min": depth_min, "max": depth_max },
        "breadth": breadth,
        "estimated": {
            "subqueries": breadth,
            "searches": breadth,
            "results_raw": breadth * cfg.research.max_results_per_query as u32,
            "pages_fetched": cfg.research.fetch_top,
            "llm_calls_min": 2,
        },
        "budget_usd": cfg.research.budget_usd,
        "llm_configured": cfg.llm.backend.is_some() || cfg.llm.model.is_some(),
        "notes": [
            "offline estimate; no network calls were made",
            "cost cannot be estimated until an LLM backend with a price table is configured",
            "current scaffold executes a single pass regardless of depth (phase 2 adds iteration)",
        ],
    });
    println!("{}", serde_json::to_string_pretty(&plan)?);
    Ok(())
}
