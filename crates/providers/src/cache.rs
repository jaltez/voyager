//! Disk cache with query-class TTLs (ADR-0010, milestone M1.2).
//!
//! Entries live at `<cache_dir>/search/<provider>/<sha256(key)>.json` and
//! expire after a TTL chosen by a keyword classifier over the query:
//! news-flavored queries stay fresh for minutes, reference-like ones for a
//! day. Corrupt entries are ignored and re-fetched, never fatal.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use vygr_core::config::CacheConf;
use vygr_core::provider::{SearchProvider, SearchQuery};
use vygr_core::types::SearchResult;
use vygr_core::VygrError;

use crate::throttle::now_unix_ms;

/// Query classes with distinct freshness horizons (hsearch-inspired).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryClass {
    News,
    Standard,
    Reference,
}

// Deliberately simple word-boundary keyword heuristics; documented as
// heuristics, tunable via [cache] TTL settings.
const NEWS_HINTS: &[&str] = &[
    "news",
    "latest",
    "today",
    "tonight",
    "breaking",
    "release",
    "released",
    "announce",
    "announcement",
    "2025",
    "2026",
    "2027",
];
const REFERENCE_HINTS: &[&str] = &[
    "docs",
    "documentation",
    "api",
    "reference",
    "specification",
    "spec",
    "manual",
    "guide",
    "tutorial",
    "cheatsheet",
    "howto",
];

pub fn classify(query: &str) -> QueryClass {
    let padded = format!(" {} ", query.to_lowercase());
    let mentions = |hints: &[&str]| hints.iter().any(|h| padded.contains(&format!(" {h} ")));
    if mentions(NEWS_HINTS) {
        QueryClass::News
    } else if mentions(REFERENCE_HINTS) {
        QueryClass::Reference
    } else {
        QueryClass::Standard
    }
}

/// Shared hit/miss counters surfaced in `--format json` meta (ADR-0008).
#[derive(Debug, Default)]
pub struct CacheStats {
    hits: AtomicU32,
    misses: AtomicU32,
}

impl CacheStats {
    /// `(hits, misses)` since the chain was built.
    pub fn snapshot(&self) -> (u32, u32) {
        (
            self.hits.load(Ordering::Relaxed),
            self.misses.load(Ordering::Relaxed),
        )
    }

    fn record_hit(&self) {
        self.hits.fetch_add(1, Ordering::Relaxed);
    }

    fn record_miss(&self) {
        self.misses.fetch_add(1, Ordering::Relaxed);
    }
}

/// Runtime cache behavior resolved from CLI flags on top of `[cache]`.
#[derive(Debug, Clone, Default)]
pub struct CacheOptions {
    /// `--no-cache`: bypass reading and writing entirely.
    pub disabled: bool,
    /// `--cache-ttl`: one TTL for every query class.
    pub force_ttl: Option<Duration>,
}

#[derive(Debug, Serialize, Deserialize)]
struct CacheEntry {
    created_at_ms: u64,
    ttl_secs: u64,
    results: Vec<SearchResult>,
}

/// Search provider decorator backed by the on-disk cache. Sits above the
/// throttle so cache hits cost neither spacing nor retries.
pub struct CachedSearchProvider {
    inner: Box<dyn SearchProvider>,
    /// `<cache_dir>/search`; `None` disables the decorator.
    dir: Option<PathBuf>,
    /// (news, standard, reference) TTLs.
    ttls: (Duration, Duration, Duration),
    opts: CacheOptions,
    stats: Arc<CacheStats>,
}

impl CachedSearchProvider {
    pub fn new(
        inner: Box<dyn SearchProvider>,
        dir: Option<PathBuf>,
        conf: &CacheConf,
        opts: CacheOptions,
        stats: Arc<CacheStats>,
    ) -> Self {
        let dir = if conf.enabled && !opts.disabled {
            dir
        } else {
            None
        };
        Self {
            inner,
            dir,
            ttls: (
                Duration::from_secs(conf.ttl_news_secs),
                Duration::from_secs(conf.ttl_standard_secs),
                Duration::from_secs(conf.ttl_reference_secs),
            ),
            opts,
            stats,
        }
    }

