//! Query planner: turns the research question into a bounded set of
//! sub-queries (generated up-front — the determinism insight from GPT
//! Researcher) and lets the LLM pick a depth within the configured range.

use serde::Deserialize;
use vygr_core::VygrError;
use vygr_llm::{ChatMessage, CompletionRequest, LlmClient};

use crate::DepthSpec;

#[derive(Debug, Deserialize)]
struct PlanJson {
    subqueries: Vec<String>,
    #[serde(default)]
    depth: Option<u32>,
}

/// Ask the LLM for sub-queries and a depth. Falls back to the raw query if
/// the model returns nothing usable (the run degrades, it does not fail).
pub async fn plan(
    llm: &dyn LlmClient,
    query: &str,
    breadth: u32,
    depth: &DepthSpec,
) -> Result<(Vec<String>, u32, Option<f64>), VygrError> {
    let system = "You are a research planner. Decompose the research question into diverse, \
searchable sub-queries covering different framings of the question, including at least one \
disconfirming or counter-evidence query. Sub-queries must be self-contained keyword-style \
web searches, not questions requiring prior context.";
    let user = format!(
        "Research question: {query}\n\nProduce at most {breadth} sub-queries. \
Choose an integer depth (number of research passes) between {min} and {max} matching the \
question's complexity.\n\nRespond with ONLY minified JSON, no prose, in the shape \
{{\"subqueries\":[\"...\"],\"depth\":N}}",
        min = depth.min,
        max = depth.max,
    );

    let resp = llm
        .complete(&CompletionRequest {
            messages: vec![ChatMessage::system(system), ChatMessage::user(user)],
            max_tokens: Some(2_500),
            temperature: Some(0.2),
            json_object: false,
        })
        .await?;

    let cost = resp.cost_usd;
    let plan: PlanJson = parse_json_object(&resp.content)?;

    let mut subqueries: Vec<String> = plan
        .subqueries
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    subqueries.dedup();
    if subqueries.is_empty() {
        subqueries.push(query.trim().to_string());
    }
    subqueries.truncate(breadth.max(1) as usize);

    let chosen = plan.depth.unwrap_or(depth.min).clamp(depth.min, depth.max);
    Ok((subqueries, chosen, cost))
}

/// Extract the first JSON object from a (possibly fenced / chatty) reply.
pub fn parse_json_object<T: serde::de::DeserializeOwned>(raw: &str) -> Result<T, VygrError> {
    let start = raw.find('{');
    let end = raw.rfind('}');
    match (start, end) {
        (Some(s), Some(e)) if e > s => serde_json::from_str(&raw[s..=e])
            .map_err(|e| VygrError::Parse(format!("planner JSON: {e}"))),
        _ => Err(VygrError::Parse(format!(
            "no JSON object in planner reply: '{}'",
            raw.chars().take(120).collect::<String>()
        ))),
    }
}

/// Output of a reflection pass (ADR-0012): distilled notes feed later
/// stages; follow-up queries target the gaps found in the new evidence.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Reflection {
    #[serde(default)]
    pub notes: Vec<String>,
    #[serde(default)]
    pub followups: Vec<String>,
    #[serde(default)]
    pub satisfied: bool,
}

const MAX_NOTES: usize = 8;
const MAX_FOLLOWUPS: usize = 3;

/// Ask the LLM to distill the new evidence and propose follow-up searches.
/// Only the returned notes (never the raw digests) feed later stages.
pub async fn reflect(
    llm: &dyn LlmClient,
    question: &str,
    notes: &[String],
    evidence_digest: &str,
) -> Result<(Reflection, Option<f64>), VygrError> {
    let system = "You are a research reflection agent. You receive a research question, \
distilled notes accumulated so far, and digests of newly gathered evidence. Distill durable, \
non-redundant facts into short notes and identify concrete gaps: missing data, contradictions \
between sources, unverified claims. Respond with ONLY minified JSON in the shape \
{\"notes\":[\"...\"],\"followups\":[\"...\"],\"satisfied\":false} with at most 8 notes \
(each under 200 characters), 1-3 followup keyword-style web searches targeting the gaps \
(empty array when satisfied), and satisfied=true only when the question is fully answered \
by the evidence.";
    let user = format!(
        "Question: {question}\n\nNotes so far:\n{}\n\nNew evidence digests:\n{evidence_digest}",
        if notes.is_empty() {
            "(none)".to_string()
        } else {
            notes
                .iter()
                .map(|n| format!("- {n}"))
                .collect::<Vec<_>>()
                .join("\n")
        }
    );

    let resp = llm
        .complete(&CompletionRequest {
            messages: vec![ChatMessage::system(system), ChatMessage::user(user)],
            max_tokens: Some(2_000),
            temperature: Some(0.2),
            json_object: false,
        })
        .await?;
    let cost = resp.cost_usd;

    let mut reflection: Reflection = parse_json_object(&resp.content)?;
    reflection.notes = clean_items(reflection.notes, MAX_NOTES, 200);
    if reflection.satisfied {
        reflection.followups.clear();
    } else {
        reflection.followups = clean_items(reflection.followups, MAX_FOLLOWUPS, 200);
    }
    Ok((reflection, cost))
}

/// Trim, drop empties/duplicates and cap a list of model-produced strings.
fn clean_items(items: Vec<String>, max: usize, max_chars: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in items {
        let item = item.trim().to_string();
        if item.is_empty() || out.contains(&item) {
            continue;
        }
        out.push(item.chars().take(max_chars).collect());
        if out.len() >= max {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_json_from_fenced_reply() {
        let plan: PlanJson = parse_json_object(
            "Here you go:\n```json\n{\"subqueries\":[\"a\",\"b\"],\"depth\":3}\n```",
        )
        .unwrap();
        assert_eq!(plan.subqueries, vec!["a", "b"]);
        assert_eq!(plan.depth, Some(3));
    }

    #[test]
    fn rejects_prose_without_json() {
        assert!(parse_json_object::<PlanJson>("no json here").is_err());
    }

    #[test]
    fn reflection_parses_and_satisfied_clears_followups() {
        let r: Reflection = parse_json_object(
            "```json\n{\"notes\":[\"fact\"],\"followups\":[\"q1\"],\"satisfied\":true}\n```",
        )
        .unwrap();
        assert_eq!(r.notes, vec!["fact"]);
        assert!(r.satisfied);
        // The orchestrator clears followups for satisfied reflections.
        let mut r = r;
        if r.satisfied {
            r.followups.clear();
        }
        assert!(r.followups.is_empty());
    }

    #[test]
    fn clean_items_dedupes_trims_and_caps() {
        let items = vec![
            "  a  ".to_string(),
            "a".to_string(),
            "".to_string(),
            "b".to_string(),
            "c".to_string(),
        ];
        let cleaned = clean_items(items, 2, 200);
        assert_eq!(cleaned, vec!["a", "b"]);
        let long = clean_items(vec!["x".repeat(300)], 8, 200);
        assert_eq!(long[0].len(), 200);
    }
}
