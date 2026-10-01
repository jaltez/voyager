//! `vygr providers` — list search providers and their configuration status.

use clap::Args as ClapArgs;
use vygr_core::config::Config;
use vygr_core::VygrError;

use crate::output::GenericFormat;

#[derive(Debug, ClapArgs)]
pub struct Args {
    #[arg(long, value_enum, default_value = "table")]
    pub format: GenericFormat,
}

pub fn run(args: Args, cfg: &Config) -> Result<(), VygrError> {
    let default_chain = cfg
        .default_provider
        .clone()
        .unwrap_or_else(|| "ddgs".to_string());

    let rows: Vec<serde_json::Value> = vygr_providers::BUILTIN
        .iter()
        .map(|id| {
            let env = vygr_providers::env_requirement(id);
            let ready = match env {
                Some(name) => std::env::var(name)
                    .map(|v| !v.trim().is_empty())
                    .unwrap_or(false),
                None => true,
            };
            serde_json::json!({
                "id": id,
                "requires_env": env,
                "ready": ready,
                "in_default_chain": default_chain.split(',').any(|p| vygr_providers::canonical(p) == Some(*id)),
            })
        })
        .collect();

    match args.format {
        GenericFormat::Json => println!("{}", serde_json::to_string_pretty(&rows)?),
        GenericFormat::Table => {
            println!(
                "{:<8}  {:<18}  {:<8}  IN DEFAULT CHAIN",
                "ID", "REQUIRES ENV", "READY"
            );
            println!("{}", "-".repeat(60));
            for row in &rows {
                let env = row["requires_env"].as_str().unwrap_or("-");
                let ready = if row["ready"].as_bool().unwrap_or(false) {
                    "yes"
                } else {
                    "no"
                };
                let in_chain = if row["in_default_chain"].as_bool().unwrap_or(false) {
                    "yes"
                } else {
                    ""
                };
                println!(
                    "{:<8}  {:<18}  {:<8}  {}",
                    row["id"].as_str().unwrap_or("?"),
                    env,
                    ready,
                    in_chain
                );
            }
            eprintln!("vygr: default chain: {default_chain}");
        }
    }
    Ok(())
}
