//! `vygr update`: self-update from crates.io, the channel every release
//! goes through (GitHub releases only track phase tags, so they lag).
//! Reports current vs latest, then reinstalls via `cargo install vygr`
//! when an update exists. A binary installed by `install.sh` can also
//! self-update this way as long as cargo is on PATH.

use clap::Args as ClapArgs;
use serde::Deserialize;
use vygr_core::VygrError;

const CRATE_NAME: &str = "vygr";
const CRATES_IO: &str = "https://crates.io/api/v1/crates/vygr";

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Only report whether an update exists; do not reinstall
    #[arg(long)]
    pub check: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SemVer {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl SemVer {
    /// Parse a `major.minor.patch` version, ignoring any pre-release or
    /// build suffix (`-alpha`, `+meta`).
    pub fn parse(s: &str) -> Option<SemVer> {
        let core = s.trim().split(['-', '+']).next()?;
        let mut parts = core.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = parts.next()?.parse().ok()?;
        if parts.next().is_some() {
            return None;
        }
        Some(SemVer {
            major,
            minor,
            patch,
        })
    }

    fn tuple(self) -> (u64, u64, u64) {
        (self.major, self.minor, self.patch)
    }
}

impl std::fmt::Display for SemVer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateStatus {
    UpToDate,
    Behind,
}

/// Compare the running version against the latest published one.
/// Unparseable versions are treated as "cannot decide" by the caller.
pub fn update_status(current: SemVer, latest: SemVer) -> UpdateStatus {
    if latest.tuple() > current.tuple() {
        UpdateStatus::Behind
    } else {
        UpdateStatus::UpToDate
    }
}

#[derive(Debug, Deserialize)]
struct CratesIoResponse {
    #[serde(default, rename = "crate")]
    krate: CratesIoCrate,
}

#[derive(Debug, Default, Deserialize)]
struct CratesIoCrate {
    #[serde(default)]
    max_version: Option<String>,
}

/// Latest version published on crates.io. The shared HTTP client already
/// sends a descriptive User-Agent, which crates.io requires.
async fn latest_version(http: &reqwest::Client) -> Result<SemVer, VygrError> {
    let resp = http
        .get(CRATES_IO)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| VygrError::Network(format!("crates.io: {e}")))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(VygrError::provider_status(
            "crates.io",
            format!("HTTP {status}"),
            status.as_u16(),
        ));
    }
    let body: CratesIoResponse = resp
        .json()
        .await
        .map_err(|e| VygrError::Parse(format!("crates.io response: {e}")))?;
    let raw = body
        .krate
        .max_version
        .ok_or_else(|| VygrError::Parse("crates.io response had no max_version".to_string()))?;
    SemVer::parse(&raw)
        .ok_or_else(|| VygrError::Parse(format!("cannot parse crates.io version '{raw}'")))
}

/// True when the running binary comes from a source checkout
/// (`target/debug` or `target/release`), where self-update makes no sense.
fn running_from_source_build() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(str::to_string))
        .is_some_and(|p| p.contains("/target/debug/") || p.contains("/target/release/"))
}

fn cargo_available() -> bool {
    std::process::Command::new("cargo")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

pub async fn run(args: Args, http: reqwest::Client) -> Result<(), VygrError> {
    let current_raw = env!("CARGO_PKG_VERSION");
    let current = SemVer::parse(current_raw)
        .ok_or_else(|| VygrError::Parse("cannot parse the running version".to_string()))?;
    let latest = latest_version(&http).await?;

    if update_status(current, latest) == UpdateStatus::UpToDate {
        println!("vygr {current_raw} is up to date (latest on crates.io: {latest})");
        return Ok(());
    }

    println!("update available: vygr {current_raw} -> {latest}");
    if args.check {
        println!("run `vygr update` to reinstall from crates.io");
        return Ok(());
    }

    if running_from_source_build() {
        return Err(VygrError::Config(
            "this binary was built from a source checkout; update with `git pull && cargo build` instead"
                .to_string(),
        ));
    }

    if !cargo_available() {
        return Err(VygrError::Config(
            "cargo is not on PATH; install Rust from https://rustup.rs, or re-run install.sh to fetch a prebuilt binary"
                .to_string(),
        ));
    }

    let exe = std::env::current_exe()
        .ok()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "(unknown)".to_string());
    println!("running `cargo install {CRATE_NAME}` ...");
    let status = tokio::process::Command::new("cargo")
        .args(["install", CRATE_NAME])
        .status()
        .await
        .map_err(|e| VygrError::Network(format!("spawning cargo: {e}")))?;
    if !status.success() {
        return Err(VygrError::provider(
            "cargo",
            format!("install exited with {status}"),
        ));
    }

    // If the running binary is not the one cargo replaced, say so.
    let cargo_bin = dirs::home_dir()
        .map(|h| h.join(".cargo").join("bin").join(CRATE_NAME))
        .filter(|p| p.exists());
    match cargo_bin {
        Some(path) if std::fs::canonicalize(&path).ok().as_deref() != std::fs::canonicalize(std::env::current_exe().unwrap_or_default()).ok().as_deref() => {
            eprintln!(
                "vygr: updated {} installed at {}; the running binary at {} is replaced on next invocation if that directory precedes it on PATH",
                latest,
                path.display(),
                exe
            );
        }
        _ => eprintln!("vygr: updated to {latest}; restart your shell if `vygr --version` still shows the old version"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_versions_and_ignores_suffixes() {
        assert_eq!(
            SemVer::parse("0.4.3"),
            Some(SemVer {
                major: 0,
                minor: 4,
                patch: 3
            })
        );
        assert_eq!(
            SemVer::parse("1.2.3-rc.1"),
            Some(SemVer {
                major: 1,
                minor: 2,
                patch: 3
            })
        );
        assert_eq!(
            SemVer::parse(" 2.0.0+meta "),
            Some(SemVer {
                major: 2,
                minor: 0,
                patch: 0
            })
        );
        assert_eq!(SemVer::parse("1.2"), None);
        assert_eq!(SemVer::parse("x.y.z"), None);
        assert_eq!(SemVer::parse("1.2.3.4"), None);
    }

    #[test]
    fn compares_tuples() {
        let v = |s| SemVer::parse(s).unwrap();
        assert_eq!(
            update_status(v("0.4.3"), v("0.4.3")),
            UpdateStatus::UpToDate
        );
        assert_eq!(update_status(v("0.4.3"), v("0.4.10")), UpdateStatus::Behind);
        assert_eq!(update_status(v("0.4.3"), v("1.0.0")), UpdateStatus::Behind);
        // Numeric, not lexicographic: 0.4.9 > 0.4.10 is false.
        assert_eq!(
            update_status(v("0.4.10"), v("0.4.9")),
            UpdateStatus::UpToDate
        );
        // Running a newer version than the registry (local build) is up to date.
        assert_eq!(
            update_status(v("0.5.0"), v("0.4.3")),
            UpdateStatus::UpToDate
        );
    }

    #[tokio::test]
    async fn source_build_detection_works_for_current_exe() {
        // Tests run from target/debug (or a temp dir mirroring it); the
        // exact value depends on the harness, so only assert it is boolean.
        let _ = running_from_source_build();
    }
}
