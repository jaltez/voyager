//! `vygr research` — full deep-research run over any LLM backend.

use clap::Args as ClapArgs;
use vygr_core::config::Config;
use vygr_core::VygrError;
use vygr_research::{DepthSpec, ResearchRequest};

use crate::commands::read_query;
use crate::output::ReportFormat;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Research question ("-" reads it from stdin)
    pub query: String,

    /// Depth as "3" (fixed) or "2..4" (range; the LLM picks within it).
    /// Defaults to the [research] depth_min..depth_max config.
    #[arg(long)]
    pub depth: Option<String>,

    /// Maximum number of sub-queries per pass
    #[arg(long)]
    pub breadth: Option<u32>,

    /// Hard budget in USD; synthesis is skipped if exceeded
    #[arg(long)]
    pub budget_usd: Option<f64>,

    /// LLM spec: pi | claude | codex | ollama:<model> | openai-compat |
    /// <models.dev provider>:<model>
    #[arg(long)]
    pub llm: Option<String>,

    /// Provider or comma-separated fallback chain
    #[arg(long)]
    pub provider: Option<String>,

    #[arg(long, value_enum, default_value = "md")]
    pub format: ReportFormat,
}

pub async fn run(args: Args, http: reqwest::Client, cfg: &Config) -> Result<(), VygrError> {
    let llm = vygr_llm::resolve(args.llm.as_deref(), &cfg.llm, &http).await?;
    tracing::info!("llm backend: {}", llm.describe());

    let depth = match &args.depth {
        Some(d) => DepthSpec::parse(d)?,
        None => DepthSpec {
            min: cfg.research.depth_min,
            max: cfg.research.depth_max,
        },
    };

    let request = ResearchRequest {
        query: read_query(&args.query)?,
        depth,
        breadth: args.breadth.unwrap_or(cfg.research.breadth),
        budget_usd: args.budget_usd.or(cfg.research.budget_usd),
        max_results_per_query: cfg.research.max_results_per_query,
        fetch_top: cfg.research.fetch_top,
        context_max_chars: cfg.research.context_max_chars,
        provider_spec: args
            .provider
            .clone()
            .or_else(|| cfg.default_provider.clone())
            .unwrap_or_else(|| "ddgs".to_string()),
        run_dir_base: cfg.research.run_dir.clone(),
        politeness: cfg.politeness.clone(),
        cache: cfg.cache.clone(),
    };

    let report = vygr_research::run(request, llm, http).await?;

    match args.format {
        ReportFormat::Md => {
            if let Some(answer) = &report.answer {
                println!("{answer}");
            } else {
                println!("# {}\n", report.query);
                println!("_no answer was synthesized (see warnings below)_\n");
                for w in &report.warnings {
                    eprintln!("vygr: {w}");
                }
            }
        }
        ReportFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
    }
    if let Some(dir) = &report.run_dir {
        eprintln!("vygr: run artifacts: {dir}");
    }
    Ok(())
}
