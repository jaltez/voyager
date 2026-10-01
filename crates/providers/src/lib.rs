//! Search and fetch providers, plus the chain / fan-out combinators that
//! give vygr its multi-provider behavior (ADR-0003).

mod brave;
mod cache;
mod ddgs;
mod exa;
mod fetch;
mod jina;
mod kagi;
mod searxng;
mod serper;
mod tavily;
mod throttle;

pub use cache::{CacheOptions, CacheStats, CachedSearchProvider, QueryClass};
pub use fetch::HttpFetch;
pub use throttle::{ThrottlePolicy, ThrottledProvider};

use std::collections::HashMap;
use std::sync::Arc;

use futures::future::join_all;
use vygr_core::config::SearchStackConf;
use vygr_core::provider::{SearchProvider, SearchQuery};
use vygr_core::types::SearchResult;
use vygr_core::VygrError;

/// Provider ids compiled into this binary.
pub const BUILTIN: &[&str] = &[
    "ddgs", "brave", "tavily", "searxng", "exa", "serper", "jina", "kagi",
];

/// Map an alias to its canonical provider id.
pub fn canonical(id: &str) -> Option<&'static str> {
    match id.trim().to_lowercase().as_str() {
        "ddgs" | "ddg" | "duckduckgo" => Some("ddgs"),
        "brave" => Some("brave"),
        "tavily" => Some("tavily"),
        "searxng" | "searx" => Some("searxng"),
        "exa" => Some("exa"),
        "serper" | "serperdev" => Some("serper"),
        "jina" | "s-jina" => Some("jina"),
        "kagi" => Some("kagi"),
        _ => None,
    }
}

/// Environment variable a provider needs, if any. `searxng` needs no key
/// but requires an instance URL instead (see `vygr providers`).
pub fn env_requirement(id: &str) -> Option<&'static str> {
    match canonical(id)? {
        "tavily" => Some("TAVILY_API_KEY"),
        "brave" => Some("BRAVE_API_KEY"),
        "exa" => Some("EXA_API_KEY"),
        "serper" => Some("SERPER_API_KEY"),
        "jina" => Some("JINA_API_KEY"),
        "kagi" => Some("KAGI_API_KEY"),
        _ => None,
    }
}

/// A built provider chain plus its shared cache statistics. The stack per
/// provider is `Cached(Throttled(Inner))` — hits cost neither spacing nor
/// retries (ADR-0010).
pub struct ChainHandle {
    pub providers: Vec<Box<dyn SearchProvider>>,
    pub cache: Arc<CacheStats>,
}

