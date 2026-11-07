//! `vygr runs`: inspect past research runs and re-synthesize one offline
//! from its saved evidence (no new searches).

use clap::Args as ClapArgs;
use clap::Subcommand;
use vygr_core::config::Config;
use vygr_core::VygrError;

use crate::output::GenericFormat;

#[derive(Debug, ClapArgs)]
pub struct Args {
    #[command(subcommand)]
    pub action: Action,

    /// Output format for `list`
    #[arg(long, value_enum, global = true, default_value = "table")]
    pub format: GenericFormat,

    /// Run-dir base to operate on (default: config or ./agents/voyager;
    /// env VGR_RUN_DIR)
    #[arg(long, global = true)]
    pub run_dir: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum Action {
    /// List past runs (newest first)
    List,
    /// Print the answer of a run (or export it as DOCX or PDF with --export)
    Show {
        name: String,
        /// Write the answer to a .docx or .pdf file instead of printing
        #[arg(long)]
        export: Option<std::path::PathBuf>,
    },
    /// Re-synthesize a run offline from its saved sources and notes
    Resume {
        name: String,
        /// LLM spec for the re-synthesis (defaults to config)
        #[arg(long)]
        llm: Option<String>,
    },
}

pub async fn run(args: Args, http: reqwest::Client, cfg: &Config) -> Result<(), VygrError> {
    let base = crate::commands::run_dir_base(cfg, args.run_dir.as_deref());
    match args.action {
        Action::List => {
            let runs = vygr_research::list_runs(&base);
            match args.format {
                GenericFormat::Json => println!("{}", serde_json::to_string_pretty(&runs)?),
                GenericFormat::Table => {
                    if runs.is_empty() {
                        println!("no runs under {}", base.display());
                        return Ok(());
                    }
                    println!("{:<38}  {:<3}  {:<7}  QUERY", "RUN", "SRC", "ANSWER");
                    println!("{}", "-".repeat(90));
                    for r in runs {
                        println!(
                            "{:<38}  {:<3}  {:<7}  {}",
                            vygr_core::types::truncate_chars(&r.name, 36),
                            r.sources,
                            if r.has_answer { "yes" } else { "no" },
                            vygr_core::types::truncate_chars(
                                r.query.as_deref().unwrap_or("(pre-0.5.0 run)"),
                                40
                            ),
                        );
                    }
                }
            }
        }
        Action::Show { name, export } => {
            let dir = vygr_research::resolve_run(&base, &name)?;
            let answer = std::fs::read_to_string(dir.join("answer.md"))
                .map_err(|e| VygrError::Config(format!("run has no answer.md: {e}")))?;
            match export {
                Some(path) => {
                    export_answer(&answer, &path)?;
                    println!("exported report: {}", path.display());
                }
                None => println!("{answer}"),
            }
        }
        Action::Resume { name, llm } => {
            let dir = vygr_research::resolve_run(&base, &name)?;
            if !dir.join("run.json").exists() {
                return Err(VygrError::Config(
                    "the run predates run.json manifests and cannot be resumed".to_string(),
                ));
            }
            let llm = vygr_llm::resolve(llm.as_deref(), &cfg.llm, &http).await?;
            tracing::info!("llm backend: {}", llm.describe());
            eprintln!("vygr: re-synthesizing {} offline", dir.display());
            let answer =
                vygr_research::resynthesize(&dir, llm, cfg.research.context_max_chars).await?;
            println!("{answer}");
            eprintln!("vygr: updated {}", dir.join("answer.md").display());
        }
    }
    Ok(())
}

/// Export the answer by extension: `.docx` -> Word, `.pdf` -> the
/// hand-rolled PDF writer, anything else -> a config error naming both
/// formats (exit code 3).
fn export_answer(answer: &str, path: &std::path::Path) -> Result<(), VygrError> {
    match path.extension().and_then(|e| e.to_str()) {
        Some("docx") => crate::docx::export(answer, path),
        Some("pdf") => crate::pdf::export(answer, path),
        _ => Err(VygrError::Config(
            "--export expects a .docx or .pdf path (the converters target the report \
subset vygr synthesizes)"
                .to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_dispatches_on_extension() {
        let tmp = tempfile::tempdir().unwrap();
        let answer = "# Report\n\nBody **bold** with [1] citations.\n";
        let docx = tmp.path().join("report.docx");
        let pdf = tmp.path().join("report.pdf");
        export_answer(answer, &docx).unwrap();
        export_answer(answer, &pdf).unwrap();
        assert_eq!(&std::fs::read(&docx).unwrap()[..2], b"PK", "docx is a zip");
        assert!(
            std::fs::read(&pdf).unwrap().starts_with(b"%PDF-1.4"),
            "pdf writer produced the file"
        );
    }

    #[test]
    fn export_rejects_other_extensions() {
        let tmp = tempfile::tempdir().unwrap();
        for name in ["report.txt", "report.doc", "report"] {
            let err = export_answer("# R\n", &tmp.path().join(name)).unwrap_err();
            assert_eq!(err.exit_code(), 3, "{name} is a config error");
            assert!(err.to_string().contains(".docx or .pdf"), "{name}");
        }
    }
}
