//! arXiv provider (keyless): the Atom API at export.arxiv.org. Complements
//! OpenAlex for paper-shaped queries with abstracts and canonical abs
//! links. The Atom feed is machine-generated and stable, so a focused
//! parser (no XML dependency) covers it; entries with no parseable id are
//! skipped rather than guessed.

use async_trait::async_trait;
use vygr_core::provider::{SearchProvider, SearchQuery};
use vygr_core::types::{truncate_chars, SearchResult};
use vygr_core::VygrError;

const ENDPOINT: &str = "https://export.arxiv.org/api/query";

pub struct ArxivSearch {
    http: reqwest::Client,
}

impl ArxivSearch {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }
}

#[async_trait]
impl SearchProvider for ArxivSearch {
    fn id(&self) -> &'static str {
        "arxiv"
    }

    async fn search(&self, q: &SearchQuery) -> Result<Vec<SearchResult>, VygrError> {
        let resp = self
            .http
            .get(ENDPOINT)
            .query(&[
                ("search_query", format!("all:{}", q.query)),
                ("max_results", q.max_results.clamp(1, 30).to_string()),
                ("sortBy", "relevance".to_string()),
            ])
            .send()
            .await
            .map_err(|e| VygrError::Network(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(VygrError::provider_status(
                "arxiv",
                format!("HTTP {status}"),
                status.as_u16(),
            ));
        }
        let body = resp
            .text()
            .await
            .map_err(|e| VygrError::Parse(format!("arxiv response: {e}")))?;
        Ok(parse_feed(&body, q.max_results))
    }
}

/// Parse an arXiv Atom feed into results. Public for fixture tests.
pub fn parse_feed(feed: &str, max_results: usize) -> Vec<SearchResult> {
    feed.split("<entry>")
        .skip(1)
        .filter_map(|chunk| {
            let end = chunk.find("</entry>")?;
            let entry = &chunk[..end];
            let id = inner_text(entry, "id")?;
            // Canonical abs link; keep the version suffix.
            let url = if id.starts_with("http") {
                id
            } else {
                format!("https://arxiv.org/abs/{id}")
            };
            let title = collapse_ws(&inner_text(entry, "title").unwrap_or_default());
            let summary = collapse_ws(&inner_text(entry, "summary").unwrap_or_default());
            if title.is_empty() {
                return None;
            }
            Some(SearchResult {
                title,
                url,
                snippet: truncate_chars(&summary, 400),
                provider: "arxiv".into(),
                providers: vec!["arxiv".into()],
                content: None,
            })
        })
        .take(max_results)
        .collect()
}

/// Inner text of the first `<tag>...</tag>` occurrence, with the common
/// XML entities decoded.
fn inner_text(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let start = xml.find(&open)? + open.len();
    let close = format!("</{tag}>");
    let end = xml[start..].find(&close)? + start;
    Some(decode_entities(&xml[start..end]))
}

fn collapse_ws(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn decode_entities(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/arxiv_sample.xml");

    #[test]
    fn parses_entries_into_abs_links() {
        let results = parse_feed(FIXTURE, 10);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].url, "http://arxiv.org/abs/2401.12345v1");
        assert_eq!(results[0].title, "Web agents need foraging signals");
        assert!(results[0].snippet.starts_with("We study how search agents"));
        assert_eq!(results[0].provider, "arxiv");
        // A second entry without a usable id is skipped entirely.
        assert!(results.iter().all(|r| !r.url.is_empty()));
    }

    #[test]
    fn max_results_caps_and_entities_decode() {
        let results = parse_feed(FIXTURE, 1);
        assert_eq!(results.len(), 1);
        let results = parse_feed(FIXTURE, 10);
        assert!(results[1].title.contains("Q&A"), "{:?}", results[1].title);
    }

    #[test]
    fn malformed_feeds_yield_no_results() {
        assert!(parse_feed("<feed><entry>no fields</entry></feed>", 5).is_empty());
        assert!(parse_feed("totally not xml", 5).is_empty());
    }
}
