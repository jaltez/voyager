//! Plain-HTTP page fetcher with naive HTML-to-text extraction.
//!
//! Escalation to a headless browser for JS-heavy pages is planned
//! (ADR-0003, roadmap phase 5); today a failed or empty fetch is simply
//! reported and the research loop moves on to the next source.

use async_trait::async_trait;
use scraper::{Html, Selector};
use vygr_core::provider::FetchProvider;
use vygr_core::types::{truncate_chars, FetchPage};
use vygr_core::VygrError;

const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:130.0) Gecko/20100101 Firefox/130.0";

pub struct HttpFetch {
    http: reqwest::Client,
}

impl HttpFetch {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }
}

#[async_trait]
impl FetchProvider for HttpFetch {
    async fn fetch(&self, url: &str, max_chars: usize) -> Result<FetchPage, VygrError> {
        let resp = self
            .http
            .get(url)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .send()
            .await
            .map_err(|e| VygrError::Network(format!("fetching {url}: {e}")))?;
        let status = resp.status();
        let final_url = resp.url().to_string();
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_string();
        if !status.is_success() {
            return Err(VygrError::provider_status(
                "http",
                format!("HTTP {status} fetching {url}"),
                status.as_u16(),
            ));
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| VygrError::Network(format!("reading {url}: {e}")))?;
        let raw = String::from_utf8_lossy(&bytes);

        let (title, text) = if content_type.contains("html") {
            extract_html(&raw)
        } else {
            (None, raw.trim().to_string())
        };
        Ok(FetchPage {
            url: url.to_string(),
            final_url,
            title,
            text: truncate_chars(&text, max_chars),
            content_type,
        })
    }
}

/// Extract `<title>` and the collapsed text of `<body>`.
pub fn extract_html(html: &str) -> (Option<String>, String) {
    let doc = Html::parse_document(html);
    let title = Selector::parse("title")
        .ok()
        .and_then(|s| doc.select(&s).next())
        .map(|t| t.text().collect::<String>().trim().to_string())
        .filter(|t| !t.is_empty());
    let body = Selector::parse("body")
        .ok()
        .and_then(|s| doc.select(&s).next())
        .map(|b| {
            let mut text = String::new();
            for chunk in b.text() {
                text.push_str(chunk);
                text.push(' ');
            }
            text
        })
        .unwrap_or_default();
    let collapsed = body.split_whitespace().collect::<Vec<_>>().join(" ");
    (title, collapsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_title_and_collapsed_body_text() {
        let html = "<html><head><title> Example </title></head><body><script>ignore()</script><p>Hello   <b>world</b>, hi.</p></body></html>";
        let (title, text) = extract_html(html);
        assert_eq!(title.as_deref(), Some("Example"));
        assert_eq!(text, "ignore() Hello world , hi.");
    }
}
