//! Provider traits: the two seams every backend implements (ADR-0003).

use async_trait::async_trait;

use crate::error::VygrError;
use crate::types::{FetchPage, SearchResult};

/// Freshness window requested from providers that support it (M1.3).
/// Providers without native support ignore it; domain filters are always
/// enforced client-side regardless.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeRange {
    Day,
    Week,
    Month,
    Year,
}

impl TimeRange {
    pub fn parse(s: &str) -> Result<Self, VygrError> {
        match s.trim().to_lowercase().as_str() {
            "day" | "d" | "24h" => Ok(TimeRange::Day),
            "week" | "w" => Ok(TimeRange::Week),
            "month" | "mo" | "m" => Ok(TimeRange::Month),
            "year" | "y" | "yr" => Ok(TimeRange::Year),
            _ => Err(VygrError::Config(format!(
                "invalid time range '{s}' (expected day|week|month|year)"
            ))),
        }
    }

    /// DuckDuckGo HTML `df` parameter.
    pub fn ddg_param(self) -> &'static str {
        match self {
            TimeRange::Day => "d",
            TimeRange::Week => "w",
            TimeRange::Month => "m",
            TimeRange::Year => "y",
        }
    }

    /// Brave `freshness` parameter.
    pub fn brave_param(self) -> &'static str {
        match self {
            TimeRange::Day => "pd",
            TimeRange::Week => "pw",
            TimeRange::Month => "pm",
            TimeRange::Year => "py",
        }
    }

    /// Tavily `time_range` body field.
    pub fn tavily_param(self) -> &'static str {
        match self {
            TimeRange::Day => "day",
            TimeRange::Week => "week",
            TimeRange::Month => "month",
            TimeRange::Year => "year",
        }
    }
}

/// A normalized web search query.
#[derive(Debug, Clone)]
pub struct SearchQuery {
    pub query: String,
    pub max_results: usize,
    /// Freshness filter; ignored by providers without native support.
    pub time_range: Option<TimeRange>,
    /// Domain allowlist (empty = allow all).
    pub include_domains: Vec<String>,
    /// Domain blocklist.
    pub exclude_domains: Vec<String>,
}

impl SearchQuery {
    pub fn new(query: impl Into<String>, max_results: usize) -> Self {
        Self {
            query: query.into(),
            max_results,
            time_range: None,
            include_domains: Vec::new(),
            exclude_domains: Vec::new(),
        }
    }

    /// Attach filters (builder style).
    pub fn with_filters(
        mut self,
        time_range: Option<TimeRange>,
        include_domains: Vec<String>,
        exclude_domains: Vec<String>,
    ) -> Self {
        self.time_range = time_range;
        self.include_domains = normalize_domains(include_domains);
        self.exclude_domains = normalize_domains(exclude_domains);
        self
    }
}

/// Lowercase, trim and drop empty domain entries.
pub fn normalize_domains(domains: Vec<String>) -> Vec<String> {
    domains
        .into_iter()
        .map(|d| d.trim().to_lowercase())
        .filter(|d| !d.is_empty())
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_range_parsing_and_provider_params() {
        assert_eq!(TimeRange::parse("week").unwrap(), TimeRange::Week);
        assert_eq!(TimeRange::parse(" YR ").unwrap(), TimeRange::Year);
        assert!(TimeRange::parse("fortnight").is_err());
        assert_eq!(TimeRange::Month.ddg_param(), "m");
        assert_eq!(TimeRange::Day.brave_param(), "pd");
        assert_eq!(TimeRange::Year.tavily_param(), "year");
    }

    #[test]
    fn normalize_domains_cleans_lists() {
        let cleaned = normalize_domains(vec![" Rust-Lang.ORG ".into(), "".into(), "a.io".into()]);
        assert_eq!(cleaned, vec!["rust-lang.org", "a.io"]);
    }
}