    fn ttl_for(&self, query: &str) -> Duration {
        self.opts.force_ttl.unwrap_or(match classify(query) {
            QueryClass::News => self.ttls.0,
            QueryClass::Standard => self.ttls.1,
            QueryClass::Reference => self.ttls.2,
        })
    }

    fn entry_path(&self, query: &SearchQuery) -> Option<PathBuf> {
        let dir = self.dir.as_ref()?;
        let fingerprint = format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}",
            self.inner.id(),
            normalize_query(&query.query),
            query.max_results,
            query.time_range.map(|t| t.ddg_param()).unwrap_or_default(),
            query.include_domains.join(","),
            query.exclude_domains.join(","),
            query.language.clone().unwrap_or_default(),
        );
        let mut hasher = Sha256::new();
        hasher.update(fingerprint.as_bytes());
        Some(
            dir.join(self.inner.id())
                .join(format!("{}.json", hex(&hasher.finalize()))),
        )
    }

    fn read_fresh(&self, path: &Path, ttl: Duration) -> Option<Vec<SearchResult>> {
        let text = std::fs::read_to_string(path).ok()?;
        let entry: CacheEntry = match serde_json::from_str(&text) {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("ignoring corrupt cache entry {}: {e}", path.display());
                return None;
            }
        };
        let age_ms = now_unix_ms().saturating_sub(entry.created_at_ms);
        (age_ms < ttl.as_millis() as u64).then_some(entry.results)
    }

    fn write(&self, path: &Path, ttl: Duration, results: &[SearchResult]) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let entry = CacheEntry {
            created_at_ms: now_unix_ms(),
            ttl_secs: ttl.as_secs(),
            results: results.to_vec(),
        };
        if let Ok(text) = serde_json::to_string(&entry) {
            let _ = std::fs::write(path, text);
        }
    }
}

#[async_trait]
impl SearchProvider for CachedSearchProvider {
    fn id(&self) -> &'static str {
        self.inner.id()
    }

    fn requires_env(&self) -> Option<&'static str> {
        self.inner.requires_env()
    }

    async fn search(&self, query: &SearchQuery) -> Result<Vec<SearchResult>, VygrError> {
        let Some(path) = self.entry_path(query) else {
            // Cache disabled: straight through, no stats recorded.
            return self.inner.search(query).await;
        };
        let ttl = self.ttl_for(&query.query);
        match self.read_fresh(&path, ttl) {
            Some(results) => {
                self.stats.record_hit();
                tracing::debug!("{}: cache hit ({:?} old entry)", self.inner.id(), ttl);
                Ok(results)
            }
            None => {
                self.stats.record_miss();
                let results = self.inner.search(query).await?;
                self.write(&path, ttl, &results);
                Ok(results)
            }
        }
    }
}

