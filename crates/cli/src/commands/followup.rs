//! `vygr followup <run> "question"`: continue a finished investigation
//! with its evidence as the starting context (ADR-0018). New searches only
//! hunt for what is missing; the parent's sources, notes and URL history
//! carry over, so the answer emphasizes what changed.

use clap::Args as ClapArgs;
use vygr_core::config::Config;
use vygr_core::VygrError;
use vygr_research::{DepthSpec, ParentContext, ResearchRequest, RunManifest, Source};

use crate::commands::{finish_research, read_query, run_dir_base};
use crate::output::ReportFormat;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Run name (or unambiguous prefix) of the parent investigation
    pub run: String,

    /// Follow-up question ("-" reads it from stdin)
    pub query: String,

    /// Preset: quick|standard|deep|exhaustive
    #[arg(long)]
    pub mode: Option<String>,

    #[arg(long)]
    pub depth: Option<String>,

    #[arg(long)]
    pub breadth: Option<u32>,

    /// Hard budget in USD
    #[arg(long)]
    pub budget_usd: Option<f64>,

    /// LLM spec
    #[arg(long)]
    pub llm: Option<String>,

    /// Provider or comma-separated fallback chain
    #[arg(long)]
    pub provider: Option<String>,

    /// Stream the run as NDJSON lifecycle events on stdout
    #[arg(long)]
    pub events: bool,

    /// Print only the report's Summary section
    #[arg(long)]
    pub brief_only: bool,

    /// Run-dir base to resolve the parent against (default: config or
    /// ./.voyager)
    #[arg(long)]
    pub run_dir: Option<String>,

    #[arg(long, value_enum, default_value = "md")]
    pub format: ReportFormat,
}

/// Load a run's artifacts as parent context for a follow-up run.
pub(crate) fn load_parent(base: &std::path::Path, name: &str) -> Result<ParentContext, VygrError> {
    let dir = vygr_research::resolve_run(base, name)?;
    let manifest_path = dir.join("run.json");
    let manifest: RunManifest = std::fs::read_to_string(&manifest_path)
        .map_err(|e| {
            VygrError::Config(format!(
                "{}: {e} (pre-0.5.0 runs have no manifest and cannot be a parent)",
                manifest_path.display()
            ))
        })
        .and_then(|s| serde_json::from_str(&s).map_err(VygrError::Json))?;
    let sources: Vec<Source> = std::fs::read_to_string(dir.join("sources.json"))
        .map_err(VygrError::Io)
        .and_then(|s| serde_json::from_str(&s).map_err(VygrError::Json))
        .map_err(|e| {
            VygrError::Config(format!(
                "{} has no readable sources.json: {e}",
                dir.display()
            ))
        })?;
    // Prefer the reflections artifact (has notes even when the manifest is
    // from an older format); fall back to the manifest copy.
    let notes: Vec<String> = std::fs::read_to_string(dir.join("reflections.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| {
            v.get("notes").and_then(|n| {
                n.as_array().map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect()
                })
            })
        })
        .unwrap_or_else(|| manifest.reflections.clone());
    let run_name = dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| manifest.query.clone());
    Ok(ParentContext {
        run_name,
        notes,
        sources,
    })
}

pub async fn run(args: Args, http: reqwest::Client, cfg: &Config) -> Result<(), VygrError> {
    let llm = vygr_llm::resolve(args.llm.as_deref(), &cfg.llm, &http).await?;
    let base = run_dir_base(cfg, args.run_dir.as_deref());
    let parent = load_parent(&base, &args.run)?;
    eprintln!(
        "vygr: continuing '{}': {} inherited sources, {} notes",
        parent.run_name,
        parent.sources.len(),
        parent.notes.len()
    );

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
            .clone()
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
        output_schema: None,
        parent: Some(parent),
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
