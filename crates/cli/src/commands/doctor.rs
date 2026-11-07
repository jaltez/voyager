//! `vygr doctor`: environment self-check so agents (and humans) can see
//! what works before spending calls: config layers, provider readiness,
//! LLM backend reachability, cache/run-dir writability, skill installs,
//! catalog freshness (ADR-0016).

use std::path::Path;

use clap::Args as ClapArgs;
use vygr_core::config::Config;
use vygr_core::VygrError;

use crate::output::GenericFormat;

#[derive(Debug, ClapArgs)]
pub struct Args {
    #[arg(long, value_enum, default_value = "table")]
    pub format: GenericFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    Warn,
    Fail,
}

impl Status {
    fn label(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Warn => "warn",
            Status::Fail => "FAIL",
        }
    }
}

#[derive(Debug, serde::Serialize)]
struct Check {
    name: &'static str,
    status: Status,
    detail: String,
}

fn check(name: &'static str, status: Status, detail: impl Into<String>) -> Check {
    Check {
        name,
        status,
        detail: detail.into(),
    }
}

pub async fn run(args: Args) -> Result<(), VygrError> {
    let mut checks = Vec::new();
    run_config_checks(&mut checks);
    run_provider_checks(&Config::load(), &mut checks);
    run_llm_checks(&Config::load(), &mut checks).await;
    run_fs_checks(&Config::load(), &mut checks);
    run_skill_checks(&mut checks);
    run_catalog_check(&mut checks);

    let (ok, warn, fail) = checks.iter().fold((0, 0, 0), |acc, c| match c.status {
        Status::Ok => (acc.0 + 1, acc.1, acc.2),
        Status::Warn => (acc.0, acc.1 + 1, acc.2),
        Status::Fail => (acc.0, acc.1, acc.2 + 1),
    });

    match args.format {
        GenericFormat::Json => println!("{}", serde_json::to_string_pretty(&checks)?),
        GenericFormat::Table => {
            println!("{:<14}  {:<5}  DETAIL", "CHECK", "STATUS");
            println!("{}", "-".repeat(78));
            for c in &checks {
                println!("{:<14}  {:<5}  {}", c.name, c.status.label(), c.detail);
            }
            eprintln!("vygr: doctor: {ok} ok, {warn} warn, {fail} failed");
        }
    }

    // Config/auth problems are exit code 3 (ADR-0008); warnings don't fail.
    if fail > 0 {
        std::process::exit(3);
    }
    Ok(())
}

fn run_config_checks(checks: &mut Vec<Check>) {
    let cfg = Config::load();
    if cfg.sources.is_empty() {
        checks.push(check(
            "config",
            Status::Warn,
            "no config files found (defaults in effect); create one via `vygr config` paths",
        ));
    } else {
        let files = cfg
            .sources
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        checks.push(check("config", Status::Ok, files));
    }
}

fn run_provider_checks(cfg: &Config, checks: &mut Vec<Check>) {
    let mut ready = Vec::new();
    let mut missing = Vec::new();
    for id in vygr_providers::BUILTIN {
        let env = vygr_providers::env_requirement(id);
        let is_ready = match env {
            Some(name) => std::env::var(name)
                .map(|v| !v.trim().is_empty())
                .unwrap_or(false),
            None if *id == "searxng" => cfg
                .providers
                .get("searxng")
                .and_then(|p| p.base_url.clone())
                .or_else(|| {
                    std::env::var("SEARXNG_BASE_URL")
                        .ok()
                        .filter(|v| !v.trim().is_empty())
                })
                .is_some(),
            None => true,
        };
        if is_ready {
            ready.push(*id);
        } else if let Some(name) = env {
            missing.push(format!("{id} ({name})"));
        }
    }
    if ready.is_empty() {
        checks.push(check(
            "providers",
            Status::Fail,
            "no search provider is ready",
        ));
        return;
    }
    let status = if ready.len() == 1 && ready[0] == "ddgs" {
        Status::Warn
    } else {
        Status::Ok
    };
    let mut detail = format!("ready: {}", ready.join(", "));
    if !missing.is_empty() {
        detail.push_str(&format!("; missing keys: {}", missing.join(", ")));
    }
    checks.push(check("providers", status, detail));
}

/// Which kind of LLM backend a spec names; harness specs are probed by
/// spawning `--version`, ollama over HTTP, the rest by key presence.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LlmKind {
    Harness(&'static str),
    Ollama,
    Keyed,
    Unset,
}

