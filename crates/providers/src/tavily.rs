//! Tavily search API backend (requires `TAVILY_API_KEY`).

use async_trait::async_trait;
use serde::Deserialize;
use vygr_core::provider::{SearchProvider, SearchQuery};
use vygr_core::types::SearchResult;
use vygr_core::VygrError;

const ENDPOINT: &str = "https://api.tavily.com/search";
const ENV: &str = "TAVILY_API_KEY";

pub struct TavilySearch {
    http: reqwest::Client,
}

impl TavilySearch {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }
}

#[async_trait]
impl SearchProvider for TavilySearch {
    fn id(&self) -> &'static str {
        "tavily"
    }

    fn requires_env(&self) -> Option<&'static str> {
        Some(ENV)
    }

    async fn search(&self, q: &SearchQuery) -> Result<Vec<SearchResult>, VygrError> {
        let key = std::env::var(ENV)
            .ok()
            .filter(|k| !k.trim().is_empty())
            .ok_or_else(|| VygrError::Auth(format!("set {ENV} to use the tavily provider")))?;

        let resp = self
            .http
            .post(ENDPOINT)
            .bearer_auth(&key)
            .json(&request_body(q))
            .send()
            .await
            .map_err(|e| VygrError::Network(e.to_string()))?;
        let status = resp.status();
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Err(VygrError::Auth(format!(
                "tavily rejected the API key (HTTP {status})"
            )));
        }
        if !status.is_success() {
            return Err(VygrError::provider_status(
                "tavily",
                format!("HTTP {status}"),
                status.as_u16(),
            ));
        }
        let body: TavilyResponse = resp
            .json()
            .await
            .map_err(|e| VygrError::Parse(format!("tavily response: {e}")))?;

        Ok(body
            .results
            .into_iter()
            .take(q.max_results)
            .map(|r| SearchResult {
                title: r.title,
                url: r.url,
                snippet: r.content,
                provider: "tavily".into(),
                providers: vec!["tavily".into()],
                content: None,
            })
            .collect())
    }
}

/// JSON body for the search endpoint, including the native `time_range`
/// freshness filter when requested.
fn request_body(q: &SearchQuery) -> serde_json::Value {
    let mut body = serde_json::json!({
        "query": q.query,
        "max_results": q.max_results.min(20),
        "search_depth": "basic",
    });
    if let Some(range) = q.time_range {
        body["time_range"] = serde_json::json!(range.tavily_param());
    }
    body
}

#[derive(Debug, Deserialize)]
struct TavilyResponse {
    #[serde(default)]
    results: Vec<TavilyItem>,
}

#[derive(Debug, Deserialize)]
struct TavilyItem {
    title: String,
    url: String,
    content: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use vygr_core::provider::TimeRange;

    #[test]
    fn request_body_includes_freshness_only_when_requested() {
        let plain = request_body(&SearchQuery::new("rust", 5));
        assert!(plain.get("time_range").is_none());
        assert_eq!(plain["query"], "rust");

        let fresh = request_body(&SearchQuery::new("rust", 5).with_filters(
            Some(TimeRange::Week),
            vec![],
            vec![],
        ));
        assert_eq!(fresh["time_range"], "week");
    }
}
