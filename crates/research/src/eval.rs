//! Deterministic run scorecard (ADR-0017): measure the evidence quality
//! of a finished run without an LLM judge. Every component is
//! reproducible from the run's own artifacts; the composite score is a
//! transparent weighted mean, not a benchmark.

use std::collections::HashSet;
use std::path::Path;

use serde::Serialize;
use vygr_core::VygrError;

use crate::orchestrator::Source;
use crate::runs::RunManifest;
use crate::verify;

#[derive(Debug, Serialize)]
pub struct EvalReport {
    pub run: String,
    pub query: Option<String>,
    /// Share of `[n]` citations in answer.md that resolve into
    /// sources.json (100 when nothing is cited).
    pub citation_resolvable_pct: f64,
    pub verification_checked: usize,
    pub verification_weak: usize,
    /// plan.json sub-queries with at least one plausibly matching source.
    pub subquery_coverage_pct: f64,
    /// At least one sub-query looked for disconfirming evidence.
    pub disconfirmation_attempted: bool,
    pub diversity: Diversity,
    /// 0.4 * resolvability + 0.3 * coverage + 0.3 * diversity, penalized
    /// by the weak-claim share (max -50%).
    pub score_pct: f64,
}

#[derive(Debug, Serialize)]
pub struct Diversity {
    pub sources: usize,
    pub distinct_hosts: usize,
    pub syndication_clusters: usize,
}

const DISCONFIRM_MARKERS: &[&str] = &[
    "problem",
    "criticism",
    "critique",
    "fail",
    "deprecat",
    "limitation",
    "risk",
    "against",
    "downside",
    "drawback",
    "wrong",
    "myth",
    "overhype",
    "abandon",
];

/// Evaluate the run directory. Requires `sources.json`; `answer.md` and
/// `plan.json` improve the scorecard when present.
pub fn evaluate(run_dir: &Path) -> Result<EvalReport, VygrError> {
    let run_name = run_dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let sources: Vec<Source> = read_json(&run_dir.join("sources.json"))?;
    let manifest: Option<RunManifest> = read_json(&run_dir.join("run.json")).ok();
    let answer = std::fs::read_to_string(run_dir.join("answer.md")).ok();

    // Citation resolvability + lexical verification.
    let (resolvable_pct, verification) = match &answer {
        Some(answer) => {
            let total = count_citations(answer);
            let resolvable = count_resolvable_citations(answer, sources.len());
            let pct = if total == 0 {
                100.0
            } else {
                resolvable as f64 / total as f64 * 100.0
            };
            (pct, Some(verify::verify(answer, &sources)))
        }
        None => (0.0, None),
    };

    // Sub-query coverage: a sub-query counts as covered when at least one
    // source shares a third of its content words.
    let plan_subqueries: Vec<String> = manifest
        .as_ref()
        .map(|m| m.subqueries.clone())
        .unwrap_or_default();
    let source_words: Vec<HashSet<String>> = sources
        .iter()
        .map(|s| {
            let mut text = format!("{} {}", s.title, s.snippet);
            if let Some(c) = &s.content {
                text.push(' ');
                text.push_str(c);
            }
            content_words(&text)
        })
        .collect();
    let covered = plan_subqueries
        .iter()
        .filter(|q| covers(q, &source_words))
        .count();
    let coverage_pct = if plan_subqueries.is_empty() {
        100.0
    } else {
        covered as f64 / plan_subqueries.len() as f64 * 100.0
    };
    let disconfirmation = plan_subqueries.iter().any(|q| {
        let q = q.to_lowercase();
        DISCONFIRM_MARKERS.iter().any(|m| q.contains(m))
    });

    // Diversity: distinct hosts and syndication clusters.
    let mut hosts: HashSet<String> = HashSet::new();
    let mut clusters: HashSet<u32> = HashSet::new();
    for s in &sources {
        let host = vygr_providers::host_of(&s.url);
        if !host.is_empty() {
            hosts.insert(host);
        }
        if let Some(c) = s.cluster_id {
            clusters.insert(c);
        }
    }
    let diversity = Diversity {
        sources: sources.len(),
        distinct_hosts: hosts.len(),
        syndication_clusters: clusters.len(),
    };
    let diversity_pct = if sources.is_empty() {
        0.0
    } else {
        (hosts.len() as f64 / sources.len() as f64 * 100.0).min(100.0)
    };

    let weak = verification.as_ref().map_or(0, |v| v.weak_count());
    let checked = verification.as_ref().map_or(0, |v| v.checked);
    let weak_share = if checked == 0 {
        0.0
    } else {
        weak as f64 / checked as f64
    };
    let score = 0.4 * resolvable_pct + 0.3 * coverage_pct + 0.3 * diversity_pct;
    let score = score * (1.0 - (weak_share * 0.5).min(0.5));

    Ok(EvalReport {
        run: run_name,
        query: manifest.map(|m| m.query),
        citation_resolvable_pct: round1(resolvable_pct),
        verification_checked: checked,
        verification_weak: weak,
        subquery_coverage_pct: round1(coverage_pct),
        disconfirmation_attempted: disconfirmation,
        diversity,
        score_pct: round1(score),
    })
}

