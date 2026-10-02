//! Run management (T2): list past runs, inspect one, and re-synthesize a
//! finished run offline from its saved sources and reflections. The
//! `run.json` manifest (written by the orchestrator since 0.5.0) carries
//! everything resynthesis needs; runs from older versions cannot resume.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use vygr_core::VygrError;
use vygr_llm::{ChatMessage, CompletionRequest, LlmClient};

use crate::orchestrator::{Source, SYNTHESIS_SYSTEM};
use crate::verify;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunManifest {
    pub query: String,
    pub depth_used: u32,
    pub iterations: u32,
    pub subqueries: Vec<String>,
    #[serde(default)]
    pub reflections: Vec<String>,
    pub provider_spec: String,
    pub llm: String,
}

#[derive(Debug, Serialize)]
pub struct RunInfo {
    /// Directory name (timestamp-slug).
    pub name: String,
    pub query: Option<String>,
    pub has_answer: bool,
    pub sources: usize,
}

/// Default base directory for run artifacts.
pub fn default_base() -> PathBuf {
    PathBuf::from("agents").join("voyager")
}

/// List runs under `base`, newest first.
pub fn list_runs(base: &Path) -> Vec<RunInfo> {
    let mut out: Vec<RunInfo> = std::fs::read_dir(base)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .filter_map(|dir| {
            let name = dir.file_name()?.to_string_lossy().to_string();
            let sources = std::fs::read_to_string(dir.join("sources.json"))
                .ok()
                .and_then(|s| serde_json::from_str::<Vec<Source>>(&s).ok())
                .map(|v| v.len())
                .unwrap_or(0);
            let query = std::fs::read_to_string(dir.join("run.json"))
                .ok()
                .and_then(|s| serde_json::from_str::<RunManifest>(&s).ok())
                .map(|m| m.query);
            Some(RunInfo {
                name,
                query,
                has_answer: dir.join("answer.md").exists(),
                sources,
            })
        })
        .collect();
    out.sort_by(|a, b| b.name.cmp(&a.name));
    out
}

/// Resolve a run directory by name or unambiguous name prefix under
/// `base`.
pub fn resolve_run(base: &Path, name: &str) -> Result<PathBuf, VygrError> {
    let runs = list_runs(base);
    let matches: Vec<&RunInfo> = runs
        .iter()
        .filter(|r| r.name == name || r.name.starts_with(name))
        .collect();
    match matches.as_slice() {
        [one] => Ok(base.join(&one.name)),
        [] => Err(VygrError::Config(format!(
            "no run matches '{name}' under {}",
            base.display()
        ))),
        many => Err(VygrError::Config(format!(
            "'{name}' is ambiguous: {}",
            many.iter()
                .map(|r| r.name.clone())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

/// Re-synthesize a finished run from its stored artifacts: sources,
/// reflections and the manifest query, no network searches involved
/// (offline replay of the synthesis stage).
pub async fn resynthesize(
    run_dir: &Path,
    llm: Box<dyn LlmClient>,
    context_max_chars: usize,
) -> Result<String, VygrError> {
    let manifest: RunManifest = read_json(&run_dir.join("run.json"))?;
    let sources: Vec<Source> = read_json(&run_dir.join("sources.json"))
        .map_err(|e| VygrError::Config(format!("sources.json: {e}")))?;

    let context = crate::orchestrator::assemble_context_public(&sources, context_max_chars);
    let user = format!(
        "Question: {}\n\nSub-queries explored:\n{}\n\nResearch notes:\n{}\n\nEvidence blocks:\n{}",
        manifest.query,
        manifest
            .subqueries
            .iter()
            .map(|s| format!("- {s}"))
            .collect::<Vec<_>>()
            .join("\n"),
        if manifest.reflections.is_empty() {
            "(none)".to_string()
        } else {
            manifest
                .reflections
                .iter()
                .map(|n| format!("- {n}"))
                .collect::<Vec<_>>()
                .join("\n")
        },
        context,
    );
    let resp = llm
        .complete(&CompletionRequest {
            messages: vec![
                ChatMessage::system(SYNTHESIS_SYSTEM),
                ChatMessage::user(user),
            ],
            max_tokens: Some(4_096),
            temperature: Some(0.3),
            json_object: false,
        })
        .await?;
    let mut answer = resp.content.trim().to_string();
    let verification = verify::verify(&answer, &sources);
    if !verification.is_clean() {
        answer.push_str(&verify::verification_section(&verification));
    }
    std::fs::write(run_dir.join("answer.md"), &answer).map_err(VygrError::Io)?;
    Ok(answer)
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, VygrError> {
    let text = std::fs::read_to_string(path).map_err(VygrError::Io)?;
    serde_json::from_str(&text).map_err(VygrError::Json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_and_resolves_runs() {
        let tmp = tempfile::TempDir::new().unwrap();
        let base = tmp.path();
        for name in ["20260101-120000-alpha-query", "20260101-130000-beta-query"] {
            let dir = base.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("sources.json"), "[]").unwrap();
        }
        std::fs::write(
            base.join("20260101-120000-alpha-query/run.json"),
            serde_json::to_string(&RunManifest {
                query: "the alpha question".into(),
                depth_used: 2,
                iterations: 2,
                subqueries: vec!["a".into()],
                reflections: vec![],
                provider_spec: "ddgs".into(),
                llm: "test".into(),
            })
            .unwrap(),
        )
        .unwrap();

        let runs = list_runs(base);
        assert_eq!(runs.len(), 2);
        // Newest first.
        assert!(runs[0].name.starts_with("20260101-13"));
        assert_eq!(runs[1].query.as_deref(), Some("the alpha question"));

        let dir = resolve_run(base, "20260101-120000-a").unwrap();
        assert!(dir.to_string_lossy().contains("alpha"));
        assert!(resolve_run(base, "2026").is_err()); // ambiguous
        assert!(resolve_run(base, "nope").is_err());
    }
}
