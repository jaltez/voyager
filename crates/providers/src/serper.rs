//! Serper.dev (Google SERP) backend (requires `SERPER_API_KEY`).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use vygr_core::provider::{SearchProvider, SearchQuery};
use vygr_core::types::SearchResult;
use vygr_core::VygrError;

const ENDPOINT: &str = "https://google.serper.dev/search";
const ENV: &str = "SERPER_API_KEY";

pub struct SerperSearch {
    http: reqwest::Client,
}

impl SerperSearch {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }
}

#[derive(Serialize)]
struct ReqBody {
    q: String,
    num: usize,
}

#[async_trait]
impl SearchProvider for SerperSearch {
    fn id(&self) -> &'static str {
        "serper"
    }

    fn requires_env(&self) -> Option<&'static str> {
        Some(ENV)
    }

    async fn search(&self, q: &SearchQuery) -> Result<Vec<SearchResult>, VygrError> {
        let key = std::env::var(ENV)
            .ok()
            .filter(|k| !k.trim().is_empty())
            .ok_or_else(|| VygrError::Auth(format!("set {ENV} to use the serper provider")))?;

        let resp = self
            .http
            .post(ENDPOINT)
            .header("X-API-KEY", &key)
            .json(&ReqBody {
                q: q.query.clone(),
                num: q.max_results.min(20),
            })
            .send()
            .await
            .map_err(|e| VygrError::Network(e.to_string()))?;
        let status = resp.status();
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Err(VygrError::Auth(format!(
                "serper rejected the API key (HTTP {status})"
            )));
        }
        if !status.is_success() {
            return Err(VygrError::provider_status(
                "serper",
                format!("HTTP {status}"),
                status.as_u16(),
            ));
        }
        let body: SerperResponse = resp
            .json()
            .await
            .map_err(|e| VygrError::Parse(format!("serper response: {e}")))?;
        Ok(body
            .organic
            .into_iter()
            .take(q.max_results)
            .map(|r| SearchResult {
                title: r.title,
                url: r.link,
                snippet: r.snippet,
                provider: "serper".into(),
                providers: vec!["serper".into()],
                content: None,
            })
            .collect())
    }
}

#[derive(Debug, Deserialize)]
pub struct SerperResponse {
    #[serde(default)]
    pub organic: Vec<SerperItem>,
}

#[derive(Debug, Deserialize)]
pub struct SerperItem {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub link: String,
    #[serde(default)]
    pub snippet: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/serper_sample.json");

    #[test]
    fn parses_serper_json() {
        let body: SerperResponse = serde_json::from_str(FIXTURE).unwrap();
        assert_eq!(body.organic.len(), 2);
        assert_eq!(body.organic[0].link, "https://serper.dev/results");
        assert!(body.organic[1].snippet.contains("playground"));
    }
}
