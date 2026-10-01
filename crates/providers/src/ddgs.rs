//! Keyless DuckDuckGo search via the HTML endpoint (the default provider so
//! vygr works with zero configuration).

use async_trait::async_trait;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use scraper::{Html, Selector};
use vygr_core::provider::{SearchProvider, SearchQuery};
use vygr_core::types::SearchResult;
use vygr_core::VygrError;

const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:130.0) Gecko/20100101 Firefox/130.0";

pub struct DdgSearch {
    http: reqwest::Client,
}

impl DdgSearch {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }
}

#[async_trait]
impl SearchProvider for DdgSearch {
    fn id(&self) -> &'static str {
        "ddgs"
    }

    async fn search(&self, q: &SearchQuery) -> Result<Vec<SearchResult>, VygrError> {
        let url = format!(
            "https://html.duckduckgo.com/html/?q={}",
            utf8_percent_encode(&q.query, NON_ALPHANUMERIC)
        );
        let resp = self
            .http
            .get(&url)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(reqwest::header::ACCEPT_LANGUAGE, "en-US,en;q=0.9")
            .send()
            .await
            .map_err(|e| VygrError::Network(e.to_string()))?;
        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| VygrError::Network(e.to_string()))?;
        if status.as_u16() == 202 {
            // DDG answers 202 with an anti-bot challenge page when the IP
            // queries too fast; there is nothing to parse.
            return Err(VygrError::Provider {
                provider: "ddgs".into(),
                message: "anti-bot challenge (HTTP 202); wait a bit, or switch provider \
(--provider brave / tavily, or configure a chain \"ddgs,brave\")"
                    .to_string(),
            });
        }
        if !status.is_success() {
            return Err(VygrError::Provider {
                provider: "ddgs".into(),
                message: format!(
                    "HTTP {status} (DuckDuckGo may be rate-limiting; retry or switch provider)"
                ),
            });
        }
        let mut results = parse_ddg_html(&body)?;
        results.truncate(q.max_results);
        Ok(results)
    }
}

pub fn parse_ddg_html(html: &str) -> Result<Vec<SearchResult>, VygrError> {
    let doc = Html::parse_document(html);
    let results_sel = sel("div.result")?;
    let link_sel = sel("a.result__a")?;
    let snippet_sel = sel("a.result__snippet")?;

    let mut out = Vec::new();
    for node in doc.select(&results_sel) {
        let Some(link) = node.select(&link_sel).next() else {
            continue;
        };
        let Some(href) = link.value().attr("href") else {
            continue;
        };
        let title: String = link.text().collect::<String>().trim().to_string();
        let url = clean_ddg_url(href);
        if title.is_empty() || !url.starts_with("http") {
            continue;
        }
        let snippet = node
            .select(&snippet_sel)
            .next()
            .map(|s| s.text().collect::<String>().trim().to_string())
            .unwrap_or_default();
        out.push(SearchResult {
            title,
            url,
            snippet,
            provider: "ddgs".into(),
            providers: vec!["ddgs".into()],
            content: None,
        });
    }
    Ok(out)
}

fn sel(css: &str) -> Result<Selector, VygrError> {
    Selector::parse(css).map_err(|e| VygrError::Parse(format!("internal selector error: {e:?}")))
}

/// Resolve DuckDuckGo redirect links (`/l/?uddg=<encoded>`) to target URLs.
fn clean_ddg_url(href: &str) -> String {
    if let Some(idx) = href.find("uddg=") {
        let raw = href[idx + 5..].split('&').next().unwrap_or("");
        if let Ok(decoded) = percent_encoding::percent_decode_str(raw).decode_utf8() {
            return decoded.to_string();
        }
    }
    if let Some(rest) = href.strip_prefix("//") {
        return format!("https://{rest}");
    }
    href.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/ddg_sample.html");

    #[test]
    fn parses_results_and_decodes_redirects() {
        let results = parse_ddg_html(FIXTURE).expect("fixture parses");
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].url, "https://rust-lang.org/learn");
        assert_eq!(results[0].title, "Learn Rust - Rust Programming Language");
        assert!(results[0].snippet.contains("official guide"));
        assert_eq!(results[1].url, "https://doc.rust-lang.org/std/");
    }

    #[test]
    fn clean_url_handles_plain_and_redirect_links() {
        assert_eq!(
            clean_ddg_url("//duckduckgo.com/l/?uddg=https%3A%2F%2Fa.io%2Fx&rut=1"),
            "https://a.io/x"
        );
        assert_eq!(
            clean_ddg_url("//example.com/page"),
            "https://example.com/page"
        );
        assert_eq!(clean_ddg_url("https://example.com"), "https://example.com");
    }
}
