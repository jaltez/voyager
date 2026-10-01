//! Exa neural/keyword search backend (requires `EXA_API_KEY`).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use vygr_core::provider::{SearchProvider, SearchQuery};
use vygr_core::types::{truncate_chars, SearchResult};
use vygr_core::VygrError;

const ENDPOINT: &str = "https://api.exa.ai/search";
const ENV: &str = "EXA_API_KEY";

pub struct ExaSearch {
    http: reqwest::Client,
}

impl ExaSearch {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }
}

#[derive(Serialize)]
struct ReqBody {
    query: String,
    num_results: usize,
    #[serde(rename = "type")]
    kind: &'static str,
}

#[async_trait]
impl SearchProvider for ExaSearch {
    fn id(&self) -> &'static str {
        "exa"
    }

    fn requires_env(&self) -> Option<&'static str> {
        Some(ENV)
    }

    async fn search(&self, q: &SearchQuery) -> Result<Vec<SearchResult>, VygrError> {
        let key = std::env::var(ENV)
            .ok()
            .filter(|k| !k.trim().is_empty())
            .ok_or_else(|| VygrError::Auth(format!("set {ENV} to use the exa provider")))?;

        let resp = self
            .http
            .post(ENDPOINT)
            .header("x-api-key", &key)
            .json(&ReqBody {
                query: q.query.clone(),
                num_results: q.max_results.min(20),
                kind: "auto",
            })
            .send()
            .await
            .map_err(|e| VygrError::Network(e.to_string()))?;
        let status = resp.status();
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Err(VygrError::Auth(format!(
                "exa rejected the API key (HTTP {status})"
            )));
        }
        if !status.is_success() {
            return Err(VygrError::provider_status(
                "exa",
                format!("HTTP {status}"),
                status.as_u16(),
            ));
        }
        let body: ExaResponse = resp
            .json()
            .await
            .map_err(|e| VygrError::Parse(format!("exa response: {e}")))?;
        Ok(body
            .results
            .into_iter()
            .take(q.max_results)
            .map(|r| SearchResult {
                title: r.title,
                url: r.url,
                snippet: truncate_chars(&r.text, 400),
                provider: "exa".into(),
                providers: vec!["exa".into()],
                content: None,
            })
            .collect())
    }
}

#[derive(Debug, Deserialize)]
pub struct ExaResponse {
    #[serde(default)]
    pub results: Vec<ExaItem>,
}

#[derive(Debug, Deserialize)]
pub struct ExaItem {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub text: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/exa_sample.json");

    #[test]
    fn parses_exa_json() {
        let body: ExaResponse = serde_json::from_str(FIXTURE).unwrap();
        assert_eq!(body.results.len(), 2);
        assert_eq!(body.results[0].url, "https://exa.io/neural-search");
        assert!(body.results[1].text.to_lowercase().contains("keyword"));
    }
}
