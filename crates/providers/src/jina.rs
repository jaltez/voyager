//! Jina s.jina.ai search backend (requires `JINA_API_KEY`). The endpoint
//! answers with markdown rather than JSON, so parsing is best-effort and
//! pinned by a fixture.

use async_trait::async_trait;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use vygr_core::provider::{SearchProvider, SearchQuery};
use vygr_core::types::SearchResult;
use vygr_core::VygrError;

const ENDPOINT: &str = "https://s.jina.ai/";
const ENV: &str = "JINA_API_KEY";

pub struct JinaSearch {
    http: reqwest::Client,
}

impl JinaSearch {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }
}

#[async_trait]
impl SearchProvider for JinaSearch {
    fn id(&self) -> &'static str {
        "jina"
    }

    fn requires_env(&self) -> Option<&'static str> {
        Some(ENV)
    }

    async fn search(&self, q: &SearchQuery) -> Result<Vec<SearchResult>, VygrError> {
        let key = std::env::var(ENV)
            .ok()
            .filter(|k| !k.trim().is_empty())
            .ok_or_else(|| VygrError::Auth(format!("set {ENV} to use the jina provider")))?;

        let url = format!(
            "{ENDPOINT}{}",
            utf8_percent_encode(&q.query, NON_ALPHANUMERIC)
        );
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&key)
            .header(reqwest::header::ACCEPT, "text/plain")
            .send()
            .await
            .map_err(|e| VygrError::Network(e.to_string()))?;
        let status = resp.status();
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Err(VygrError::Auth(format!(
                "jina rejected the API key (HTTP {status})"
            )));
        }
        if !status.is_success() {
            return Err(VygrError::provider_status(
                "jina",
                format!("HTTP {status}"),
                status.as_u16(),
            ));
        }
        let body = resp
            .text()
            .await
            .map_err(|e| VygrError::Network(e.to_string()))?;
        let mut results = parse_jina_markdown(&body);
        results.truncate(q.max_results);
        Ok(results)
    }
}

/// Best-effort parser for s.jina.ai's markdown answer: records start at a
/// `Title:` line, carry a `URL Source:` line, and collect up to two
/// following content lines as the snippet.
pub fn parse_jina_markdown(raw: &str) -> Vec<SearchResult> {
    fn flush(
        title: &mut Option<String>,
        url: &mut Option<String>,
        snippet: &mut Vec<String>,
        out: &mut Vec<SearchResult>,
    ) {
        if let (Some(t), Some(u)) = (title.take(), url.take()) {
            if u.starts_with("http") {
                out.push(SearchResult {
                    title: t,
                    url: u,
                    snippet: snippet.join(" "),
                    provider: "jina".into(),
                    providers: vec!["jina".into()],
                    content: None,
                });
            }
        }
        snippet.clear();
    }

    let mut out = Vec::new();
    let mut title: Option<String> = None;
    let mut url: Option<String> = None;
    let mut snippet: Vec<String> = Vec::new();

    for line in raw.lines() {
        let line = line.trim_end();
        if let Some(t) = line.strip_prefix("Title: ") {
            flush(&mut title, &mut url, &mut snippet, &mut out);
            title = Some(t.trim().to_string());
        } else if let Some(u) = line.strip_prefix("URL Source: ") {
            url = Some(u.trim().to_string());
        } else if line.starts_with("Published Time:")
            || line.starts_with("Markdown Content:")
            || line.starts_with("Warning:")
            || line.trim().is_empty()
        {
            // metadata or separators — never snippet material
        } else if title.is_some() && url.is_some() && snippet.len() < 2 {
            snippet.push(line.trim().to_string());
        }
    }
    flush(&mut title, &mut url, &mut snippet, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/jina_sample.md");

    #[test]
    fn parses_jina_markdown() {
        let results = parse_jina_markdown(FIXTURE);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Jina AI Search Foundation");
        assert_eq!(results[0].url, "https://jina.ai/news/foundation");
        assert!(results[0].snippet.contains("Search foundation APIs"));
        assert_eq!(results[1].url, "https://docs.jina.ai");
    }

    #[test]
    fn ignores_non_http_sources() {
        let results = parse_jina_markdown("Title: X\nURL Source: not-a-url\nbody line\n");
        assert!(results.is_empty());
    }
}
