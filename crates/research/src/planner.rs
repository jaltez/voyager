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
            max_tokens: Some(600),
            temperature: Some(0.2),
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
}
