//! Brave Search API backend (requires `BRAVE_API_KEY`).

use async_trait::async_trait;
use serde::Deserialize;
use vygr_core::provider::{SearchProvider, SearchQuery};
use vygr_core::types::SearchResult;
use vygr_core::VygrError;

const ENDPOINT: &str = "https://api.search.brave.com/res/v1/web/search";
const ENV: &str = "BRAVE_API_KEY";

pub struct BraveSearch {
    http: reqwest::Client,
}

impl BraveSearch {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }
}

#[async_trait]
impl SearchProvider for BraveSearch {
    fn id(&self) -> &'static str {
        "brave"
    }

    fn requires_env(&self) -> Option<&'static str> {
        Some(ENV)
    }

    async fn search(&self, q: &SearchQuery) -> Result<Vec<SearchResult>, VygrError> {
        let key = std::env::var(ENV)
            .ok()
            .filter(|k| !k.trim().is_empty())
            .ok_or_else(|| VygrError::Auth(format!("set {ENV} to use the brave provider")))?;

        let mut params: Vec<(&str, String)> = vec![
            ("q", q.query.clone()),
            ("count", q.max_results.min(20).to_string()),
        ];
        if let Some(range) = q.time_range {
            params.push(("freshness", range.brave_param().to_string()));
        }
        let resp = self
            .http
            .get(ENDPOINT)
            .query(&params)
            .header("X-Subscription-Token", &key)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|e| VygrError::Network(e.to_string()))?;
        let status = resp.status();
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Err(VygrError::Auth(format!(
                "brave rejected the API key (HTTP {status})"
            )));
        }
        if !status.is_success() {
            return Err(VygrError::provider_status(
                "brave",
                format!("HTTP {status}"),
                status.as_u16(),
            ));
        }
        let body: BraveResponse = resp
            .json()
            .await
            .map_err(|e| VygrError::Parse(format!("brave response: {e}")))?;

        Ok(body
            .web
            .map(|w| w.results)
            .unwrap_or_default()
            .into_iter()
            .take(q.max_results)
            .map(|r| SearchResult {
                title: r.title,
                url: r.url,
                snippet: r.description,
                provider: "brave".into(),
                providers: vec!["brave".into()],
                content: None,
            })
            .collect())
    }
}

#[derive(Debug, Deserialize)]
struct BraveResponse {
    web: Option<BraveWeb>,
}

#[derive(Debug, Deserialize)]
struct BraveWeb {
    #[serde(default)]
    results: Vec<BraveItem>,
}

#[derive(Debug, Deserialize)]
struct BraveItem {
    title: String,
    url: String,
    #[serde(default)]
    description: String,
}
