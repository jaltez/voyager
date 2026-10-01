//! Self-hosted SearXNG metasearch backend (keyless; needs an instance URL
//! via `[providers.searxng] base_url` or `SEARXNG_BASE_URL`, with the
//! instance's `json` format enabled).

use async_trait::async_trait;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde::Deserialize;
use vygr_core::provider::{SearchProvider, SearchQuery};
use vygr_core::types::SearchResult;
use vygr_core::VygrError;

const ENV_BASE: &str = "SEARXNG_BASE_URL";

pub struct SearxngSearch {
    http: reqwest::Client,
    base_url: String,
}

impl SearxngSearch {
    pub fn new(http: reqwest::Client, base_url: String) -> Self {
        Self { http, base_url }
    }

    /// Resolve the instance URL from config or the environment.
    pub fn resolve_base(configured: Option<&str>) -> Result<String, VygrError> {
        configured
            .map(str::to_string)
            .or_else(|| {
                std::env::var(ENV_BASE)
                    .ok()
                    .filter(|v| !v.trim().is_empty())
            })
            .ok_or_else(|| {
                VygrError::Config(format!(
                    "searxng needs an instance URL: set [providers.searxng] base_url or {ENV_BASE}"
                ))
            })
    }
}

#[async_trait]
impl SearchProvider for SearxngSearch {
    fn id(&self) -> &'static str {
        "searxng"
    }

    async fn search(&self, q: &SearchQuery) -> Result<Vec<SearchResult>, VygrError> {
        let mut url = format!(
            "{}/search?q={}&format=json",
            self.base_url.trim_end_matches('/'),
            utf8_percent_encode(&q.query, NON_ALPHANUMERIC)
        );
        if let Some(range) = q.time_range {
            // SearXNG shares Tavily's day|week|month|year vocabulary.
            url.push_str("&time_range=");
            url.push_str(range.tavily_param());
        }
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| VygrError::Network(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(VygrError::provider_status(
                "searxng",
                format!("HTTP {status} (is the 'json' format enabled on the instance?)"),
                status.as_u16(),
            ));
        }
        let body: SearxngResponse = resp
            .json()
            .await
            .map_err(|e| VygrError::Parse(format!("searxng response: {e}")))?;
        Ok(body
            .results
            .into_iter()
            .filter(|r| r.url.starts_with("http"))
            .take(q.max_results)
            .map(|r| SearchResult {
                title: r.title,
                url: r.url,
                snippet: r.content,
                provider: "searxng".into(),
                providers: vec!["searxng".into()],
                content: None,
            })
            .collect())
    }
}

#[derive(Debug, Deserialize)]
pub struct SearxngResponse {
    #[serde(default)]
    pub results: Vec<SearxngItem>,
}

#[derive(Debug, Deserialize)]
pub struct SearxngItem {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub content: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/searxng_sample.json");

    #[test]
    fn parses_searxng_json() {
        let body: SearxngResponse = serde_json::from_str(FIXTURE).unwrap();
        assert_eq!(body.results.len(), 2);
        assert_eq!(body.results[0].url, "https://example.org/rust");
        assert_eq!(body.results[1].title, "SearXNG docs");
    }
}