/// Build a fallback chain from a comma-separated spec such as `"ddgs,brave"`,
/// wrapped in politeness and caching decorators.
pub fn build_chain(
    spec: &str,
    http: reqwest::Client,
    stack: &SearchStackConf,
    cache_opts: &CacheOptions,
) -> Result<ChainHandle, VygrError> {
    let ids: Vec<&str> = spec
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if ids.is_empty() {
        return Err(VygrError::Config("empty provider chain".to_string()));
    }

    let stats = Arc::new(CacheStats::default());
    let cache_root = vygr_core::config::Config::cache_dir().map(|d| d.join("search"));
    let providers = ids
        .iter()
        .map(|id| {
            let inner: Box<dyn SearchProvider> = match canonical(id) {
                Some("ddgs") => Box::new(ddgs::DdgSearch::new(http.clone())),
                Some("brave") => Box::new(brave::BraveSearch::new(http.clone())),
                Some("tavily") => Box::new(tavily::TavilySearch::new(http.clone())),
                Some("exa") => Box::new(exa::ExaSearch::new(http.clone())),
                Some("serper") => Box::new(serper::SerperSearch::new(http.clone())),
                Some("jina") => Box::new(jina::JinaSearch::new(http.clone())),
                Some("kagi") => Box::new(kagi::KagiSearch::new(http.clone())),
                Some("searxng") => {
                    let configured = stack
                        .providers
                        .get("searxng")
                        .and_then(|p| p.base_url.as_deref());
                    let base = searxng::SearxngSearch::resolve_base(configured)?;
                    Box::new(searxng::SearxngSearch::new(http.clone(), base))
                }
                _ => {
                    return Err(VygrError::Config(format!(
                        "unknown provider '{}' (built-ins: {})",
                        id,
                        BUILTIN.join(", ")
                    )))
                }
            };
            let policy = ThrottlePolicy::from_config(inner.id(), &stack.politeness);
            let throttled =
                ThrottledProvider::new(inner, policy, vygr_core::config::Config::cache_dir());
            let cached = CachedSearchProvider::new(
                Box::new(throttled),
                cache_root.clone(),
                &stack.cache,
                cache_opts.clone(),
                Arc::clone(&stats),
            );
            Ok(Box::new(cached) as Box<dyn SearchProvider>)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ChainHandle {
        providers,
        cache: stats,
    })
}

/// Run the chain in order; the first provider with a non-empty result set
/// (after domain filters) wins, failures degrade to the next provider
/// (hsearch-style fallback).
pub async fn search_chain(
    chain: &[Box<dyn SearchProvider>],
    query: &SearchQuery,
) -> (Vec<SearchResult>, Vec<String>) {
    let mut warnings = Vec::new();
    for provider in chain {
        match provider.search(query).await {
            Ok(results) => {
                let filtered = filter_domains(results, query);
                if !filtered.is_empty() {
                    return (filtered, warnings);
                }
                warnings.push(format!(
                    "{}: no results after domain filters",
                    provider.id()
                ));
            }
            // VygrError::Provider already carries the provider id.
            Err(e) => warnings.push(e.to_string()),
        }
    }
    (Vec::new(), warnings)
}

/// Fan out to every provider in the chain concurrently and merge the
/// results, deduplicating by normalized URL (hsearch `--all` behavior).
pub async fn search_all(
    chain: &[Box<dyn SearchProvider>],
    query: &SearchQuery,
) -> (Vec<SearchResult>, Vec<String>) {
    let outcomes = join_all(chain.iter().map(|p| p.search(query))).await;
    let mut warnings = Vec::new();
    let mut all = Vec::new();
    for outcome in outcomes {
        match outcome {
            Ok(results) => all.extend(results),
            Err(e) => warnings.push(e.to_string()),
        }
    }
    (filter_domains(dedup(all), query), warnings)
}

/// Merge results that point at the same normalized URL, recording which
/// providers returned each one.
pub fn dedup(results: Vec<SearchResult>) -> Vec<SearchResult> {
    let mut out: Vec<SearchResult> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for r in results {
        let key = url_key(&r.url);
        match index.get(&key) {
            Some(&i) => {
                if !out[i].providers.contains(&r.provider) {
                    out[i].providers.push(r.provider);
                }
            }
            None => {
                index.insert(key, out.len());
                out.push(r);
            }
        }
    }
    out
}

/// Normalized identity of a URL for dedup across searches and levels
/// (scheme, `www.`, trailing slash and case stripped).
pub fn url_key(url: &str) -> String {
    let u = url.trim();
    let u = u
        .strip_prefix("https://")
        .or_else(|| u.strip_prefix("http://"))
        .unwrap_or(u);
    let u = u.strip_prefix("www.").unwrap_or(u);
    u.trim_end_matches('/').to_lowercase()
}

/// Host of a URL, normalized for domain matching (scheme, `www.` and port
/// stripped).
pub fn host_of(url: &str) -> String {
    let u = url.trim();
    let u = u
        .strip_prefix("https://")
        .or_else(|| u.strip_prefix("http://"))
        .unwrap_or(u);
    let u = u.strip_prefix("www.").unwrap_or(u);
    let end = u.find('/').unwrap_or(u.len());
    u[..end].split(':').next().unwrap_or("").to_lowercase()
}

fn domain_matches(host: &str, domain: &str) -> bool {
    host == domain || host.ends_with(&format!(".{domain}"))
}

/// Apply the query's include/exclude domain filters client-side (M1.3) —
/// uniformly across providers, including those with native support.
pub fn filter_domains(results: Vec<SearchResult>, query: &SearchQuery) -> Vec<SearchResult> {
    if query.include_domains.is_empty() && query.exclude_domains.is_empty() {
        return results;
    }
    results
        .into_iter()
        .filter(|r| {
            let host = host_of(&r.url);
            if host.is_empty() {
                return false;
            }
            if query
                .exclude_domains
                .iter()
                .any(|d| domain_matches(&host, d))
            {
                return false;
            }
            query.include_domains.is_empty()
                || query
                    .include_domains
                    .iter()
                    .any(|d| domain_matches(&host, d))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(provider: &str, url: &str) -> SearchResult {
        SearchResult {
            title: format!("t-{url}"),
            url: url.to_string(),
            snippet: String::new(),
            provider: provider.to_string(),
            providers: vec![provider.to_string()],
            content: None,
        }
    }

    #[test]
    fn dedup_merges_equivalent_urls() {
        let merged = dedup(vec![
            hit("ddgs", "https://www.Example.com/a/"),
            hit("brave", "http://example.com/a"),
            hit("brave", "https://example.com/b"),
        ]);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].providers, vec!["ddgs", "brave"]);
    }

    #[test]
    fn host_extraction_normalizes() {
        assert_eq!(
            host_of("https://Blog.Rust-Lang.org/x?y=1"),
            "blog.rust-lang.org"
        );
        assert_eq!(host_of("http://www.Example.com:8080/a"), "example.com");
        assert_eq!(host_of("https://a.io"), "a.io");
    }

    #[test]
    fn domain_filters_allowlist_and_blocklist() {
        let results = vec![
            hit("ddgs", "https://doc.rust-lang.org/std/"),
            hit("ddgs", "https://blog.rust-lang.org/inside-rust"),
            hit("ddgs", "https://ziglang.org/news"),
        ];
        let allow =
            SearchQuery::new("q", 5).with_filters(None, vec!["rust-lang.org".into()], vec![]);
        let kept = filter_domains(results.clone(), &allow);
        assert_eq!(kept.len(), 2);

        let block =
            SearchQuery::new("q", 5).with_filters(None, vec![], vec!["blog.rust-lang.org".into()]);
        let kept = filter_domains(results.clone(), &block);
        assert_eq!(kept.len(), 2);
        assert!(kept.iter().all(|r| !r.url.contains("blog.")));

        // No filters configured: everything passes through untouched.
        let plain = SearchQuery::new("q", 5);
        assert_eq!(filter_domains(results, &plain).len(), 3);
    }
}