fn normalize_query(query: &str) -> String {
    query
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct CountingProvider {
        calls: Arc<AtomicU32>,
    }

    #[async_trait]
    impl SearchProvider for CountingProvider {
        fn id(&self) -> &'static str {
            "counting"
        }

        async fn search(&self, q: &SearchQuery) -> Result<Vec<SearchResult>, VygrError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(vec![SearchResult {
                title: q.query.clone(),
                url: "https://a.io".into(),
                snippet: String::new(),
                provider: "counting".into(),
                providers: vec!["counting".into()],
                content: None,
            }])
        }
    }

    fn setup(
        dir: &tempfile::TempDir,
        opts: CacheOptions,
    ) -> (CachedSearchProvider, Arc<CacheStats>, Arc<AtomicU32>) {
        let calls = Arc::new(AtomicU32::new(0));
        let inner = CountingProvider {
            calls: Arc::clone(&calls),
        };
        let stats = Arc::new(CacheStats::default());
        let provider = CachedSearchProvider::new(
            Box::new(inner),
            Some(dir.path().join("search")),
            &CacheConf::default(),
            opts,
            Arc::clone(&stats),
        );
        (provider, stats, calls)
    }

    #[tokio::test]
    async fn second_identical_query_is_served_from_cache() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (p, stats, calls) = setup(&tmp, CacheOptions::default());
        let q = SearchQuery::new("rust language", 5);
        assert_eq!(p.search(&q).await.unwrap().len(), 1);
        assert_eq!(p.search(&q).await.unwrap().len(), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(stats.snapshot(), (1, 1));
    }

    #[tokio::test]
    async fn different_query_or_depth_is_a_miss() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (p, stats, calls) = setup(&tmp, CacheOptions::default());
        p.search(&SearchQuery::new("rust language", 5))
            .await
            .unwrap();
        p.search(&SearchQuery::new("zig language", 5))
            .await
            .unwrap();
        p.search(&SearchQuery::new("rust language", 3))
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(stats.snapshot(), (0, 3));
    }

    #[tokio::test]
    async fn query_normalization_shares_entries() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (p, _stats, calls) = setup(&tmp, CacheOptions::default());
        p.search(&SearchQuery::new("Rust   Language", 5))
            .await
            .unwrap();
        p.search(&SearchQuery::new("rust language", 5))
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn expired_entry_refetches() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (p, stats, calls) = setup(&tmp, CacheOptions::default());
        let q = SearchQuery::new("rust language", 5);
        p.search(&q).await.unwrap();

        // Age the entry out by zeroing its creation stamp.
        let path = p.entry_path(&q).unwrap();
        let mut entry: CacheEntry =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        entry.created_at_ms = 0;
        std::fs::write(&path, serde_json::to_string(&entry).unwrap()).unwrap();

        p.search(&q).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(stats.snapshot(), (0, 2));
    }

    #[tokio::test]
    async fn corrupt_entry_is_ignored() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (p, _stats, calls) = setup(&tmp, CacheOptions::default());
        let q = SearchQuery::new("rust language", 5);
        p.search(&q).await.unwrap();
        let path = p.entry_path(&q).unwrap();
        std::fs::write(&path, "not json at all").unwrap();
        p.search(&q).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn forced_zero_ttl_never_serves() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (p, _stats, calls) = setup(
            &tmp,
            CacheOptions {
                disabled: false,
                force_ttl: Some(Duration::ZERO),
            },
        );
        let q = SearchQuery::new("rust language", 5);
        p.search(&q).await.unwrap();
        p.search(&q).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn disabled_option_bypasses_cache() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (p, stats, calls) = setup(
            &tmp,
            CacheOptions {
                disabled: true,
                force_ttl: None,
            },
        );
        let q = SearchQuery::new("rust language", 5);
        p.search(&q).await.unwrap();
        p.search(&q).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        // No stats are recorded while disabled.
        assert_eq!(stats.snapshot(), (0, 0));
    }

    #[test]
    fn classify_by_keywords() {
        assert_eq!(classify("rust latest news"), QueryClass::News);
        assert_eq!(classify("wasm gc 2026"), QueryClass::News);
        assert_eq!(classify("tokio api documentation"), QueryClass::Reference);
        assert_eq!(classify("rust vs zig benchmarks"), QueryClass::Standard);
    }

    #[test]
    fn fingerprint_is_stable_across_processes() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (p, _, _) = setup(&tmp, CacheOptions::default());
        let a = p
            .entry_path(&SearchQuery::new("Rust  Language", 5))
            .unwrap();
        let b = p.entry_path(&SearchQuery::new("rust language", 5)).unwrap();
        let c = p.entry_path(&SearchQuery::new("rust language", 3)).unwrap();
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}
