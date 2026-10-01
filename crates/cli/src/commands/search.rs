//! `vygr search` — provider-chain web search with optional content inlining.

use clap::Args as ClapArgs;
use futures::future::join_all;
use vygr_core::config::Config;
use vygr_core::provider::{FetchProvider, SearchQuery};
use vygr_core::VygrError;

use crate::commands::read_query;
use crate::output::{render_search, SearchFormat};

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Search query ("-" reads it from stdin)
    pub query: String,

    /// Provider or comma-separated fallback chain, e.g. "ddgs,brave"
    #[arg(long)]
    pub provider: Option<String>,

    /// Fan out to all providers with satisfied credentials, concurrently
    #[arg(long)]
    pub all: bool,

    /// Maximum results per query
    #[arg(long, default_value_t = 5)]
    pub max_results: usize,

    /// Inline the content of the top N results into the output
    #[arg(long, default_value_t = 0)]
    pub extract_top: usize,

    #[arg(long, value_enum, default_value = "table")]
    pub format: SearchFormat,
}

pub async fn run(args: Args, http: reqwest::Client, cfg: &Config) -> Result<(), VygrError> {
    let query = read_query(&args.query)?;

    let chain_spec = if args.all {
        vygr_providers::BUILTIN
            .iter()
            .filter(|id| match vygr_providers::env_requirement(id) {
                Some(env) => std::env::var(env)
                    .map(|v| !v.trim().is_empty())
                    .unwrap_or(false),
                None => true,
            })
            .cloned()
            .collect::<Vec<_>>()
            .join(",")
    } else {
        args.provider
            .clone()
            .or_else(|| cfg.default_provider.clone())
            .unwrap_or_else(|| "ddgs".to_string())
    };

    let chain = vygr_providers::build_chain(&chain_spec, http.clone())?;
    let q = SearchQuery::new(query.clone(), args.max_results);
    let (mut results, warnings) = if args.all {
        vygr_providers::search_all(&chain, &q).await
    } else {
        vygr_providers::search_chain(&chain, &q).await
    };
    for w in &warnings {
        tracing::warn!("{w}");
    }

    if args.extract_top > 0 {
        let fetcher = vygr_providers::HttpFetch::new(http);
        let indexed: Vec<usize> = (0..results.len().min(args.extract_top)).collect();
        let pages = join_all(
            indexed
                .iter()
                .map(|i| fetcher.fetch(&results[*i].url, 12_000)),
        )
        .await;
        for (i, page) in indexed.into_iter().zip(pages) {
            match page {
                Ok(p) => results[i].content = Some(p.truncated(12_000)),
                Err(e) => tracing::warn!("extract {}: {e}", results[i].url),
            }
        }
        results.truncate(args.extract_top);
    }

    if results.is_empty() {
        for w in &warnings {
            eprintln!("vygr: {w}");
        }
        return Err(VygrError::Provider {
            provider: chain_spec,
            message: "no results".to_string(),
        });
    }

    println!(
        "{}",
        render_search(args.format, &query, &chain_spec, &results)
    );
    Ok(())
}
