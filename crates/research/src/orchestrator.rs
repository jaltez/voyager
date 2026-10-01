//! Orchestrates one research run: plan -> search fan-out -> fetch ->
//! score -> synthesize, leaving an artifact trail on disk (ADR-0005,
//! ADR-0007, ADR-0009).
//!
//! The scaffold implements a single full pass; iterating with distilled
//! reflections across depth levels is roadmap phase 2.

use std::path::PathBuf;

use futures::future::join_all;
use serde::Serialize;
use vygr_core::provider::{FetchProvider, SearchQuery};
use vygr_core::types::SearchResult;
use vygr_core::VygrError;
use vygr_llm::{ChatMessage, CompletionRequest, LlmClient};

use crate::{planner, run_dir, score};

/// Depth bounds for a run; the LLM picks within the range (ADR-0007).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DepthSpec {
    pub min: u32,
    pub max: u32,
}

impl DepthSpec {
    /// Parse `"3"` (fixed) or `"2..4"` / `"2-4"` (range).
    pub fn parse(s: &str) -> Result<Self, VygrError> {
        let bad = || VygrError::Config(format!("invalid depth '{s}' (use \"3\" or \"2..4\")"));
        let s = s.trim();
        if let Some((a, b)) = s.split_once("..").or_else(|| s.split_once('-')) {
            let a: u32 = a.trim().parse().map_err(|_| bad())?;
            let b: u32 = b.trim().parse().map_err(|_| bad())?;
            return Ok(Self {
                min: a.min(b),
                max: a.max(b),
            });
        }
        let v: u32 = s.parse().map_err(|_| bad())?;
        Ok(Self { min: v, max: v })
    }
}

#[derive(Debug, Clone)]
pub struct ResearchRequest {
    pub query: String,
    pub depth: DepthSpec,
    pub breadth: u32,
    pub budget_usd: Option<f64>,
    pub max_results_per_query: usize,
    pub fetch_top: usize,
    pub context_max_chars: usize,
    pub provider_spec: String,
    pub run_dir_base: Option<String>,
    /// Spacing/retry policy applied to every provider in the chain.
    pub politeness: vygr_core::config::PolitenessConf,
}

