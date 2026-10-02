//! Semantic Scholar provider (keyless with rate limits; `S2_API_KEY`
//! raises them): CS-focused paper index with clean abstracts.

use async_trait::async_trait;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde::Deserialize;
use vygr_core::provider::{SearchProvider, SearchQuery};
use vygr_core::types::{truncate_chars, SearchResult};
use vygr_core::VygrError;

const ENDPOINT: &str = "https://api.semanticscholar.org/graph/v1/paper/search";
const ENV: &str = "S2_API_KEY";

pub struct SemanticScholarSearch {
    http: reqwest::Client,
}

impl SemanticScholarSearch {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }
}

#[async_trait]
impl SearchProvider for SemanticScholarSearch {
    fn id(&self) -> &'static str {
        "s2"
    }

    async fn search(&self, q: &SearchQuery) -> Result<Vec<SearchResult>, VygrError> {
        let url = format!(
            "{ENDPOINT}?query={}&limit={}&fields=title,url,abstract",
            utf8_percent_encode(&q.query, NON_ALPHANUMERIC),
            q.max_results.min(20)
        );
        let mut request = self.http.get(&url);
        if let Ok(key) = std::env::var(ENV) {
            if !key.trim().is_empty() {
                request = request.header("x-api-key", key.trim());
            }
        }
        let resp = request
            .send()
            .await
            .map_err(|e| VygrError::Network(e.to_string()))?;
        let status = resp.status();
        if status.as_u16() == 429 {
            return Err(VygrError::provider_status(
                "s2",
                "rate limited (keyless pool); set S2_API_KEY or retry later".to_string(),
                429,
            ));
        }
        if !status.is_success() {
            return Err(VygrError::provider_status(
                "s2",
                format!("HTTP {status}"),
                status.as_u16(),
            ));
        }
        let body: S2Response = resp
            .json()
            .await
            .map_err(|e| VygrError::Parse(format!("s2 response: {e}")))?;
        Ok(body
            .data
            .into_iter()
            .filter(|p| p.url.starts_with("http"))
            .take(q.max_results)
            .map(|p| SearchResult {
                title: p.title,
                url: p.url,
                snippet: truncate_chars(&p.paper_abstract.unwrap_or_default(), 400),
                provider: "s2".into(),
                providers: vec!["s2".into()],
                content: None,
            })
            .collect())
    }
}

#[derive(Debug, Deserialize)]
pub struct S2Response {
    #[serde(default)]
    pub data: Vec<S2Paper>,
}

#[derive(Debug, Deserialize)]
pub struct S2Paper {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub url: String,
    #[serde(default, rename = "abstract")]
    pub paper_abstract: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/s2_sample.json");

    #[test]
    fn parses_s2_json() {
        let body: S2Response = serde_json::from_str(FIXTURE).unwrap();
        assert_eq!(body.data.len(), 2);
        assert_eq!(body.data[0].url, "https://www.semanticscholar.org/paper/1");
        assert!(body.data[1].paper_abstract.is_none());
    }
}
