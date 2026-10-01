//! Kagi search backend (requires `KAGI_API_KEY`).

use async_trait::async_trait;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde::Deserialize;
use vygr_core::provider::{SearchProvider, SearchQuery};
use vygr_core::types::SearchResult;
use vygr_core::VygrError;

const ENDPOINT: &str = "https://kagi.com/api/v0/search";
const ENV: &str = "KAGI_API_KEY";

pub struct KagiSearch {
    http: reqwest::Client,
}

impl KagiSearch {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }
}

#[async_trait]
impl SearchProvider for KagiSearch {
    fn id(&self) -> &'static str {
        "kagi"
    }

    fn requires_env(&self) -> Option<&'static str> {
        Some(ENV)
    }

    async fn search(&self, q: &SearchQuery) -> Result<Vec<SearchResult>, VygrError> {
        let key = std::env::var(ENV)
            .ok()
            .filter(|k| !k.trim().is_empty())
            .ok_or_else(|| VygrError::Auth(format!("set {ENV} to use the kagi provider")))?;

        let url = format!(
            "{ENDPOINT}?q={}&limit={}",
            utf8_percent_encode(&q.query, NON_ALPHANUMERIC),
            q.max_results.min(20)
        );
        let resp = self
            .http
            .get(&url)
            .header(reqwest::header::AUTHORIZATION, format!("Token {key}"))
            .send()
            .await
            .map_err(|e| VygrError::Network(e.to_string()))?;
        let status = resp.status();
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Err(VygrError::Auth(format!(
                "kagi rejected the API key (HTTP {status})"
            )));
        }
        if !status.is_success() {
            return Err(VygrError::provider_status(
                "kagi",
                format!("HTTP {status}"),
                status.as_u16(),
            ));
        }
        let body: KagiResponse = resp
            .json()
            .await
            .map_err(|e| VygrError::Parse(format!("kagi response: {e}")))?;
        Ok(body
            .data
            .into_iter()
            .filter(|r| r.url.starts_with("http"))
            .take(q.max_results)
            .map(|r| SearchResult {
                title: r.t,
                url: r.url,
                snippet: r.d,
                provider: "kagi".into(),
                providers: vec!["kagi".into()],
                content: None,
            })
            .collect())
    }
}

#[derive(Debug, Deserialize)]
pub struct KagiResponse {
    #[serde(default)]
    pub data: Vec<KagiItem>,
}

#[derive(Debug, Deserialize)]
pub struct KagiItem {
    #[serde(default)]
    pub t: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub d: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/kagi_sample.json");

    #[test]
    fn parses_kagi_json() {
        let body: KagiResponse = serde_json::from_str(FIXTURE).unwrap();
        assert_eq!(body.data.len(), 2);
        assert_eq!(body.data[0].url, "https://kagi.com/pricing");
        assert!(body.data[1].d.contains("orb"));
    }
}
