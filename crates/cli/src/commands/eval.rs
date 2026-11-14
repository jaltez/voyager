//! `vygr eval <run>`: deterministic quality scorecard for a finished run
//! (ADR-0017): citation resolvability, lexical verification, sub-query
//! coverage, disconfirmation attempt, source diversity. Results are also
//! written to `eval.json` next to the run's artifacts.

use clap::Args as ClapArgs;
use vygr_core::config::Config;
use vygr_core::VygrError;

use crate::commands::run_dir_base;
use crate::output::GenericFormat;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Run name (or unambiguous prefix)
    pub run: String,

    /// Run-dir base to resolve the run against (default: config or
    /// ./.voyager)
    #[arg(long)]
    pub run_dir: Option<String>,

    #[arg(long, value_enum, default_value = "table")]
    pub format: GenericFormat,
}

pub fn run(args: Args, cfg: &Config) -> Result<(), VygrError> {
    let base = run_dir_base(cfg, args.run_dir.as_deref());
    let dir = vygr_research::resolve_run(&base, &args.run)?;
    let report = vygr_research::evaluate(&dir)?;

    // Persist the scorecard as a run artifact.
    let _ = std::fs::write(
        dir.join("eval.json"),
        serde_json::to_string_pretty(&report).unwrap_or_default(),
    );

    match args.format {
        GenericFormat::Json => println!("{}", serde_json::to_string_pretty(&report)?),
        GenericFormat::Table => {
            println!("eval: {}", report.run);
            if let Some(q) = &report.query {
                println!("query: {q}");
            }
            println!();
            println!("{:<34} {:>8}", "CHECK", "VALUE");
            println!("{}", "-".repeat(44));
            println!(
                "{:<34} {:>7.1}%",
                "citation resolvability", report.citation_resolvable_pct
            );
            println!(
                "{:<34} {:>4} / {} weak",
                "lexical verification", report.verification_checked, report.verification_weak
            );
            println!(
                "{:<34} {:>7.1}%",
                "sub-query coverage", report.subquery_coverage_pct
            );
            println!(
                "{:<34} {:>8}",
                "disconfirmation attempted",
                if report.disconfirmation_attempted {
                    "yes"
                } else {
                    "no"
                }
            );
            println!(
                "{:<34} {:>8}",
                "sources / hosts / clusters",
                format!(
                    "{}/{} / {}",
                    report.diversity.sources,
                    report.diversity.distinct_hosts,
                    report.diversity.syndication_clusters
                )
            );
            println!("{}", "-".repeat(44));
            println!("{:<34} {:>7.1}%", "score", report.score_pct);
            eprintln!(
                "vygr: scorecard written to {}",
                dir.join("eval.json").display()
            );
        }
    }
    Ok(())
}
