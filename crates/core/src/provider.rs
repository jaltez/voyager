//! Provider traits: the two seams every backend implements (ADR-0003).

use async_trait::async_trait;

use crate::error::VygrError;
use crate::types::{FetchPage, SearchResult};

/// A normalized web search query.
#[derive(Debug, Clone)]
pub struct SearchQuery {
    pub query: String,
    pub max_results: usize,
}

impl SearchQuery {
    pub fn new(query: impl Into<String>, max_results: usize) -> Self {
        Self {
            query: query.into(),
            max_results,
        }
    }
}

/// A searchable backend (DuckDuckGo, Brave, Tavily, ...).
#[async_trait]
pub trait SearchProvider: Send + Sync {
    /// Stable provider id, e.g. `"ddgs"` or `"brave"`.
    fn id(&self) -> &'static str;

    /// Environment variable required to use this provider, if any.
    fn requires_env(&self) -> Option<&'static str> {
        None
    }

    async fn search(&self, query: &SearchQuery) -> Result<Vec<SearchResult>, VygrError>;
}

/// A page fetcher: plain HTTP now, headless-browser escalation later (ADR-0003).
#[async_trait]
pub trait FetchProvider: Send + Sync {
    async fn fetch(&self, url: &str, max_chars: usize) -> Result<FetchPage, VygrError>;
}