pub(crate) fn llm_kind(backend: Option<&str>) -> LlmKind {
    let Some(b) = backend.map(str::trim).filter(|s| !s.is_empty()) else {
        return LlmKind::Unset;
    };
    match b {
        "pi" => LlmKind::Harness("pi"),
        "claude" => LlmKind::Harness("claude"),
        "codex" => LlmKind::Harness("codex"),
        "ollama" => LlmKind::Ollama,
        other => {
            let head = other.split(':').next().unwrap_or_default();
            if head == "ollama" {
                LlmKind::Ollama
            } else {
                LlmKind::Keyed
            }
        }
    }
}

async fn run_llm_checks(cfg: &Config, checks: &mut Vec<Check>) {
    let backend: Option<String> = cfg
        .llm
        .backend
        .clone()
        .or_else(|| std::env::var("VGR_LLM_BACKEND").ok());
    match llm_kind(backend.as_deref()) {
        LlmKind::Unset => checks.push(check(
            "llm",
            Status::Warn,
            "no LLM backend configured; `vygr research` needs --llm (search still works)",
        )),
        LlmKind::Harness(bin) => match probe_binary(bin).await {
            Some(version) => checks.push(check("llm", Status::Ok, format!("{bin}: {version}"))),
            None => checks.push(check(
                "llm",
                Status::Fail,
                format!("{bin} not found on PATH; `--llm {bin}` would fail"),
            )),
        },
        LlmKind::Ollama => {
            let base = cfg
                .llm
                .base_url
                .clone()
                .unwrap_or_else(|| "http://localhost:11434".to_string());
            let url = format!("{}/api/tags", base.trim_end_matches('/'));
            match reqwest::get(&url).await {
                Ok(resp) if resp.status().is_success() => {
                    let models = resp
                        .json::<serde_json::Value>()
                        .await
                        .ok()
                        .and_then(|v| v["models"].as_array().cloned())
                        .map(|m| m.len())
                        .unwrap_or(0);
                    checks.push(check(
                        "llm",
                        Status::Ok,
                        format!("ollama reachable at {base} ({models} models)"),
                    ));
                }
                _ => checks.push(check(
                    "llm",
                    Status::Fail,
                    format!("ollama not reachable at {base}; is `ollama serve` running?"),
                )),
            }
        }
        LlmKind::Keyed => {
            let key_env = cfg.llm.api_key_env.clone();
            match key_env {
                Some(name) => {
                    let present = std::env::var(&name)
                        .map(|v| !v.trim().is_empty())
                        .unwrap_or(false);
                    let status = if present { Status::Ok } else { Status::Fail };
                    checks.push(check(
                        "llm",
                        status,
                        format!(
                            "key env {name} is {}{}",
                            if present { "set" } else { "NOT set" },
                            " (endpoint itself not probed)"
                        ),
                    ));
                }
                None => checks.push(check(
                    "llm",
                    Status::Warn,
                    format!(
                        "backend '{}' selected; pass --llm explicitly to test it",
                        backend.unwrap_or_default()
                    ),
                )),
            }
        }
    }
}

/// Spawn `<bin> --version`, returning its trimmed stdout (or None).
async fn probe_binary(bin: &str) -> Option<String> {
    let out = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::process::Command::new(bin).arg("--version").output(),
    )
    .await
    .ok()?
    .ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let text = if text.is_empty() {
        String::from_utf8_lossy(&out.stderr).trim().to_string()
    } else {
        text
    };
    if out.status.success() && !text.is_empty() {
        Some(text.lines().next().unwrap_or("").to_string())
    } else {
        None
    }
}

/// Write-and-delete probe without creating directories as a side effect:
/// existing dirs are probed directly, missing ones via their nearest
/// existing ancestor (vygr creates the full path on first run).
fn probe_dir_writable(dir: &Path) -> Result<(), String> {
    let probe_in = if dir.exists() {
        dir.to_path_buf()
    } else {
        let mut ancestor = dir.parent();
        loop {
            match ancestor {
                Some(p) if p.exists() => break p.to_path_buf(),
                Some(p) => ancestor = p.parent(),
                None => {
                    return Err(format!(
                        "{} does not exist and has no existing ancestor",
                        dir.display()
                    ))
                }
            }
        }
    };
    let probe = probe_in.join(format!(".vygr-doctor-{}", std::process::id()));
    std::fs::write(&probe, b"probe").map_err(|e| format!("{}: {e}", probe_in.display()))?;
    std::fs::remove_file(&probe).map_err(|e| format!("cleanup: {e}"))?;
    Ok(())
}

