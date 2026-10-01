//! Normalized data types shared across providers and the research engine.

use serde::{Deserialize, Serialize};

/// A single web search hit, normalized across providers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    /// Provider id that produced this result (the first one, for merged results).
    pub provider: String,
    /// All providers that returned this URL after dedup merging.
    pub providers: Vec<String>,
    /// Full page content inlined on demand (`--extract-top`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

/// A fetched page, reduced to plain text.
#[derive(Debug, Clone, Serialize)]
pub struct FetchPage {
    pub url: String,
    pub final_url: String,
    pub title: Option<String>,
    pub text: String,
    pub content_type: String,
}

impl FetchPage {
    pub fn truncated(&self, max_chars: usize) -> String {
        truncate_chars(&self.text, max_chars)
    }
}

/// Truncate a string to `max_chars` bytes, cutting on a UTF-8 char boundary.
pub fn truncate_chars(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}
