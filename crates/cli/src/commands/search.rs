//! `vygr search`: provider-chain web search with optional content inlining.

use clap::Args as ClapArgs;
use futures::future::join_all;
use vygr_core::config::Config;
use vygr_core::provider::{FetchProvider, SearchQuery};
use vygr_core::VygrError;

use crate::commands::{parse_domain_list, read_query};
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

    /// Bypass the disk cache for this query
    #[arg(long)]
    pub no_cache: bool,

    /// Force this TTL (seconds) for every query class
    #[arg(long)]
    pub cache_ttl: Option<u64>,

    /// Restrict results by freshness: day|week|month|year (where supported)
    #[arg(long)]
    pub time_range: Option<String>,

    /// Comma-separated domain allowlist (empty = allow all)
    #[arg(long)]
    pub include_domains: Option<String>,

    /// Comma-separated domain blocklist
    #[arg(long)]
    pub exclude_domains: Option<String>,
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

    let cache_opts = vygr_providers::CacheOptions {
        disabled: args.no_cache,
        force_ttl: args.cache_ttl.map(std::time::Duration::from_secs),
    };
    let stack = vygr_core::config::SearchStackConf::from_config(cfg);
    let handle = vygr_providers::build_chain(&chain_spec, http.clone(), &stack, &cache_opts)?;
    let q = SearchQuery::new(query.clone(), args.max_results).with_filters(
        match args.time_range.as_deref() {
            Some(s) => Some(vygr_core::provider::TimeRange::parse(s)?),
            None => None,
        },
        parse_domain_list(args.include_domains.as_deref()),
        parse_domain_list(args.exclude_domains.as_deref()),
    );
    let (mut results, warnings) = if args.all {
        vygr_providers::search_all(&handle.providers, &q).await
    } else {
        vygr_providers::search_chain(&handle.providers, &q).await
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
        return Err(VygrError::provider(chain_spec, "no results"));
    }

    let (cache_hits, cache_misses) = handle.cache.snapshot();
    let meta = serde_json::json!({ "cache": { "hits": cache_hits, "misses": cache_misses } });
    println!(
        "{}",
        render_search(args.format, &query, &chain_spec, &results, &meta)
    );
    Ok(())
}
