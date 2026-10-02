//! Orchestrates research runs (ADR-0005, ADR-0012): plan-then-execute with
//! iterative depth. Each level searches its sub-queries, fetches and scores
//! new sources, then a reflection pass distills notes and generates
//! follow-up queries for the next level; only distilled notes feed later
//! stages (Tavily lesson). Breadth halves per level (GPT Researcher model)
//! and LLM costs are accumulated, with the budget guard running before
//! every expensive call.

use std::collections::HashSet;
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
    /// Politeness, cache and per-provider settings for the search stack.
    pub stack: vygr_core::config::SearchStackConf,
    /// Filters applied to every research search (M1.3).
    pub time_range: Option<vygr_core::provider::TimeRange>,
    pub include_domains: Vec<String>,
    pub exclude_domains: Vec<String>,
    /// Raw JSON Schema the synthesized answer must satisfy (M2.5). When
    /// set, the answer is requested as JSON and validated (parse-level).
    pub output_schema: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Source {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub provider: String,
    pub score: f32,
    /// Depth level at which this source was gathered.
    pub level: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ResearchReport {
    pub query: String,
    pub depth_used: u32,
    /// Levels actually executed (may be lower than depth_used when the
    /// reflection runs out of follow-ups or the budget runs out).
    pub iterations: u32,
    pub subqueries: Vec<String>,
    pub sources: Vec<Source>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_dir: Option<String>,
    pub warnings: Vec<String>,
    /// Distilled notes accumulated across reflection passes.
    pub reflections: Vec<String>,
    pub budget_exhausted: bool,
    /// Whether the synthesized answer parsed as JSON when an output
    /// schema was requested; `None` when no schema was given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema_valid: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    pub llm: String,
}

const SYNTHESIS_SYSTEM: &str = "You are a senior research analyst. Write a focused markdown report answering the question. \
Cite sources inline with bracketed numbers like [1] referring only to the numbered evidence blocks provided. \
Prefer primary sources. Use the research notes as guidance but verify claims against the evidence blocks. \
If evidence blocks disagree, reconcile the disagreement explicitly instead of \
averaging incompatible figures. End with a '## Caveats & open questions' section listing what could not \
be verified from the provided evidence.";

pub async fn run(
    req: ResearchRequest,
    llm: Box<dyn LlmClient>,
    http: reqwest::Client,
) -> Result<ResearchReport, VygrError> {
    let mut warnings = Vec::new();
    let mut spent = 0.0f64;
    let mut tracked = false;
    let llm_desc = llm.describe();
    let over_budget =
        |spent: f64, tracked: bool| tracked && req.budget_usd.is_some_and(|b| spent >= b);

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
    let (initial_queries, depth_used, plan_cost) =
        planner::plan(&*llm, &req.query, req.breadth, &req.depth).await?;
    if let Some(c) = plan_cost {
        spent += c;
        tracked = true;
    }
    tracing::info!(depth_used, subqueries = initial_queries.len(), "plan ready");
    let _ = run_dir::write_file(
        &dir,
        "plan.json",
        &serde_json::to_string_pretty(&serde_json::json!({
            "subqueries": initial_queries,
            "depth_used": depth_used,
        }))
        .unwrap_or_default(),
    );

    // 2) Iterative levels (ADR-0012): search -> fetch -> score -> reflect.
    let handle = vygr_providers::build_chain(
        &req.provider_spec,
        http.clone(),
        &req.stack,
        &vygr_providers::CacheOptions::default(),
    )?;
    let fetcher = vygr_providers::HttpFetch::new(http);

    let mut visited: HashSet<String> = HashSet::new();
    let mut sources: Vec<Source> = Vec::new();
    let mut reflections: Vec<String> = Vec::new();
    let mut next_queries: Vec<String> = initial_queries.clone();
    let mut budget_exhausted = false;
    let mut iterations_done = 0u32;

    for level in 0..depth_used {
        // Breadth halves per level, floored at 2 (GPT Researcher model).
        let level_breadth = (req.breadth >> level).max(2) as usize;
        let queries: Vec<String> = next_queries.iter().take(level_breadth).cloned().collect();
        if queries.is_empty() {
            break;
        }

        // 2a) Search this level's sub-queries.
        let mut level_results: Vec<SearchResult> = Vec::new();
        for sq in &queries {
            let q = SearchQuery::new(sq.clone(), req.max_results_per_query).with_filters(
                req.time_range,
                req.include_domains.clone(),
                req.exclude_domains.clone(),
            );
            let (rs, w) = vygr_providers::search_chain(&handle.providers, &q).await;
            warnings.extend(w);
            level_results.extend(rs);
        }
        let (cache_hits, cache_misses) = handle.cache.snapshot();
        tracing::info!(cache_hits, cache_misses, level, "search cache");

        // 2b) Global URL dedup across levels.
        let fresh: Vec<SearchResult> = vygr_providers::dedup(level_results)
            .into_iter()
            .filter(|r| visited.insert(vygr_providers::url_key(&r.url)))
            .collect();
        if fresh.is_empty() {
            if level == 0 {
                return Err(VygrError::provider(
                    req.provider_spec.clone(),
                    "all searches returned nothing",
                ));
            }
            tracing::info!(level, "no new sources; stopping");
            break;
        }
        tracing::info!(sources = fresh.len(), level, "search done");

        // 2c) Fetch the top fresh pages; failures degrade to snippets.
        let fetch_count = fresh.len().min(req.fetch_top);
        let pages = join_all(fresh.iter().take(fetch_count).enumerate().map(|(i, r)| {
            let url = r.url.clone();
            let fetcher = &fetcher;
            async move {
                fetcher
                    .fetch(&url, 8_000)
                    .await
                    .map(|p| (i, p))
                    .map_err(|e| (i, e))
            }
        }))
        .await;
        let mut fetched: Vec<Option<String>> = vec![None; fresh.len()];
        for page in pages {
            match page {
                Ok((i, p)) => fetched[i] = Some(p.truncated(8_000)),
                Err((i, e)) => warnings.push(format!("fetch {}: {e}", fresh[i].url)),
            }
        }

        // 2d) Score the new sources with BM25 against question + level
        // queries + accumulated notes (ADR-0006).
        let mut query_terms = score::terms(&req.query);
        for sq in &queries {
            query_terms.extend(score::terms(sq));
        }
        for note in &reflections {
            query_terms.extend(score::terms(note));
        }
        let docs: Vec<String> = fresh
            .iter()
            .zip(&fetched)
            .map(|(r, content)| {
                let mut doc = format!("{} {}", r.title, r.snippet);
                if let Some(c) = content {
                    doc.push(' ');
                    doc.extend(c.chars().take(2_000));
                }
                doc
            })
            .collect();
        let index = score::Bm25::build(docs.iter().map(String::as_str));
        let level_sources: Vec<Source> = fresh
            .into_iter()
            .zip(fetched)
            .enumerate()
            .map(|(i, (r, content))| Source {
                title: r.title,
                url: r.url,
                snippet: r.snippet,
                provider: r.providers.join("+"),
                score: index.score(&query_terms, i),
                level,
                content,
            })
            .collect();
        let new_count = level_sources.len();
        sources.extend(level_sources);
        iterations_done = level + 1;
        eprintln!(
            "vygr: level {}/{depth_used}: {new_count} new sources ({} total)",
            level + 1,
            sources.len(),
            depth_used = depth_used
        );

        // 2e) Reflect for the next level (skipped on the last one).
        if level + 1 >= depth_used {
            break;
        }
        if over_budget(spent, tracked) {
            budget_exhausted = true;
            warnings.push(format!(
                "budget ${:.2} reached after level {}; stopping before reflection",
                req.budget_usd.unwrap_or(0.0),
                level + 1
            ));
            break;
        }
        let digest = sources_digest(&sources, new_count);
        match planner::reflect(&*llm, &req.query, &reflections, &digest).await {
            Ok((reflection, cost)) => {
                if let Some(c) = cost {
                    spent += c;
                    tracked = true;
                }
                let has_followups = !reflection.followups.is_empty();
                reflections.extend(reflection.notes);
                let _ = run_dir::write_file(
                    &dir,
                    "reflections.json",
                    &serde_json::to_string_pretty(&serde_json::json!({
                        "notes": reflections,
                        "satisfied": reflection.satisfied,
                        "level": level + 1,
                    }))
                    .unwrap_or_default(),
                );
                if !has_followups {
                    tracing::info!(level, "reflection satisfied; stopping");
                    break;
                }
                next_queries = reflection.followups;
            }
            Err(e) => {
                // A failed reflection ends the loop; the evidence so far
                // still gets synthesized.
                warnings.push(format!("reflection: {e}"));
                break;
            }
        }
    }
    let (cache_hits, cache_misses) = handle.cache.snapshot();
    tracing::info!(
        cache_hits,
        cache_misses,
        iterations = iterations_done,
        "research loop done"
    );

    if sources.is_empty() {
        return Err(VygrError::provider(
            req.provider_spec.clone(),
            "no sources gathered across levels",
        ));
    }

    // 3) Rank everything and persist the evidence (citations [n] index
    // into sources.json order).
    sources.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let sources_json = serde_json::to_string_pretty(&sources).unwrap_or_default();
    let _ = run_dir::write_file(&dir, "sources.json", &sources_json);

    // 4) Budget guard before the expensive call (ADR-0007).
    let mut schema_valid: Option<bool> = None;
    let answer = if over_budget(spent, tracked) {
        budget_exhausted = true;
        warnings.push(format!(
            "budget ${:.2} exhausted before synthesis (spent {spent:.2}); returning sources only",
            req.budget_usd.unwrap_or(0.0)
        ));
        None
    } else {
        match synthesize(&*llm, &req, &initial_queries, &reflections, &sources).await {
            Ok((content, cost)) => {
                if let Some(c) = cost {
                    spent += c;
                    tracked = true;
                }
                let answer = content.trim().to_string();
                if req.output_schema.is_some() {
                    let valid = planner::parse_json_object::<serde_json::Value>(&answer).is_ok();
                    if !valid {
                        warnings.push(
                            "output did not parse as JSON; returning the raw reply \
(response_format is best-effort across backends)"
                                .to_string(),
                        );
                    }
                    schema_valid = Some(valid);
                }
                let _ = run_dir::write_file(&dir, "answer.md", &answer);
                Some(answer)
            }
            Err(e) => {
                warnings.push(format!("synthesis: {e}"));
                None
            }
        }
    };

    Ok(ResearchReport {
        query: req.query,
        depth_used,
        iterations: iterations_done,
        subqueries: initial_queries,
        sources,
        answer,
        run_dir: Some(dir.display().to_string()),
        warnings,
        reflections,
        budget_exhausted,
        schema_valid,
        cost_usd: if tracked { Some(spent) } else { None },
        llm: llm_desc,
    })
}

/// One synthesis call over the bounded, ranked context.
async fn synthesize(
    llm: &dyn LlmClient,
    req: &ResearchRequest,
    subqueries: &[String],
    reflections: &[String],
    sources: &[Source],
) -> Result<(String, Option<f64>), VygrError> {
    let context = assemble_context(sources, req.context_max_chars);
    let mut user = format!(
        "Question: {}\n\nSub-queries explored:\n{}\n\nResearch notes:\n{}\n\nEvidence blocks:\n{}",
        req.query,
        subqueries
            .iter()
            .map(|s| format!("- {s}"))
            .collect::<Vec<_>>()
            .join("\n"),
        if reflections.is_empty() {
            "(none)".to_string()
        } else {
            reflections
                .iter()
                .map(|n| format!("- {n}"))
                .collect::<Vec<_>>()
                .join("\n")
        },
        context,
    );
    let mut max_tokens = 4_096u32;
    if let Some(schema) = &req.output_schema {
        user.push_str(
            "\n\nOutput contract: respond with ONLY a JSON object (no prose, no code fences) \
validating against this JSON Schema:\n",
        );
        user.push_str(schema);
        max_tokens = 8_192;
    }
    let resp = llm
        .complete(&CompletionRequest {
            messages: vec![
                ChatMessage::system(SYNTHESIS_SYSTEM),
                ChatMessage::user(user),
            ],
            max_tokens: Some(max_tokens),
            temperature: Some(0.3),
            json_object: req.output_schema.is_some(),
        })
        .await?;
    Ok((resp.content, resp.cost_usd))
}

/// Numbered evidence blocks bounded by `max_chars` (citations map to
/// `sources.json` order).
fn assemble_context(sources: &[Source], max_chars: usize) -> String {
    let mut context = String::new();
    let mut used = 0usize;
    for (i, s) in sources.iter().enumerate() {
        if used >= max_chars {
            break;
        }
        let excerpt: String = match &s.content {
            Some(c) => c.chars().take(2_000).collect(),
            None => s.snippet.clone(),
        };
        if excerpt.is_empty() {
            continue;
        }
        let block = format!("[{}] {} ({})\n{}\n\n", i + 1, s.title, s.url, excerpt);
        if used + block.len() > max_chars {
            break;
        }
        used += block.len();
        context.push_str(&block);
    }
    context
}

/// Digest of the `newest` most recent sources, for the reflection input.
/// Always small; raw pages never enter the loop (ADR-0012).
const DIGEST_MAX: usize = 6_000;

fn sources_digest(sources: &[Source], newest: usize) -> String {
    let start = sources.len().saturating_sub(newest);
    let mut digest = String::new();
    for s in sources[start..].iter().rev() {
        if digest.len() >= DIGEST_MAX {
            digest.push_str("...(truncated)\n");
            break;
        }
        let excerpt: String = match &s.content {
            Some(c) => c.chars().take(300).collect(),
            None => s.snippet.chars().take(300).collect(),
        };
        digest.push_str(&format!("- {} ({}): {excerpt}\n", s.title, s.url));
    }
    digest
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

    fn source(title: &str, score: f32, content: Option<&str>) -> Source {
        Source {
            title: title.to_string(),
            url: format!("https://a.io/{title}"),
            snippet: String::new(),
            provider: "t".into(),
            score,
            level: 0,
            content: content.map(str::to_string),
        }
    }

    #[test]
    fn context_assembly_is_bounded_and_numbered() {
        let mut sources = vec![
            source("a", 3.0, Some("content one")),
            source("b", 2.0, None),
        ];
        sources[1].snippet = "snippet two".into();
        let ctx = assemble_context(&sources, 10_000);
        assert!(ctx.starts_with("[1] a (https://a.io/a)"));
        assert!(ctx.contains("[2] b"));
        // A tiny cap stops after the first block.
        let ctx = assemble_context(&sources, 40);
        assert!(ctx.contains("[1]"));
        assert!(!ctx.contains("[2]"));
    }

    #[test]
    fn digest_covers_only_newest_sources_and_caps() {
        let sources = vec![source("old", 1.0, None), source("new", 1.0, None)];
        let digest = sources_digest(&sources, 1);
        assert!(digest.contains("new"));
        assert!(!digest.contains("old"));
        let big = "x".repeat(10_000);
        let many: Vec<Source> = (0..50)
            .map(|i| source(&format!("s{i}"), 1.0, Some(&big)))
            .collect();
        assert!(sources_digest(&many, 50).len() <= 6_400);
    }
}
