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

/// Extract `<title>` and a structured markdown rendering of the main
/// content (M1.5): prefer `article` / `[role=main]` / `main` as the root,
/// drop boilerplate subtrees (nav, footer, aside, header, form, scripts…)
/// and emit block elements in document order — headings as markdown, list
/// items as bullets, `pre` as fenced code, paragraphs as text. Elements
/// nested inside another block element are skipped so text is not
/// duplicated; the ancestor emits its flattened content instead.
pub fn extract_html(html: &str) -> (Option<String>, String) {
    let doc = Html::parse_document(html);
    let title = Selector::parse("title")
        .ok()
        .and_then(|s| doc.select(&s).next())
        .map(|t| t.text().collect::<String>().trim().to_string())
        .filter(|t| !t.is_empty());

    let root = ["article", "[role=main]", "main", "body"]
        .iter()
        .find_map(|css| {
            Selector::parse(css)
                .ok()
                .and_then(|s| doc.select(&s).next())
        });

    let Some(root) = root else {
        return (title, String::new());
    };

    let blocks = Selector::parse("p, h1, h2, h3, h4, h5, h6, li, pre, blockquote")
        .ok()
        .map(|sel| {
            root.select(&sel)
                .filter(|el| !inside_skipped_or_nested_block(el))
                .filter_map(|el| {
                    let text = collapse(el.text());
                    if text.is_empty() {
                        return None;
                    }
                    Some(render_block(el.value().name(), &text))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    (title, blocks.join("\n\n"))
}

/// Boilerplate subtrees never contribute content.
const SKIP_TAGS: &[&str] = &[
    "script", "style", "noscript", "nav", "footer", "aside", "header", "form", "template",
    "iframe", "svg", "button", "select",
];

const BLOCK_TAGS: &[&str] = &[
    "p",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "li",
    "pre",
    "blockquote",
];

fn inside_skipped_or_nested_block(el: &scraper::ElementRef) -> bool {
    el.ancestors()
        .filter_map(|node| node.value().as_element())
        .any(|ancestor| {
            SKIP_TAGS.contains(&ancestor.name())
                || BLOCK_TAGS.contains(&ancestor.name())
                // ARIA landmark roles catch boilerplate that skips semantic
                // tags (Wikipedia sidebars are div role="navigation").
                || matches!(
                    ancestor.attr("role"),
                    Some("navigation") | Some("complementary") | Some("banner") | Some("contentinfo")
                )
        })
}

fn render_block(tag: &str, text: &str) -> String {
    match tag {
        "h1" => format!("# {text}"),
        "h2" => format!("## {text}"),
        "h3" => format!("### {text}"),
        "h4" => format!("#### {text}"),
        "h5" => format!("##### {text}"),
        "h6" => format!("###### {text}"),
        "li" => format!("- {text}"),
        "blockquote" => format!("> {text}"),
        "pre" => format!("```\n{text}\n```"),
        _ => text.to_string(),
    }
}

fn collapse<'a>(text: impl Iterator<Item = &'a str>) -> String {
    text.collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_scripts_and_keeps_paragraphs() {
        let html = "<html><head><title> Example </title></head><body><script>ignore()</script><p>Hello   <b>world</b>, hi.</p></body></html>";
        let (title, text) = extract_html(html);
        assert_eq!(title.as_deref(), Some("Example"));
        assert_eq!(text, "Hello world, hi.");
    }

    #[test]
    fn renders_article_structure_as_markdown() {
        let html = "<html><head><title>Article</title></head><body>\
<nav><p>Home About</p></nav>\
<main><article>\
<h1>Big Title</h1>\
<p>Intro paragraph.</p>\
<h2>Section</h2>\
<ul><li>one</li><li>two</li></ul>\
<pre>let x = 1;</pre>\
</article></main>\
<footer><p>copyright</p></footer>\
</body></html>";
        let (title, text) = extract_html(html);
        assert_eq!(title.as_deref(), Some("Article"));
        assert_eq!(
            text,
            "# Big Title\n\nIntro paragraph.\n\n## Section\n\n- one\n\n- two\n\n```\nlet x = 1;\n```"
        );
    }

    #[test]
    fn body_root_drops_nav_and_footer_noise() {
        let html = "<html><body>\
<nav><p>nav noise</p><a>skip link</a></nav>\
<h1>Title</h1>\
<p>Real content.</p>\
<footer><p>foot noise</p></footer>\
</body></html>";
        let (_title, text) = extract_html(html);
        assert_eq!(text, "# Title\n\nReal content.");
    }

    #[test]
    fn nested_blocks_are_not_duplicated() {
        // A p inside a blockquote is skipped: the blockquote emits once.
        let html = "<body><blockquote><p>quoted words</p></blockquote><ul><li>outer <ul><li>inner</li></ul></li></ul></body>";
        let (_title, text) = extract_html(html);
        assert_eq!(text, "> quoted words\n\n- outer inner");
    }
}
