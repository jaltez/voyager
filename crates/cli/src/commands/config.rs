//! `vygr config` — show the effective configuration, layer sources and paths.

use clap::Args as ClapArgs;
use vygr_core::config::Config;
use vygr_core::VygrError;

use crate::output::ConfigFormat;

#[derive(Debug, ClapArgs)]
pub struct Args {
    #[arg(long, value_enum, default_value = "toml")]
    pub format: ConfigFormat,
}

pub fn run(args: Args, cfg: &Config) -> Result<(), VygrError> {
    match args.format {
        ConfigFormat::Json => {
            println!("{}", serde_json::to_string_pretty(cfg)?);
        }
        ConfigFormat::Toml => {
            let body = toml::to_string_pretty(cfg)
                .map_err(|e| VygrError::Parse(format!("serializing config: {e}")))?;
            if cfg.sources.is_empty() {
                println!("# no config files found; showing defaults");
            } else {
                println!(
                    "# layers (in order): {}",
                    cfg.sources
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(" -> ")
                );
            }
            println!("{body}");
        }
    }
    if let Some(path) = Config::user_config_path() {
        eprintln!("vygr: user config path: {}", path.display());
    }
    eprintln!("vygr: project config path: ./.voyager.toml (walked up to the filesystem root)");
    Ok(())
}