fn run_fs_checks(cfg: &Config, checks: &mut Vec<Check>) {
    let cache = Config::cache_dir();
    match cache {
        Some(dir) => match probe_dir_writable(&dir) {
            Ok(()) => checks.push(check("cache_dir", Status::Ok, dir.display().to_string())),
            Err(e) => checks.push(check("cache_dir", Status::Fail, e)),
        },
        None => checks.push(check(
            "cache_dir",
            Status::Fail,
            "cannot determine cache dir",
        )),
    }

    let run_dir = cfg
        .research
        .run_dir
        .clone()
        .unwrap_or_else(|| "./agents/voyager".to_string());
    let run_path = Path::new(&run_dir);
    match probe_dir_writable(run_path) {
        Ok(()) if run_path.exists() => checks.push(check("run_dir", Status::Ok, run_dir.clone())),
        Ok(()) => checks.push(check(
            "run_dir",
            Status::Ok,
            format!("{run_dir} (created on first run)"),
        )),
        Err(e) => checks.push(check("run_dir", Status::Fail, e)),
    }
}

fn run_skill_checks(checks: &mut Vec<Check>) {
    const PACKAGED: &str = include_str!("../../skill/SKILL.md");
    let Some(home) = dirs::home_dir() else {
        checks.push(check(
            "skill",
            Status::Warn,
            "cannot determine home directory",
        ));
        return;
    };
    let mut installed = Vec::new();
    let mut drifted = Vec::new();
    for (id, dir) in crate::commands::init::candidate_skill_dirs(&home) {
        let path = dir.join("SKILL.md");
        if !path.exists() {
            continue;
        }
        match std::fs::read_to_string(&path) {
            Ok(contents) if contents == PACKAGED => installed.push(id),
            Ok(_) => drifted.push(id),
            Err(_) => drifted.push(id),
        }
    }
    if !installed.is_empty() {
        checks.push(check(
            "skill",
            Status::Ok,
            format!("up to date for: {}", installed.join(", ")),
        ));
    }
    if !drifted.is_empty() {
        checks.push(check(
            "skill",
            Status::Warn,
            format!(
                "drifted for: {}; refresh with `vygr init --agent <id>`",
                drifted.join(", ")
            ),
        ));
    }
    if installed.is_empty() && drifted.is_empty() {
        checks.push(check(
            "skill",
            Status::Warn,
            "not installed for any known harness; run `vygr init --agent <id>`",
        ));
    }
}

fn run_catalog_check(checks: &mut Vec<Check>) {
    let Some(path) = Config::cache_dir().map(|d| d.join("models-dev.json")) else {
        return;
    };
    if !path.exists() {
        checks.push(check(
            "models_catalog",
            Status::Warn,
            "not cached yet; first `vygr models` downloads it (~once/24h)",
        ));
        return;
    }
    let age = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .map(|d| d.as_secs() / 3600)
        .unwrap_or(0);
    checks.push(check(
        "models_catalog",
        Status::Ok,
        format!("cached {}h ago ({})", age, path.display()),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn llm_kind_classifies_specs() {
        assert_eq!(llm_kind(None), LlmKind::Unset);
        assert_eq!(llm_kind(Some("")), LlmKind::Unset);
        assert_eq!(llm_kind(Some("pi")), LlmKind::Harness("pi"));
        assert_eq!(llm_kind(Some("claude")), LlmKind::Harness("claude"));
        assert_eq!(llm_kind(Some("codex")), LlmKind::Harness("codex"));
        assert_eq!(llm_kind(Some("ollama")), LlmKind::Ollama);
        assert_eq!(llm_kind(Some("ollama:qwen3:8b")), LlmKind::Ollama);
        assert_eq!(llm_kind(Some("openrouter:x/y")), LlmKind::Keyed);
        assert_eq!(llm_kind(Some("openai-compat")), LlmKind::Keyed);
    }

    #[test]
    fn writability_probe_finds_unwritable_dirs() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert!(probe_dir_writable(tmp.path()).is_ok());
        // A missing dir whose parent exists probes via the parent.
        assert!(probe_dir_writable(&tmp.path().join("sub/deeper")).is_ok());
        // A path under a regular file fails instead of exploding.
        let file = tmp.path().join("f");
        std::fs::write(&file, b"x").unwrap();
        assert!(probe_dir_writable(&file.join("nope")).is_err());
    }
}
