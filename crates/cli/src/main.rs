//! vygr: the voyager deep research CLI.

mod commands;
mod docx;
mod markdown;
mod mcp;
mod mcp_http;
mod output;
mod pdf;

use clap::{CommandFactory, Parser, Subcommand};
use vygr_core::Config;

#[derive(Debug, Parser)]
#[command(
    name = "vygr",
    version,
    about = "voyager: configurable deep research CLI",
    long_about = "voyager: configurable deep research CLI.\n\n\
Search the web through provider chains (ddgs keyless, brave, tavily), run full \
deep-research loops over any LLM backend (models.dev providers, Ollama, or the \
LLM already configured in a harness like pi), and leave a complete evidence \
trail on disk.\n\n\
Docs: https://github.com/jaltez/voyager"
)]
struct Cli {
    /// Increase log verbosity: -v info, -vv debug (logs go to stderr)
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Web search through a configurable provider chain
    Search(commands::search::Args),
    /// Fetch pages and reduce them to plain text
    Extract(commands::extract::Args),
    /// Full deep-research run (requires an LLM backend)
    Research(commands::research::Args),
    /// Offline preflight: show what a research run would do
    Plan(commands::plan::Args),
    /// List search providers and their configuration status
    Providers(commands::providers::Args),
    /// Browse the models.dev LLM catalog
    Models(commands::models::Args),
    /// Print a machine-readable self-description for agents
    Schema,
    /// Show the effective configuration and file paths
    Config(commands::config::Args),
    /// Install the voyager skill into an agent harness
    Init(commands::init::Args),
    /// Inspect or clear the on-disk search cache
    Cache(commands::cache::Args),
    /// Run as an MCP server: stdio by default, Streamable HTTP with --http
    Serve {
        /// Serve MCP over Streamable HTTP at IP:PORT (e.g. 127.0.0.1:8080)
        /// instead of stdio; refused on non-loopback binds without a token
        #[arg(long, value_name = "IP:PORT")]
        http: Option<String>,
        /// Bearer token every HTTP request must send (default: VGR_MCP_TOKEN;
        /// prefer the env var — a flag is visible in ps(1) and shell history)
        #[arg(long, value_name = "SECRET")]
        token: Option<String>,
    },
    /// Self-update from crates.io (`--check` only reports)
    Update(commands::update::Args),
    /// Inspect past research runs; re-synthesize offline
    Runs(commands::runs::Args),
    /// Continue a finished investigation from its saved evidence
    Followup(commands::followup::Args),
    /// Recurring research with trend snapshots
    Monitor(commands::monitor::Args),
    /// Deterministic quality scorecard for a finished run
    Eval(commands::eval::Args),
    /// Self-check the environment: providers, LLM backend, dirs, skill
    Doctor(commands::doctor::Args),
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    init_tracing(cli.verbose);
    let http = http_client();
    let cfg = Config::load();

    let result = match cli.command {
        Commands::Search(args) => commands::search::run(args, http, &cfg).await,
        Commands::Extract(args) => commands::extract::run(args, http).await,
        Commands::Research(args) => commands::research::run(args, http, &cfg).await,
        Commands::Plan(args) => commands::plan::run(args, &cfg),
        Commands::Providers(args) => commands::providers::run(args, &cfg),
        Commands::Models(args) => commands::models::run(args, http).await,
        Commands::Schema => commands::schema::run(),
        Commands::Config(args) => commands::config::run(args, &cfg),
        Commands::Init(args) => commands::init::run(args),
        Commands::Cache(args) => commands::cache::run(args),
        Commands::Serve {
            http: http_addr,
            token,
        } => mcp::run(http, http_addr, token).await,
        Commands::Update(args) => commands::update::run(args, http).await,
        Commands::Runs(args) => commands::runs::run(args, http, &cfg).await,
        Commands::Followup(args) => commands::followup::run(args, http, &cfg).await,
        Commands::Monitor(args) => commands::monitor::run(args, http, &cfg).await,
        Commands::Eval(args) => commands::eval::run(args, &cfg),
        Commands::Doctor(args) => commands::doctor::run(args).await,
    };

    if let Err(e) = result {
        eprintln!("vygr: error: {e}");
        std::process::exit(e.exit_code());
    }
}

/// The fully-built clap command tree for machine introspection (`vygr
/// schema`). Building propagates global args and enum value sets, so the
/// schema can never drift from the real interface.
pub(crate) fn cli_command() -> clap::Command {
    let mut cmd = Cli::command();
    cmd.build();
    cmd
}

fn init_tracing(verbose: u8) {
    let level = match verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(level));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("vygr/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("failed to build http client")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Machine contract (ADR-0008): every command that emits data accepts
    /// `--format`. Extend the list when adding a data-producing command.
    #[test]
    fn data_commands_accept_format_flag() {
        let cmd = Cli::command();
        let data_commands = [
            "search",
            "extract",
            "research",
            "providers",
            "models",
            "config",
            "runs",
        ];
        for name in data_commands {
            let sub = cmd
                .find_subcommand(name)
                .unwrap_or_else(|| panic!("missing subcommand `{name}`"));
            assert!(
                sub.get_arguments().any(|a| a.get_id() == "format"),
                "`vygr {name}` lost its --format flag (ADR-0008 machine contract)"
            );
        }
    }

    /// Docs that agents read must keep covering the full exit-code
    /// taxonomy of `VygrError::exit_code` (code `1` silently vanished
    /// from user-facing docs once already).
    #[test]
    fn documented_exit_codes_cover_the_contract() {
        const SKILL: &str = include_str!("../skill/SKILL.md");
        const README: &str = include_str!("../README.md");
        for (doc, name) in [(SKILL, "skill"), (README, "README")] {
            // Backticks are markdown dressing; strip them so the phrases
            // match either style.
            let normalized = doc.replace('`', "");
            for phrase in ["0 ok", "1 internal", "2 usage", "3 config", "4 provider"] {
                assert!(
                    normalized.contains(phrase),
                    "{name} exit-code docs missing '{phrase}' (ADR-0008 contract drift)"
                );
            }
        }
    }
}
