//! `vygr monitor`: recurring research with trend snapshots (ADR-0018,
//! the concrete minimal form of deferred "cross-run memory"). A monitor
//! stores a topic and the name of its last run; `monitor run` continues
//! from that run like a follow-up, so the report emphasizes what changed
//! since the previous check, and links the new run as the next baseline.

use std::path::PathBuf;

use clap::Args as ClapArgs;
use vygr_core::config::Config;
use vygr_core::VygrError;
use vygr_research::{DepthSpec, ResearchRequest};

use crate::commands::run_dir_base;
use crate::output::ReportFormat;

#[derive(Debug, ClapArgs)]
pub struct Args {
    #[command(subcommand)]
    pub command: Sub,
}

#[derive(Debug, clap::Subcommand)]
pub enum Sub {
    /// Start monitoring a topic
    Add {
        /// Short identifier for the monitor
        name: String,
        /// The topic (research question) to watch
        topic: String,
    },
    /// List configured monitors
    List {
        #[arg(long, value_enum, default_value = "table")]
        format: crate::output::GenericFormat,
    },
    /// Run a monitor now: fresh research continuing from the last run
    Run {
        /// Monitor name
        name: String,
        /// LLM spec
        #[arg(long)]
        llm: Option<String>,
        /// Hard budget in USD
        #[arg(long)]
        budget_usd: Option<f64>,
        /// Preset: quick|standard|deep|exhaustive (default: quick-ish
        /// checks favor `quick` for recurring topics)
        #[arg(long)]
        mode: Option<String>,
        #[arg(long)]
        events: bool,
        #[arg(long)]
        brief_only: bool,
        #[arg(long, value_enum, default_value = "md")]
        format: ReportFormat,
    },
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct MonitorFile {
    name: String,
    topic: String,
    /// Run directory name of the last check, if one happened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_run: Option<String>,
    /// Unix seconds of the last update.
    updated_unix: u64,
}

fn monitors_dir() -> Result<PathBuf, VygrError> {
    dirs::data_dir()
        .map(|d| d.join("voyager").join("monitors"))
        .ok_or_else(|| VygrError::Config("cannot determine data directory".to_string()))
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn valid_name(name: &str) -> Result<(), VygrError> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(VygrError::Config(
            "monitor name must be non-empty ascii [a-zA-Z0-9_-]".to_string(),
        ));
    }
    Ok(())
}

fn write_monitor(m: &MonitorFile) -> Result<PathBuf, VygrError> {
    let dir = monitors_dir()?;
    std::fs::create_dir_all(&dir).map_err(VygrError::Io)?;
    let path = dir.join(format!("{}.json", m.name));
    std::fs::write(
        &path,
        serde_json::to_string_pretty(m).map_err(VygrError::Json)?,
    )
    .map_err(VygrError::Io)?;
    Ok(path)
}

