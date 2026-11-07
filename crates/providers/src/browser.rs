//! Headless-browser fetch — the last rung of the escalation ladder
//! (ADR-0019), behind the `browser` feature. Renders the page with
//! headless Chrome/Chromium via CDP (chromiumoxide) and reuses the plain
//! HTTP fetcher's HTML extraction. Requires a chrome binary on the
//! system; without one (or without the feature) it fails cleanly and the
//! ladder keeps the previous result. As root (CI, containers) Chrome
//! refuses to sandbox, so there the browser runs sandbox-less or not at
//! all. One browser process per escalated
//! URL: escalations are rare by design (only pages that failed plain
//! HTTP *and* the Jina reader), so session reuse is not worth the
//! lifecycle complexity.

use async_trait::async_trait;
use vygr_core::provider::FetchProvider;
use vygr_core::types::{truncate_chars, FetchPage};
use vygr_core::VygrError;

use crate::fetch::extract_html;

pub struct BrowserFetch;

impl BrowserFetch {
    pub fn new() -> Self {
        Self
    }
}

impl Default for BrowserFetch {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl FetchProvider for BrowserFetch {
    async fn fetch(&self, url: &str, max_chars: usize) -> Result<FetchPage, VygrError> {
        fetch_page(url, max_chars).await
    }
}

/// Render a URL in a headless browser and return its HTML.
#[cfg(feature = "browser")]
async fn render_html(url: &str) -> Result<String, VygrError> {
    use chromiumoxide::{Browser, BrowserConfig};
    use futures::StreamExt;

    let mut builder = BrowserConfig::builder();
    // chromiumoxide resolves the executable itself when none is set, but
    // hand it the probed binary explicitly when we have one so the rung
    // works wherever the probe does.
    if let Some(bin) = detect_chrome() {
        builder = builder.chrome_executable(bin);
    }
    if chrome_needs_no_sandbox() {
        builder = builder.no_sandbox();
    }
    let config = builder
        .build()
        .map_err(|e| VygrError::Config(format!("browser config: {e}")))?;
    let (mut browser, mut handler) = Browser::launch(config).await.map_err(|e| {
        VygrError::Network(format!(
            "launching headless browser (is chrome/chromium installed?): {e}"
        ))
    })?;
    tokio::spawn(async move { while let Some(_event) = handler.next().await {} });
    let page = browser
        .new_page(url)
        .await
        .map_err(|e| VygrError::Network(format!("browser open {url}: {e}")))?;
    let html: String = page
        .content()
        .await
        .map_err(|e| VygrError::Network(format!("browser render {url}: {e}")))?;
    let _ = browser.close().await;
    if html.trim().is_empty() {
        return Err(VygrError::Network(format!(
            "browser rendered nothing for {url}"
        )));
    }
    Ok(html)
}

#[cfg(not(feature = "browser"))]
async fn render_html(_url: &str) -> Result<String, VygrError> {
    Err(VygrError::Config(
        "browser escalation needs a build with `--features browser`".to_string(),
    ))
}

/// Probe the usual chrome binary names the way chromiumoxide's own
/// detection does (`detection::default_executable` walks a similar
/// candidate list), so callers can tell "no browser installed" apart
/// from "browser failed to render".
#[cfg(feature = "browser")]
fn detect_chrome() -> Option<std::path::PathBuf> {
    const CANDIDATES: [&str; 3] = ["google-chrome-stable", "google-chrome", "chromium"];
    for bin in CANDIDATES {
        if let Ok(out) = std::process::Command::new(bin).arg("--version").output() {
            if out.status.success() {
                // chromiumoxide canonicalizes the executable without
                // searching PATH, so hand it an absolute path.
                return resolve_on_path(bin);
            }
        }
    }
    None
}

/// Locate `bin` on PATH the same way `std::process::Command` did when
/// probing it, as an absolute path.
#[cfg(feature = "browser")]
fn resolve_on_path(bin: &str) -> Option<std::path::PathBuf> {
    let dirs = std::env::var_os("PATH")?;
    std::env::split_paths(&dirs)
        .map(|dir| dir.join(bin))
        .find(|p| p.is_file())
}

/// Chrome refuses to start its sandbox when run as root (common in CI
/// and container shells), so there the rung is sandbox-less or nothing;
/// as a normal user the sandbox stays on.
#[cfg(feature = "browser")]
fn chrome_needs_no_sandbox() -> bool {
    // SAFETY: geteuid is a plain syscall wrapper with no preconditions.
    unsafe { libc::geteuid() == 0 }
}

/// Extract a FetchPage from browser-rendered HTML (same extraction as
/// the plain fetcher).
async fn fetch_page(url: &str, max_chars: usize) -> Result<FetchPage, VygrError> {
    let html = render_html(url).await?;
    let (title, text) = extract_html(&html);
    Ok(FetchPage {
        url: url.to_string(),
        final_url: url.to_string(),
        title,
        text: truncate_chars(&text, max_chars),
        content_type: "text/html".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "browser")]
    use std::time::Duration;

    /// Decisive in both directions. A probe of the usual chrome names
    /// decides what the rung must do: with a binary installed the fetch
    /// must really render example.com (Ok, non-empty text); without one
    /// (or without the feature) it must degrade to a clean Config |
    /// Network error. Either way: no panic, no hang.
    #[tokio::test]
    async fn missing_browser_degrades_cleanly() {
        #[cfg(not(feature = "browser"))]
        {
            let err = BrowserFetch::new()
                .fetch("https://example.com", 1_000)
                .await;
            assert!(matches!(err, Err(VygrError::Config(_))));
        }

        #[cfg(feature = "browser")]
        {
            let fetcher = BrowserFetch::new();
            let fetched = fetcher.fetch("https://example.com", 1_000);
            let result = tokio::time::timeout(Duration::from_secs(120), fetched)
                .await
                .expect("browser fetch did not finish within 120s");
            match (detect_chrome(), result) {
                (Some(bin), Ok(page)) => {
                    println!(
                        "rendered example.com with {}: title={:?}, {} chars: {:?}",
                        bin.display(),
                        page.title,
                        page.text.len(),
                        truncate_chars(&page.text, 120)
                    );
                    assert!(
                        !page.text.is_empty(),
                        "chrome ({}) rendered example.com but the extracted text is empty",
                        bin.display()
                    );
                }
                (Some(bin), Err(e)) => {
                    panic!(
                        "chrome binary {} is installed but the browser fetch failed: {e}",
                        bin.display()
                    )
                }
                (None, Err(e)) => {
                    assert!(
                        matches!(e, VygrError::Config(_) | VygrError::Network(_)),
                        "without a chrome binary the rung must degrade to a clean error, got: {e}"
                    );
                }
                (None, Ok(_)) => {
                    panic!("browser fetch succeeded although no chrome binary was detected")
                }
            }
        }
    }
}
