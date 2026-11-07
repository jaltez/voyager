//! `vygr research`: full deep-research run over any LLM backend.

use clap::Args as ClapArgs;
use vygr_core::config::Config;
use vygr_core::VygrError;
use vygr_research::{DepthSpec, ResearchRequest};

use crate::commands::{finish_research, read_query};
use crate::output::ReportFormat;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Research question ("-" reads it from stdin)
    pub query: String,

    /// Preset: quick|standard|deep|exhaustive. Explicit depth/breadth
    /// flags override the preset; the preset overrides config defaults.
    #[arg(long)]
    pub mode: Option<String>,

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

    /// JSON Schema file the synthesized answer must satisfy (JSON output)
    #[arg(long)]
    pub output_schema: Option<String>,

    /// Stream the run as NDJSON lifecycle events on stdout; the final
    /// `report` line carries the whole ResearchReport
    #[arg(long)]
    pub events: bool,

    /// Print only the report's Summary section (context-tight agents)
    #[arg(long)]
    pub brief_only: bool,

    /// Base directory for this run's artifacts (default: ./agents/voyager
    /// or [research] run_dir; env VGR_RUN_DIR)
    #[arg(long)]
    pub run_dir: Option<String>,

    #[arg(long, value_enum, default_value = "md")]
    pub format: ReportFormat,
}

/// Read the `--output-schema` JSON Schema file.
fn load_schema(path: &str) -> Result<String, VygrError> {
    std::fs::read_to_string(path)
        .map_err(|e| VygrError::Config(format!("output schema {path}: {e}")))
}

pub async fn run(args: Args, http: reqwest::Client, cfg: &Config) -> Result<(), VygrError> {
    let llm = vygr_llm::resolve(args.llm.as_deref(), &cfg.llm, &http).await?;
    tracing::info!("llm backend: {}", llm.describe());

    // Preset fills what the flags and config leave open (ADR-0017).
    let preset = match &args.mode {
        Some(m) => Some(vygr_research::ResearchMode::parse(m)?.preset()),
        None => None,
    };
    let depth = match &args.depth {
        Some(d) => DepthSpec::parse(d)?,
        None => preset.map(|p| p.0).unwrap_or(DepthSpec {
            min: cfg.research.depth_min,
            max: cfg.research.depth_max,
        }),
    };

    let request = ResearchRequest {
        query: read_query(&args.query)?,
        depth,
        breadth: args
            .breadth
            .or(preset.map(|p| p.1))
            .unwrap_or(cfg.research.breadth),
        budget_usd: args.budget_usd.or(cfg.research.budget_usd),
        max_results_per_query: cfg.research.max_results_per_query,
        fetch_top: preset.map(|p| p.2).unwrap_or(cfg.research.fetch_top),
        context_max_chars: cfg.research.context_max_chars,
        provider_spec: args
            .provider
            .clone()
            .or_else(|| cfg.default_provider.clone())
            .unwrap_or_else(|| "ddgs".to_string()),
        run_dir_base: args
            .run_dir
            .or_else(|| std::env::var("VGR_RUN_DIR").ok())
            .or_else(|| cfg.research.run_dir.clone()),
        stack: vygr_core::config::SearchStackConf::from_config(cfg),
        time_range: match cfg.research.time_range.as_deref() {
            Some(s) => Some(vygr_core::provider::TimeRange::parse(s)?),
            None => None,
        },
        include_domains: vygr_core::provider::normalize_domains(
            cfg.research.include_domains.clone(),
        ),
        exclude_domains: vygr_core::provider::normalize_domains(
            cfg.research.exclude_domains.clone(),
        ),
        language: cfg.research.language.clone(),
        output_schema: match &args.output_schema {
            Some(path) => Some(load_schema(path)?),
            None => None,
        },
        parent: None,
    };

    finish_research(
        request,
        llm,
        http,
        args.events,
        args.format,
        args.brief_only,
    )
    .await
}
