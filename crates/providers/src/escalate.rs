//! Fetch escalation ladder (T1, Web Forager lesson): plain HTTP first;
//! when the result fails or comes back too thin to be real content
//! (JS-rendered page, bot wall), retry through the Jina reader
//! (`https://r.jina.ai/<url>`), which renders pages server-side and
//! answers with markdown. The reader works keyless under rate limits;
//! `JINA_API_KEY` raises them.

use async_trait::async_trait;
use vygr_core::provider::FetchProvider;
use vygr_core::types::{truncate_chars, FetchPage};
use vygr_core::VygrError;

use crate::fetch::HttpFetch;

const READER_BASE: &str = "https://r.jina.ai/";
const ENV: &str = "JINA_API_KEY";

/// Below this many extracted characters an HTML page is considered thin
/// and worth a reader retry.
const THIN_HTML_CHARS: usize = 400;

pub struct EscalatingFetch {
    inner: HttpFetch,
    http: reqwest::Client,
    /// Set to false to disable the reader retry entirely.
    pub escalate: bool,
}

impl EscalatingFetch {
    pub fn new(inner: HttpFetch, http: reqwest::Client) -> Self {
        Self {
            inner,
            http,
            escalate: true,
        }
    }
}

/// Whether a plain-fetch result warrants a reader retry: hard failures,
/// thin HTML, or empty text.
pub fn needs_escalation(page: Option<&FetchPage>) -> bool {
    match page {
        None => true,
        Some(p) => {
            (p.content_type.contains("html") && p.text.trim().len() < THIN_HTML_CHARS)
                || p.text.trim().is_empty()
        }
    }
}

#[async_trait]
impl FetchProvider for EscalatingFetch {
    async fn fetch(&self, url: &str, max_chars: usize) -> Result<FetchPage, VygrError> {
        let plain = self.inner.fetch(url, max_chars).await;
        if !self.escalate || !needs_escalation(plain.as_ref().ok()) {
            return plain;
        }

        let mut request = self
            .http
            .get(format!("{READER_BASE}{url}"))
            .header(reqwest::header::ACCEPT, "text/plain")
            .timeout(std::time::Duration::from_secs(60));
        if let Ok(key) = std::env::var(ENV) {
            if !key.trim().is_empty() {
                request = request.bearer_auth(key.trim());
            }
        }
        match request.send().await {
            Ok(resp) if resp.status().is_success() => {
                let text = resp
                    .text()
                    .await
                    .map_err(|e| VygrError::Network(format!("jina reader: {e}")))?;
                if text.trim().is_empty() {
                    return plain;
                }
                // The reader answers markdown: the first heading is a
                // reasonable title when present.
                let title = text
                    .lines()
                    .find(|l| l.starts_with("# "))
                    .map(|l| l.trim_start_matches("# ").trim().to_string());
                Ok(FetchPage {
                    url: url.to_string(),
                    final_url: url.to_string(),
                    title,
                    text: truncate_chars(text.trim(), max_chars),
                    content_type: "text/markdown".to_string(),
                })
            }
            // Reader failed too: surface the original plain result.
            _ => plain,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(content_type: &str, text: &str) -> FetchPage {
        FetchPage {
            url: "https://a.io".into(),
            final_url: "https://a.io".into(),
            title: None,
            text: text.to_string(),
            content_type: content_type.to_string(),
        }
    }

    #[test]
    fn escalation_heuristic() {
        // Failures escalate.
        assert!(needs_escalation(None));
        // Thin HTML escalates.
        assert!(needs_escalation(Some(&page("text/html", "loading..."))));
        // Substantial HTML does not.
        assert!(!needs_escalation(Some(&page(
            "text/html",
            &"word ".repeat(200)
        ))));
        // Short non-HTML (plain text endpoints) is left alone.
        assert!(!needs_escalation(Some(&page("application/json", "{}"))));
        // Empty anything escalates.
        assert!(needs_escalation(Some(&page("text/html", "   "))));
    }
}