pub async fn run(args: Args, http: reqwest::Client, cfg: &Config) -> Result<(), VygrError> {
    match args.command {
        Sub::Add { name, topic } => {
            valid_name(&name)?;
            let path = monitors_dir()?.join(format!("{name}.json"));
            if path.exists() {
                return Err(VygrError::Config(format!(
                    "monitor '{name}' already exists ({})",
                    path.display()
                )));
            }
            let monitor = MonitorFile {
                name,
                topic: topic.trim().to_string(),
                last_run: None,
                updated_unix: now_unix(),
            };
            let path = write_monitor(&monitor)?;
            println!("monitor added: {}", path.display());
            eprintln!("vygr: run it with `vygr monitor run {}`", monitor.name);
            Ok(())
        }
        Sub::List { format } => {
            let dir = monitors_dir()?;
            let mut monitors: Vec<MonitorFile> = std::fs::read_dir(&dir)
                .into_iter()
                .flatten()
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().map(|x| x == "json").unwrap_or(false))
                .filter_map(|e| {
                    std::fs::read_to_string(e.path())
                        .ok()
                        .and_then(|s| serde_json::from_str(&s).ok())
                })
                .collect();
            monitors.sort_by(|a, b| a.name.cmp(&b.name));
            match format {
                crate::output::GenericFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&monitors)?);
                }
                crate::output::GenericFormat::Table => {
                    if monitors.is_empty() {
                        eprintln!(
                            "vygr: no monitors; add one with `vygr monitor add <name> \"topic\"`"
                        );
                        return Ok(());
                    }
                    println!("{:<16}  {:<44}  LAST RUN", "NAME", "TOPIC");
                    println!("{}", "-".repeat(80));
                    for m in &monitors {
                        println!(
                            "{:<16}  {:<44}  {}",
                            m.name,
                            vygr_core::types::truncate_chars(&m.topic, 42),
                            m.last_run.as_deref().unwrap_or("-")
                        );
                    }
                    eprintln!("vygr: {} monitor(s) in {}", monitors.len(), dir.display());
                }
            }
            Ok(())
        }
        Sub::Run {
            name,
            llm,
            budget_usd,
            mode,
            events,
            brief_only,
            format,
        } => {
            valid_name(&name)?;
            let path = monitors_dir()?.join(format!("{name}.json"));
            let monitor: MonitorFile = serde_json::from_str(
                &std::fs::read_to_string(&path)
                    .map_err(|e| VygrError::Config(format!("{}: {e}", path.display())))?,
            )
            .map_err(VygrError::Json)?;

            let llm_client = vygr_llm::resolve(llm.as_deref(), &cfg.llm, &http).await?;
            let base = run_dir_base(cfg, None);
            let parent = monitor
                .last_run
                .as_deref()
                .map(|last| crate::commands::followup::load_parent(&base, last))
                .transpose()
                .unwrap_or_else(|e| {
                    tracing::warn!("baseline run unavailable ({e}); running a fresh check");
                    eprintln!("vygr: baseline run unavailable; running a fresh check");
                    None
                });
            if let Some(p) = &parent {
                eprintln!(
                    "vygr: checking '{}': {} inherited sources from {}",
                    monitor.name,
                    p.sources.len(),
                    p.run_name
                );
            }

            let preset = match &mode {
                Some(m) => Some(vygr_research::ResearchMode::parse(m)?.preset()),
                None => None,
            };
            let depth = preset.map(|p| p.0).unwrap_or(DepthSpec {
                min: cfg.research.depth_min,
                max: cfg.research.depth_max,
            });
            let request = ResearchRequest {
                query: monitor.topic.clone(),
                depth,
                breadth: preset.map(|p| p.1).unwrap_or(cfg.research.breadth),
                budget_usd: budget_usd.or(cfg.research.budget_usd),
                max_results_per_query: cfg.research.max_results_per_query,
                fetch_top: preset.map(|p| p.2).unwrap_or(cfg.research.fetch_top),
                context_max_chars: cfg.research.context_max_chars,
                provider_spec: cfg
                    .default_provider
                    .clone()
                    .unwrap_or_else(|| "ddgs".to_string()),
                run_dir_base: std::env::var("VGR_RUN_DIR")
                    .ok()
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
                parent,
            };

            // The run dir name is needed to advance the baseline, so run
            // the engine here instead of through `finish_research`.
            let progress: vygr_research::ProgressSink =
                std::sync::Arc::new(|msg| eprintln!("vygr: {msg}"));
            let event_sink: Option<vygr_research::EventSink> = if events {
                Some(std::sync::Arc::new(|ev: &vygr_research::ResearchEvent| {
                    if let Ok(line) = serde_json::to_string(ev) {
                        println!("{line}");
                    }
                }))
            } else {
                None
            };
            let report =
                match vygr_research::run(request, llm_client, http, Some(progress), event_sink)
                    .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        if events {
                            println!(
                                "{}",
                                serde_json::json!({"event": "error", "message": e.to_string()})
                            );
                        }
                        return Err(e);
                    }
                };
            if let Some(dir) = &report.run_dir {
                if let Some(run_name) = std::path::Path::new(dir).file_name() {
                    let updated = MonitorFile {
                        name: monitor.name.clone(),
                        topic: monitor.topic.clone(),
                        last_run: Some(run_name.to_string_lossy().to_string()),
                        updated_unix: now_unix(),
                    };
                    write_monitor(&updated)?;
                    eprintln!(
                        "vygr: monitor '{}' baseline advanced to {dir}",
                        monitor.name
                    );
                }
            }
            if events {
                let mut last = serde_json::json!({ "event": "report" });
                if let Ok(value) = serde_json::to_value(&report) {
                    last["report"] = value;
                }
                println!("{last}");
                return Ok(());
            }
            if brief_only {
                match &report.answer {
                    Some(answer) => match crate::output::extract_summary(answer) {
                        Some(summary) => println!("{summary}"),
                        None => println!("{answer}"),
                    },
                    None => eprintln!("vygr: no answer was synthesized (see warnings)"),
                }
                return Ok(());
            }
            match format {
                ReportFormat::Md => {
                    if let Some(answer) = &report.answer {
                        println!("{answer}");
                    } else {
                        eprintln!("vygr: no answer was synthesized (see warnings)");
                    }
                }
                ReportFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&report)?);
                }
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monitor_names_are_restricted() {
        assert!(valid_name("rust-ecosystem").is_ok());
        assert!(valid_name("wasm_watch").is_ok());
        assert!(valid_name("").is_err());
        assert!(valid_name("../escape").is_err());
        assert!(valid_name("has space").is_err());
    }
}
