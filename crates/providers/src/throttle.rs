//! Per-provider politeness (ADR-0010, milestone M1.1): minimum request
//! spacing — in-process and advisory across processes — plus retry with
//! exponential backoff for retriable failures (429/5xx/connection errors).
//! Anti-bot challenges such as DDG's HTTP 202 are deliberately not retried;
//! spacing is what fixes them.

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use vygr_core::config::PolitenessConf;
use vygr_core::provider::{SearchProvider, SearchQuery};
use vygr_core::types::SearchResult;
use vygr_core::VygrError;

/// Cross-process gap sleeps are capped so a corrupt timestamp file cannot
/// stall a run.
const CROSS_PROCESS_CAP: Duration = Duration::from_secs(10);

/// Backoff grows by powers of two starting at this base.
const BACKOFF_BASE_MS: u64 = 500;

#[derive(Debug, Clone)]
pub struct ThrottlePolicy {
    pub min_interval: Duration,
    pub max_retries: u32,
    /// Backoff base; overridable so tests run in milliseconds.
    pub backoff_base: Duration,
}

impl ThrottlePolicy {
    pub fn from_config(id: &str, cfg: &PolitenessConf) -> Self {
        let ms = cfg
            .overrides
            .get(id)
            .copied()
            .unwrap_or(cfg.min_interval_ms_default);
        Self {
            min_interval: Duration::from_millis(ms),
            max_retries: cfg.max_retries,
            backoff_base: Duration::from_millis(BACKOFF_BASE_MS),
        }
    }
}

/// How long `now` must wait given the last accepted in-process timestamp.
/// `None` means "no wait"; a future timestamp also means "no wait".
fn spacing_wait(last: Instant, now: Instant, min_interval: Duration) -> Option<Duration> {
    let elapsed = now.checked_duration_since(last)?;
    nonzero(min_interval.checked_sub(elapsed))
}

/// Cross-process variant of [`spacing_wait`] on millisecond unix stamps.
fn cross_spacing_wait(last_ms: u64, now_ms: u64, min_interval: Duration) -> Option<Duration> {
    let elapsed = Duration::from_millis(now_ms.saturating_sub(last_ms));
    let wait = nonzero(min_interval.checked_sub(elapsed))?;
    (wait <= CROSS_PROCESS_CAP).then_some(wait)
}

/// A zero wait is no wait.
fn nonzero(wait: Option<Duration>) -> Option<Duration> {
    wait.filter(|w| !w.is_zero())
}

pub(crate) fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Search provider decorator applying spacing and retry.
pub struct ThrottledProvider {
    inner: Box<dyn SearchProvider>,
    policy: ThrottlePolicy,
    last: tokio::sync::Mutex<Option<Instant>>,
    cache_dir: Option<PathBuf>,
}

impl ThrottledProvider {
    pub fn new(
        inner: Box<dyn SearchProvider>,
        policy: ThrottlePolicy,
        cache_dir: Option<PathBuf>,
    ) -> Self {
        Self {
            inner,
            policy,
            last: tokio::sync::Mutex::new(None),
            cache_dir,
        }
    }

    async fn observe_spacing(&self) {
        let id = self.inner.id();
        let wait = {
            let mut guard = self.last.lock().await;
            match guard
                .as_ref()
                .and_then(|last| spacing_wait(*last, Instant::now(), self.policy.min_interval))
            {
                Some(wait) => Some(wait),
                None => {
                    *guard = Some(Instant::now());
                    None
                }
            }
        };
        if let Some(wait) = wait {
            tracing::debug!("{id}: in-process spacing for {wait:?}");
            tokio::time::sleep(wait).await;
            *self.last.lock().await = Some(Instant::now());
        }
        self.cross_process_spacing(id).await;
    }

    /// Advisory spacing across vygr invocations via a timestamp file in
    /// the cache directory. Best effort: races are tolerated.
    async fn cross_process_spacing(&self, id: &str) {
        let Some(dir) = &self.cache_dir else {
            return;
        };
        let path = dir.join("politeness").join(format!("{id}.last"));
        let now = now_unix_ms();
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Some(wait) = text
                .trim()
                .parse::<u64>()
                .ok()
                .and_then(|last| cross_spacing_wait(last, now, self.policy.min_interval))
            {
                tracing::debug!("{id}: cross-process spacing for {wait:?}");
                tokio::time::sleep(wait).await;
            }
        }
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, now_unix_ms().to_string());
    }
}

