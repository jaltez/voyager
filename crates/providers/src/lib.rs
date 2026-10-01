//! Search and fetch providers, plus the chain / fan-out combinators that
//! give vygr its multi-provider behavior (ADR-0003).

mod brave;
mod ddgs;
mod fetch;
mod tavily;

pub use fetch::HttpFetch;

use std::collections::HashMap;

use futures::future::join_all;
use vygr_core::provider::{SearchProvider, SearchQuery};
use vygr_core::types::SearchResult;
use vygr_core::VygrError;

/// Provider ids compiled into this binary.
pub const BUILTIN: &[&str] = &["ddgs", "brave", "tavily"];

/// Map an alias to its canonical provider id.
pub fn canonical(id: &str) -> Option<&'static str> {
    match id.trim().to_lowercase().as_str() {
        "ddgs" | "ddg" | "duckduckgo" => Some("ddgs"),
        "brave" => Some("brave"),
        "tavily" => Some("tavily"),
        _ => None,
    }
}

/// Environment variable a provider needs, if any.
pub fn env_requirement(id: &str) -> Option<&'static str> {
    match canonical(id)? {
        "tavily" => Some("TAVILY_API_KEY"),
        "brave" => Some("BRAVE_API_KEY"),
        _ => None,
    }
}

pub fn build(id: &str, http: reqwest::Client) -> Result<Box<dyn SearchProvider>, VygrError> {
    match canonical(id) {
        Some("ddgs") => Ok(Box::new(ddgs::DdgSearch::new(http))),
        Some("brave") => Ok(Box::new(brave::BraveSearch::new(http))),
        Some("tavily") => Ok(Box::new(tavily::TavilySearch::new(http))),
        _ => Err(VygrError::Config(format!(
            "unknown provider '{}' (built-ins: {})",
            id,
            BUILTIN.join(", ")
        ))),
    }
}

/// Build a fallback chain from a comma-separated spec such as `"ddgs,brave"`.
pub fn build_chain(
    spec: &str,
    http: reqwest::Client,
) -> Result<Vec<Box<dyn SearchProvider>>, VygrError> {
    let ids: Vec<&str> = spec
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if ids.is_empty() {
        return Err(VygrError::Config("empty provider chain".to_string()));
    }
    ids.iter()
        .map(|id| build(id, http.clone()))
        .collect::<Result<Vec<_>, _>>()
}

/// Run the chain in order; the first provider with a non-empty result set
/// wins, failures degrade to the next provider (hsearch-style fallback).
pub async fn search_chain(
    chain: &[Box<dyn SearchProvider>],
    query: &SearchQuery,
) -> (Vec<SearchResult>, Vec<String>) {
    let mut warnings = Vec::new();
    for provider in chain {
        match provider.search(query).await {
            Ok(results) if !results.is_empty() => return (results, warnings),
            Ok(_) => warnings.push(format!("{}: no results", provider.id())),
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
    (dedup(all), warnings)
}

/// Merge results that point at the same normalized URL, recording which
/// providers returned each one.
pub fn dedup(results: Vec<SearchResult>) -> Vec<SearchResult> {
    let mut out: Vec<SearchResult> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for r in results {
        let key = normalize_url(&r.url);
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

fn normalize_url(url: &str) -> String {
    let u = url.trim();
    let u = u
        .strip_prefix("https://")
        .or_else(|| u.strip_prefix("http://"))
        .unwrap_or(u);
    let u = u.strip_prefix("www.").unwrap_or(u);
    u.trim_end_matches('/').to_lowercase()
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
}
