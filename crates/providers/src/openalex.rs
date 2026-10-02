//! OpenAlex provider (keyless, generous limits): open scholarly index
//! covering papers, DOIs and citations. Abstracts arrive as an inverted
//! word index and are reassembled positionally.

use async_trait::async_trait;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde::Deserialize;
use std::collections::HashMap;
use vygr_core::provider::{SearchProvider, SearchQuery};
use vygr_core::types::{truncate_chars, SearchResult};
use vygr_core::VygrError;

const ENDPOINT: &str = "https://api.openalex.org/works";

pub struct OpenAlexSearch {
    http: reqwest::Client,
}

impl OpenAlexSearch {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }
}

#[async_trait]
impl SearchProvider for OpenAlexSearch {
    fn id(&self) -> &'static str {
        "openalex"
    }

    async fn search(&self, q: &SearchQuery) -> Result<Vec<SearchResult>, VygrError> {
        let mailto = std::env::var("OPENALEX_MAILTO")
            .ok()
            .filter(|m| m.contains('@'));
        let mut url = format!(
            "{ENDPOINT}?search={}&per-page={}",
            utf8_percent_encode(&q.query, NON_ALPHANUMERIC),
            q.max_results.min(25)
        );
        // The polite pool asks for a contact address.
        if let Some(mail) = mailto {
            url.push_str("&mailto=");
            url.push_str(&mail);
        }
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| VygrError::Network(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(VygrError::provider_status(
                "openalex",
                format!("HTTP {status}"),
                status.as_u16(),
            ));
        }
        let body: OpenAlexResponse = resp
            .json()
            .await
            .map_err(|e| VygrError::Parse(format!("openalex response: {e}")))?;
        Ok(body
            .results
            .into_iter()
            .filter(|w| w.doi.as_deref().is_some_and(|d| d.starts_with("http")))
            .take(q.max_results)
            .map(|w| SearchResult {
                title: w.display_name,
                url: w.doi.unwrap_or_default(),
                snippet: truncate_chars(&rebuild_abstract(w.abstract_inverted_index), 400),
                provider: "openalex".into(),
                providers: vec!["openalex".into()],
                content: None,
            })
            .collect())
    }
}

/// Reassemble OpenAlex's `abstract_inverted_index` (word -> positions).
pub fn rebuild_abstract(inverted: Option<HashMap<String, Vec<usize>>>) -> String {
    let Some(inverted) = inverted else {
        return String::new();
    };
    let max_pos = inverted.values().flatten().copied().max().unwrap_or(0);
    let mut words = vec![String::new(); max_pos + 1];
    for (word, positions) in inverted {
        for pos in positions {
            if let Some(slot) = words.get_mut(pos) {
                *slot = word.clone();
            }
        }
    }
    words.join(" ")
}

#[derive(Debug, Deserialize)]
pub struct OpenAlexResponse {
    #[serde(default)]
    pub results: Vec<OpenAlexWork>,
}

#[derive(Debug, Deserialize)]
pub struct OpenAlexWork {
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub doi: Option<String>,
    #[serde(default, rename = "abstract_inverted_index")]
    pub abstract_inverted_index: Option<HashMap<String, Vec<usize>>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/openalex_sample.json");

    #[test]
    fn abstract_reassembly() {
        let body: OpenAlexResponse = serde_json::from_str(FIXTURE).unwrap();
        assert_eq!(body.results.len(), 2);
        assert_eq!(
            body.results[0].doi.as_deref(),
            Some("https://doi.org/10.1000/fake")
        );
        let abstract_text = rebuild_abstract(body.results[0].abstract_inverted_index.clone());
        assert_eq!(
            abstract_text,
            "Web search agents benefit from foraging signals"
        );
        // Works without an abstract degrade to an empty snippet.
        assert_eq!(
            rebuild_abstract(body.results[1].abstract_inverted_index.clone()),
            ""
        );
    }
}
