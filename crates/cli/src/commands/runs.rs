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
}

#[derive(Debug, Subcommand)]
pub enum Action {
    /// List past runs (newest first)
    List,
    /// Print the answer of a run
    Show { name: String },
    /// Re-synthesize a run offline from its saved sources and notes
    Resume {
        name: String,
        /// LLM spec for the re-synthesis (defaults to config)
        #[arg(long)]
        llm: Option<String>,
    },
}

pub async fn run(args: Args, http: reqwest::Client, cfg: &Config) -> Result<(), VygrError> {
    let base = cfg
        .research
        .run_dir
        .clone()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(vygr_research::default_base);
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
        Action::Show { name } => {
            let dir = vygr_research::resolve_run(&base, &name)?;
            let answer = std::fs::read_to_string(dir.join("answer.md"))
                .map_err(|e| VygrError::Config(format!("run has no answer.md: {e}")))?;
            println!("{answer}");
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