/// A sub-query is "covered" when some source contains at least a third of
/// the sub-query's content words (and at least one word).
fn covers(subquery: &str, source_words: &[HashSet<String>]) -> bool {
    let query_words = content_words(subquery);
    if query_words.is_empty() {
        return false;
    }
    let words: Vec<&String> = query_words.iter().collect();
    let needed = (words.len() / 3).max(1);
    source_words
        .iter()
        .any(|s| words.iter().filter(|w| s.contains(**w)).count() >= needed)
}

fn count_citations(answer: &str) -> usize {
    parse_citations(answer).count()
}

fn count_resolvable_citations(answer: &str, source_count: usize) -> usize {
    parse_citations(answer)
        .filter(|n| (1..=source_count).contains(n))
        .count()
}

fn parse_citations(answer: &str) -> impl Iterator<Item = usize> + '_ {
    answer.match_indices('[').filter_map(|(i, _)| {
        let rest = &answer[i + 1..];
        let end = rest.find(']')?;
        rest[..end].trim().parse::<usize>().ok()
    })
}

fn content_words(text: &str) -> HashSet<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| t.len() >= 3)
        .map(str::to_string)
        .collect()
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, VygrError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| VygrError::Config(format!("{}: {e}", path.display())))?;
    serde_json::from_str(&text).map_err(VygrError::Json)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_run(dir: &Path, sources: &str, answer: &str, subqueries: &[&str]) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("sources.json"), sources).unwrap();
        std::fs::write(dir.join("answer.md"), answer).unwrap();
        std::fs::write(
            dir.join("run.json"),
            serde_json::to_string(&RunManifest {
                query: "q".into(),
                depth_used: 2,
                iterations: 2,
                subqueries: subqueries.iter().map(|s| s.to_string()).collect(),
                reflections: vec![],
                provider_spec: "ddgs".into(),
                llm: "t".into(),
                parent: None,
            })
            .unwrap(),
        )
        .unwrap();
    }

    fn source(title: &str, url: &str, content: &str) -> String {
        serde_json::json!({
            "title": title, "url": url, "snippet": "", "provider": "ddgs",
            "score": 1.0, "level": 0, "content": content
        })
        .to_string()
    }

    #[test]
    fn a_clean_run_scores_high() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("run-a");
        let sources = format!(
            "[{},{}]",
            source(
                "rust bench",
                "https://a.io/1",
                "rust webassembly benchmark numbers for engines"
            ),
            source(
                "zig bench",
                "https://b.io/2",
                "zig webassembly benchmark numbers comparison"
            )
        );
        write_run(
            &dir,
            &sources,
            "## Summary\n\nRust posts strong webassembly benchmark numbers [1]. \
Zig posts a close webassembly benchmark comparison [2].\n",
            &["rust wasm bench", "zig wasm criticism"],
        );
        let report = evaluate(&dir).unwrap();
        assert_eq!(report.citation_resolvable_pct, 100.0);
        assert_eq!(report.verification_weak, 0);
        assert!(report.subquery_coverage_pct >= 50.0);
        assert!(report.disconfirmation_attempted);
        assert_eq!(report.diversity.distinct_hosts, 2);
        assert!(report.score_pct > 70.0, "score was {}", report.score_pct);
    }

    #[test]
    fn fabricated_citations_and_weak_claims_hurt() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("run-b");
        let sources = format!("[{}]", source("a", "https://a.io/1", "alpha beta gamma"));
        write_run(
            &dir,
            &sources,
            "Totally unrelated claim about quantum moon bases [1]. Nonexistent source [7].\n",
            &["completely unrelated subquery about cheese"],
        );
        let report = evaluate(&dir).unwrap();
        assert!(report.citation_resolvable_pct < 100.0);
        assert_eq!(report.diversity.distinct_hosts, 1);
        assert!(report.subquery_coverage_pct < 100.0);
        assert!(report.score_pct < 70.0, "score was {}", report.score_pct);
    }

    #[test]
    fn missing_answer_scores_zero_resolvability() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("run-c");
        write_run(&dir, "[]", "", &[]);
        std::fs::remove_file(dir.join("answer.md")).unwrap();
        let report = evaluate(&dir).unwrap();
        assert_eq!(report.citation_resolvable_pct, 0.0);
        assert_eq!(report.verification_checked, 0);
    }
}