#[async_trait]
impl SearchProvider for ThrottledProvider {
    fn id(&self) -> &'static str {
        self.inner.id()
    }

    fn requires_env(&self) -> Option<&'static str> {
        self.inner.requires_env()
    }

    async fn search(&self, query: &SearchQuery) -> Result<Vec<SearchResult>, VygrError> {
        self.observe_spacing().await;
        let mut attempt = 0u32;
        loop {
            match self.inner.search(query).await {
                Ok(results) => return Ok(results),
                Err(e) if e.is_retriable() && attempt < self.policy.max_retries => {
                    attempt += 1;
                    let delay = backoff_delay(self.policy.backoff_base, attempt);
                    tracing::warn!(
                        "{}: {e}; retry {}/{} in {delay:?}",
                        self.inner.id(),
                        attempt,
                        self.policy.max_retries
                    );
                    tokio::time::sleep(delay).await;
                }
                Err(e) => return Err(e),
            }
        }
    }
}

/// Exponential backoff with jitter: `base * 2^(attempt-1) + 0..250ms`.
/// Jitter derives from the clock's subsecond nanos to avoid a rand
/// dependency; uniformity is irrelevant here.
fn backoff_delay(base: Duration, attempt: u32) -> Duration {
    let shift = attempt.saturating_sub(1).min(5);
    let millis = (base.as_millis() as u64) << shift;
    let jitter = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| (d.subsec_nanos() % 250) as u64)
        .unwrap_or(0);
    Duration::from_millis(millis + jitter)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn hit() -> SearchResult {
        SearchResult {
            title: "t".into(),
            url: "https://a.io".into(),
            snippet: String::new(),
            provider: "flaky".into(),
            providers: vec!["flaky".into()],
            content: None,
        }
    }

    /// Fails `failures` times with a 429, then succeeds once.
    struct FlakyProvider {
        failures: AtomicU32,
    }

    #[async_trait]
    impl SearchProvider for FlakyProvider {
        fn id(&self) -> &'static str {
            "flaky"
        }

        async fn search(&self, _q: &SearchQuery) -> Result<Vec<SearchResult>, VygrError> {
            if self.failures.fetch_sub(1, Ordering::SeqCst) > 0 {
                Err(VygrError::provider_status("flaky", "rate limited", 429))
            } else {
                Ok(vec![hit()])
            }
        }
    }

    fn fast_policy(retries: u32) -> ThrottlePolicy {
        ThrottlePolicy {
            min_interval: Duration::ZERO,
            max_retries: retries,
            backoff_base: Duration::from_millis(1),
        }
    }

    #[tokio::test]
    async fn recovers_retriable_failures_within_budget() {
        let inner = Box::new(FlakyProvider {
            failures: AtomicU32::new(2),
        });
        let p = ThrottledProvider::new(inner, fast_policy(2), None);
        assert_eq!(p.search(&SearchQuery::new("q", 1)).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn gives_up_after_max_retries() {
        let inner = Box::new(FlakyProvider {
            failures: AtomicU32::new(5),
        });
        let p = ThrottledProvider::new(inner, fast_policy(2), None);
        assert!(p.search(&SearchQuery::new("q", 1)).await.is_err());
    }

    #[tokio::test]
    async fn does_not_retry_non_retriable_errors() {
        struct Gone;
        #[async_trait]
        impl SearchProvider for Gone {
            fn id(&self) -> &'static str {
                "gone"
            }
            async fn search(&self, _q: &SearchQuery) -> Result<Vec<SearchResult>, VygrError> {
                Err(VygrError::provider_status("gone", "not found", 404))
            }
        }
        let p = ThrottledProvider::new(Box::new(Gone), fast_policy(3), None);
        assert!(p.search(&SearchQuery::new("q", 1)).await.is_err());
    }

    #[test]
    fn spacing_wait_math() {
        let now = Instant::now();
        let min = Duration::from_millis(1_200);
        assert_eq!(
            spacing_wait(now - Duration::from_millis(100), now, min),
            Some(Duration::from_millis(1_100))
        );
        assert_eq!(spacing_wait(now - min, now, min), None);
        // A future timestamp does not produce a wait.
        assert_eq!(spacing_wait(now + Duration::from_secs(5), now, min), None);
    }

    #[test]
    fn cross_spacing_wait_caps_and_ignores_old_stamps() {
        let now = 1_000_000u64;
        let min = Duration::from_millis(1_200);
        assert_eq!(
            cross_spacing_wait(now - 100, now, min),
            Some(Duration::from_millis(1_100))
        );
        // Long-past stamps mean no wait.
        assert_eq!(cross_spacing_wait(now - 60_000, now, min), None);
        // Future stamps degrade to a single interval, not an unbounded one.
        assert_eq!(
            cross_spacing_wait(now + 20 * 60 * 1_000, now, min),
            Some(min)
        );
    }

    #[test]
    fn backoff_delay_doubles_with_bounds() {
        let base = Duration::from_millis(500);
        let d1 = backoff_delay(base, 1);
        assert!(d1 >= Duration::from_millis(500) && d1 <= Duration::from_millis(750));
        let d2 = backoff_delay(base, 2);
        assert!(d2 >= Duration::from_millis(1_000) && d2 <= Duration::from_millis(1_250));
    }
}