#[derive(Debug, Clone, Serialize)]
pub struct Source {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub provider: String,
    pub score: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ResearchReport {
    pub query: String,
    pub depth_used: u32,
    pub subqueries: Vec<String>,
    pub sources: Vec<Source>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_dir: Option<String>,
    pub warnings: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    pub llm: String,
}

pub async fn run(
    req: ResearchRequest,
    llm: Box<dyn LlmClient>,
    http: reqwest::Client,
) -> Result<ResearchReport, VygrError> {
    let mut warnings = Vec::new();
    let mut spent = 0.0f64;
    let mut tracked = false;
    let llm_desc = llm.describe();

    // 0) Evidence trail (ADR-0009).
    let dir: PathBuf = run_dir::create_run_dir(req.run_dir_base.as_deref(), &req.query)?;
    let _ = run_dir::write_file(
        &dir,
        "prompt.md",
        &format!(
            "# Research prompt\n\n{}\n\n- provider chain: {}\n- depth: {}..{}\n- breadth: {}\n- budget: {}\n- llm: {}\n",
            req.query,
            req.provider_spec,
            req.depth.min,
            req.depth.max,
            req.breadth,
            req.budget_usd
                .map(|b| format!("${b:.2}"))
                .unwrap_or_else(|| "unbounded".to_string()),
            llm_desc,
        ),
    );

    // 1) Plan: sub-queries generated up-front, depth chosen by the LLM.
    let (subqueries, depth_used, plan_cost) =
        planner::plan(&*llm, &req.query, req.breadth, &req.depth).await?;
    if let Some(c) = plan_cost {
        spent += c;
        tracked = true;
    }
    tracing::info!(depth_used, subqueries = subqueries.len(), "plan ready");
    let _ = run_dir::write_file(
        &dir,
        "plan.json",
        &serde_json::to_string_pretty(&serde_json::json!({
            "subqueries": subqueries,
            "depth_used": depth_used,
        }))
        .unwrap_or_default(),
    );

    // 2) Search fan-out with cross-subquery URL dedup.
    let chain = vygr_providers::build_chain(&req.provider_spec, http.clone(), &req.politeness)?;
    let mut results: Vec<SearchResult> = Vec::new();
    for sq in &subqueries {
        let q = SearchQuery::new(sq.clone(), req.max_results_per_query);
        let (rs, w) = vygr_providers::search_chain(&chain, &q).await;
        warnings.extend(w);
        results.extend(rs);
    }
    let results = vygr_providers::dedup(results);
    if results.is_empty() {
        return Err(VygrError::provider(
            req.provider_spec.clone(),
            "all searches returned nothing",
        ));
    }
    tracing::info!(sources = results.len(), "search done");

    // 3) Fetch the top pages; failures degrade to snippets.
    let fetcher = vygr_providers::HttpFetch::new(http);
    let indexed: Vec<(usize, &SearchResult)> =
        results.iter().enumerate().take(req.fetch_top).collect();
    let pages = join_all(indexed.iter().map(|(i, r)| {
        let url = r.url.clone();
        let idx = *i;
        let fetcher = &fetcher;
        async move {
            match fetcher.fetch(&url, 8_000).await {
                Ok(p) => Ok((idx, p)),
                Err(e) => Err((idx, e)),
            }
        }
    }))
    .await;
    let mut fetched: Vec<Option<String>> = vec![None; results.len()];
    for page in pages {
        match page {
            Ok((i, p)) => fetched[i] = Some(p.truncated(8_000)),
            Err((i, e)) => warnings.push(format!("fetch {}: {e}", results[i].url)),
        }
    }

    // 4) Score sources against the question + sub-queries (ADR-0006).
    let mut query_terms = score::terms(&req.query);
    for sq in &subqueries {
        query_terms.extend(score::terms(sq));
    }
    let mut sources: Vec<Source> = results
        .into_iter()
        .zip(fetched)
        .map(|(r, content)| {
            let mut doc = format!("{} {}", r.title, r.snippet);
            if let Some(c) = &content {
                doc.push(' ');
                doc.extend(c.chars().take(2_000));
            }
            Source {
                title: r.title,
                url: r.url,
                snippet: r.snippet,
                provider: r.providers.join("+"),
                score: score::relevance(&query_terms, &doc),
                content,
            }
        })
        .collect();
    sources.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let sources_json = serde_json::to_string_pretty(&sources).unwrap_or_default();
    let _ = run_dir::write_file(&dir, "sources.json", &sources_json);

    // 5) Coarse budget guard before the expensive call (ADR-0007).
    if let Some(budget) = req.budget_usd {
        if tracked && spent > budget {
            warnings.push(format!(
                "budget ${budget:.2} exhausted before synthesis (spent {:.2}); returning sources only",
                spent
            ));
            return Ok(ResearchReport {
                query: req.query,
                depth_used,
                subqueries,
                sources,
                answer: None,
                run_dir: Some(dir.display().to_string()),
                warnings,
                cost_usd: if tracked { Some(spent) } else { None },
                llm: llm_desc,
            });
        }
    }

    // 6) Assemble a bounded context window.
    let mut context = String::new();
    let mut used = 0usize;
    for (i, s) in sources.iter().enumerate() {
        if used >= req.context_max_chars {
            break;
        }
        let excerpt: String = match &s.content {
            Some(c) => c.chars().take(2_000).collect(),
            None => s.snippet.clone(),
        };
        if excerpt.is_empty() {
            continue;
        }
        let block = format!("[{}] {} — {}\n{}\n\n", i + 1, s.title, s.url, excerpt);
        if used + block.len() > req.context_max_chars {
            break;
        }
        used += block.len();
        context.push_str(&block);
    }

    // 7) Synthesize the report.
    let system = "You are a senior research analyst. Write a focused markdown report answering the question. \
Cite sources inline with bracketed numbers like [1] referring only to the numbered evidence blocks provided. \
Prefer primary sources. If evidence blocks disagree, reconcile the disagreement explicitly instead of \
averaging incompatible figures. End with a '## Caveats & open questions' section listing what could not \
be verified from the provided evidence.";
    let user = format!(
        "Question: {}\n\nSub-queries explored:\n{}\n\nEvidence blocks:\n{}",
        req.query,
        subqueries
            .iter()
            .map(|s| format!("- {s}"))
            .collect::<Vec<_>>()
            .join("\n"),
        context
    );
    let resp = llm
        .complete(&CompletionRequest {
            messages: vec![ChatMessage::system(system), ChatMessage::user(user)],
            max_tokens: Some(4_096),
            temperature: Some(0.3),
        })
        .await?;
    if let Some(c) = resp.cost_usd {
        spent += c;
        tracked = true;
    }
    let answer = resp.content.trim().to_string();
    let _ = run_dir::write_file(&dir, "answer.md", &answer);

    Ok(ResearchReport {
        query: req.query,
        depth_used,
        subqueries,
        sources,
        answer: Some(answer),
        run_dir: Some(dir.display().to_string()),
        warnings,
        cost_usd: if tracked { Some(spent) } else { None },
        llm: llm_desc,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fixed_and_range_depth() {
        assert_eq!(DepthSpec::parse("3").unwrap(), DepthSpec { min: 3, max: 3 });
        assert_eq!(
            DepthSpec::parse("2..4").unwrap(),
            DepthSpec { min: 2, max: 4 }
        );
        assert_eq!(
            DepthSpec::parse("4-2").unwrap(),
            DepthSpec { min: 2, max: 4 }
        );
        assert!(DepthSpec::parse("deep").is_err());
    }
}
