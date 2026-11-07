//! Subcommand implementations.

pub mod cache;
pub mod config;
pub mod doctor;
pub mod eval;
pub mod extract;
pub mod followup;
pub mod init;
pub mod models;
pub mod monitor;
pub mod plan;
pub mod providers;
pub mod research;
pub mod runs;
pub mod schema;
pub mod search;
pub mod update;

use std::io::Read;
use std::path::PathBuf;

use vygr_core::config::Config;
use vygr_core::VygrError;

/// Split a comma-separated domain list into normalized entries.
pub fn parse_domain_list(spec: Option<&str>) -> Vec<String> {
    vygr_core::provider::normalize_domains(
        spec.unwrap_or_default()
            .split(',')
            .map(str::to_string)
            .collect(),
    )
}

/// Resolve a query argument: `"-"` reads the query from stdin.
pub fn read_query(raw: &str) -> Result<String, VygrError> {
    let raw = raw.trim();
    if raw != "-" {
        return Ok(raw.to_string());
    }
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .map_err(VygrError::Io)?;
    let query = buf.trim().to_string();
    if query.is_empty() {
        return Err(VygrError::Config("empty query on stdin".to_string()));
    }
    Ok(query)
}

/// Effective run-dir base shared by `runs`, `eval`, `followup` and
/// `monitor` (same precedence as `research --run-dir`).
pub(crate) fn run_dir_base(cfg: &Config, override_dir: Option<&str>) -> PathBuf {
    override_dir
        .map(PathBuf::from)
        .or_else(|| std::env::var("VGR_RUN_DIR").ok().map(PathBuf::from))
        .or_else(|| cfg.research.run_dir.clone().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("./agents/voyager"))
}

/// Shared execution + output path for every command that starts a
/// research run (`research`, `followup`, `monitor run`): progress to
/// stderr, optional NDJSON events to stdout, then md/json/brief output.
pub(crate) async fn finish_research(
    request: vygr_research::ResearchRequest,
    llm: Box<dyn vygr_llm::LlmClient>,
    http: reqwest::Client,
    events: bool,
    format: crate::output::ReportFormat,
    brief_only: bool,
) -> Result<(), VygrError> {
    let progress: vygr_research::ProgressSink = std::sync::Arc::new(|msg| eprintln!("vygr: {msg}"));
    let event_sink: Option<vygr_research::EventSink> = if events {
        Some(std::sync::Arc::new(|ev: &vygr_research::ResearchEvent| {
            if let Ok(line) = serde_json::to_string(ev) {
                println!("{line}");
            }
        }))
    } else {
        None
    };
    let report = match vygr_research::run(request, llm, http, Some(progress), event_sink).await {
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

    if events {
        // Final line: the complete report so one `jq` pass gets everything.
        let mut last = serde_json::json!({ "event": "report" });
        if let Ok(value) = serde_json::to_value(&report) {
            last["report"] = value;
        }
        println!("{last}");
        if let Some(dir) = &report.run_dir {
            eprintln!("vygr: run artifacts: {dir}");
        }
        return Ok(());
    }

    if brief_only {
        match &report.answer {
            Some(answer) => match crate::output::extract_summary(answer) {
                Some(summary) => println!("{summary}"),
                None => {
                    eprintln!("vygr: no Summary section in the report; printing the full answer");
                    println!("{answer}");
                }
            },
            None => {
                eprintln!("vygr: no answer was synthesized (see warnings)");
                for w in &report.warnings {
                    eprintln!("vygr: {w}");
                }
            }
        }
        if let Some(dir) = &report.run_dir {
            eprintln!("vygr: run artifacts: {dir}");
        }
        return Ok(());
    }

    match format {
        crate::output::ReportFormat::Md => {
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
        crate::output::ReportFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
    }
    if let Some(dir) = &report.run_dir {
        eprintln!("vygr: run artifacts: {dir}");
    }
    Ok(())
}
