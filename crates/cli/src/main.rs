//! vygr: the voyager deep research CLI.

mod commands;
mod mcp;
mod output;

use clap::{Parser, Subcommand};
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
    /// Run as an MCP server over stdio (search, extract, research, get_artifact)
    Serve,
    /// Self-update from crates.io (`--check` only reports)
    Update(commands::update::Args),
    /// Inspect past research runs; re-synthesize offline
    Runs(commands::runs::Args),
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
        Commands::Serve => mcp::serve(http).await,
        Commands::Update(args) => commands::update::run(args, http).await,
        Commands::Runs(args) => commands::runs::run(args, http, &cfg).await,
    };

    if let Err(e) = result {
        eprintln!("vygr: error: {e}");
        std::process::exit(e.exit_code());
    }
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
