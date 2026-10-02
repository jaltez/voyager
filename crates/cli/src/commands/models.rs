//! `vygr models`: browse the models.dev catalog (providers, models,
//! pricing) used for LLM backend discovery and cost estimation.

use clap::Args as ClapArgs;
use vygr_core::VygrError;

use crate::output::GenericFormat;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Provider id to inspect (lists its models); omit to list providers
    pub provider: Option<String>,

    /// Ignore the on-disk catalog cache
    #[arg(long)]
    pub refresh: bool,

    #[arg(long, value_enum, default_value = "table")]
    pub format: GenericFormat,
}

pub async fn run(args: Args, http: reqwest::Client) -> Result<(), VygrError> {
    if args.refresh {
        if let Some(cache) = vygr_core::config::Config::cache_dir() {
            let _ = std::fs::remove_file(cache.join("models-dev.json"));
        }
    }
    let catalog = vygr_llm::models_dev::load(&http).await?;

    match args.provider {
        Some(pid) => {
            let pid = pid.to_lowercase();
            let info = catalog
                .provider(&pid)
                .ok_or_else(|| VygrError::Config(format!("unknown provider '{pid}'")))?;
            let env = info.env.first().cloned().unwrap_or_else(|| "-".to_string());
            let base = info
                .api
                .clone()
                .unwrap_or_else(|| "(no OpenAI-compatible endpoint)".to_string());

            let mut rows: Vec<serde_json::Value> = Vec::new();
            for model in &info.model_ids {
                let cost = catalog.cost(&pid, model);
                rows.push(serde_json::json!({
                    "model": model,
                    "cost_in_per_mtok": cost.map(|c| c.input),
                    "cost_out_per_mtok": cost.map(|c| c.output),
                }));
            }

            match args.format {
                GenericFormat::Json => println!("{}", serde_json::to_string_pretty(&rows)?),
                GenericFormat::Table => {
                    println!("# {} ({pid})\n  env: {env}\n  base: {base}\n", info.name);
                    println!("{:<44}  {:>10}  {:>10}", "MODEL", "$IN/1M", "$OUT/1M");
                    println!("{}", "-".repeat(70));
                    for row in &rows {
                        let cin = row["cost_in_per_mtok"]
                            .as_f64()
                            .map(|v| format!("{v:.2}"))
                            .unwrap_or_else(|| "-".into());
                        let cout = row["cost_out_per_mtok"]
                            .as_f64()
                            .map(|v| format!("{v:.2}"))
                            .unwrap_or_else(|| "-".into());
                        println!(
                            "{:<44}  {:>10}  {:>10}",
                            row["model"].as_str().unwrap_or("?"),
                            cin,
                            cout
                        );
                    }
                }
            }
        }
        None => {
            let rows = catalog.provider_rows();
            match args.format {
                GenericFormat::Json => {
                    let json: Vec<serde_json::Value> = rows
                        .into_iter()
                        .map(|(id, name, env, compat)| {
                            serde_json::json!({
                                "id": id,
                                "name": name,
                                "api_key_env": env,
                                "openai_compatible": compat,
                            })
                        })
                        .collect();
                    println!("{}", serde_json::to_string_pretty(&json)?);
                }
                GenericFormat::Table => {
                    println!(
                        "{:<24}  {:<28}  {:<22}  OPENAI-COMPAT",
                        "ID", "NAME", "API KEY ENV"
                    );
                    println!("{}", "-".repeat(100));
                    for (id, name, env, compat) in rows {
                        println!(
                            "{:<24}  {:<28}  {:<22}  {}",
                            id,
                            vygr_core::types::truncate_chars(&name, 26),
                            env.unwrap_or_else(|| "-".into()),
                            if compat { "yes" } else { "-" }
                        );
                    }
                }
            }
        }
    }
    Ok(())
}
