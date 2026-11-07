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
use serde::{Deserialize, Serialize};
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

/// Prior-investigation context for follow-up runs and monitors (ADR-0018):
/// the loop seeds its visited set, notes and evidence from the parent, so
/// follow-up searches only hunt for what is new.
#[derive(Debug, Clone)]
pub struct ParentContext {
    /// Run directory name of the parent investigation.
    pub run_name: String,
    /// Distilled notes from the parent run (become starting notes).
    pub notes: Vec<String>,
    /// Parent sources incl. content (become starting evidence).
    pub sources: Vec<Source>,
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
    pub language: Option<String>,
    /// Raw JSON Schema the synthesized answer must satisfy (M2.5). When
    /// set, the answer is requested as JSON and validated (parse-level).
    pub output_schema: Option<String>,
    /// Continue an earlier investigation (follow-up / monitor runs).
    pub parent: Option<ParentContext>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub provider: String,
    pub score: f32,
    /// Depth level at which this source was gathered.
    pub level: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Same-story cluster id (syndication de-dup, ADR-0017); unique
    /// stories are `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster_id: Option<u32>,
    /// Copies of this story (cluster size) when > 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_count: Option<usize>,
    /// Best-matching passage, for citation-precise synthesis (ADR-0017).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub best_quote: Option<String>,
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
    /// The answer is a deterministic degraded report (budget would not
    /// cover synthesis), not an LLM synthesis.
    pub degraded: bool,
    /// Whether the synthesized answer parsed as JSON when an output
    /// schema was requested; `None` when no schema was given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema_valid: Option<bool>,
    /// Citation verification against the stored sources; `None` when no
    /// synthesis ran.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification: Option<crate::verify::Verification>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    pub llm: String,
}

pub(crate) const SYNTHESIS_SYSTEM: &str = "You are a senior research analyst. Write a focused markdown report answering the question. \
Begin with a '## Summary' section of at most 200 words before any other content. \
Cite sources inline with bracketed numbers like [1] referring only to the numbered evidence blocks provided. \
Prefer primary sources. Use the research notes as guidance but verify claims against the evidence blocks. \
Sources marked as syndicated copies are one evidence chain, not independent confirmation. \
If evidence blocks or the reported disagreements conflict, reconcile them explicitly instead of \
averaging incompatible figures, and add a '## Disagreements' section explaining each conflict and \
how you resolved it (definitions, units, time periods). End with a '## Caveats & open questions' \
section listing what could not be verified from the provided evidence.";

/// Receiver for human-readable progress lines during a run (levels,
/// reflections, synthesis). The CLI prints them to stderr; the MCP server
/// maps them to `notifications/progress` (which also keeps pi's 60s
/// per-request timeout at bay).
pub type ProgressSink = std::sync::Arc<dyn Fn(&str) + Send + Sync>;

/// Structured lifecycle events for machine consumers (`--events`):
/// one JSON line each on stdout, complementing the human progress lines
/// on stderr (ADR-0016).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ResearchEvent {
    RunStart {
        query: String,
        provider: String,
        depth_min: u32,
        depth_max: u32,
        breadth: u32,
        llm: String,
        /// Parent investigation when this is a follow-up/monitor run.
        #[serde(skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
    },
    Plan {
        subqueries: Vec<String>,
        depth_used: u32,
    },
    LevelStart {
        level: u32,
        total: u32,
        queries: Vec<String>,
    },
    SourcesFound {
        level: u32,
        new: usize,
        total: usize,
    },
    Reflection {
        level: u32,
        notes: usize,
        followups: Vec<String>,
        disagreements: usize,
        satisfied: bool,
    },
    SynthesisStart {
        sources: usize,
        estimated_cost_usd: Option<f64>,
    },
    Verified {
        checked: usize,
        weak: usize,
    },
    Done {
        cost_usd: Option<f64>,
        degraded: bool,
        run_dir: Option<String>,
    },
    Warning {
        message: String,
    },
}

/// Receiver for structured run events. The CLI serializes each event as
/// a single JSON line on stdout so harnesses can render progress or kill
/// long runs early.
pub type EventSink = std::sync::Arc<dyn Fn(&ResearchEvent) + Send + Sync>;

pub async fn run(
    req: ResearchRequest,
    llm: Box<dyn LlmClient>,
    http: reqwest::Client,
    progress: Option<ProgressSink>,
    events: Option<EventSink>,
) -> Result<ResearchReport, VygrError> {
    let progress = progress.unwrap_or_else(|| std::sync::Arc::new(|_| {}));
    let report = |msg: String| progress(msg.as_str());
    let events = events.as_ref();
    let emit = |ev: ResearchEvent| {
        if let Some(sink) = events {
            sink(&ev);
        }
    };
    let mut warnings = Vec::new();
    let mut spent = 0.0f64;
    let mut tracked = false;
    let llm_desc = llm.describe();
    let over_budget =
        |spent: f64, tracked: bool| tracked && req.budget_usd.is_some_and(|b| spent >= b);

    emit(ResearchEvent::RunStart {
        query: req.query.clone(),
        provider: req.provider_spec.clone(),
        depth_min: req.depth.min,
        depth_max: req.depth.max,
        breadth: req.breadth,
        llm: llm_desc.clone(),
        parent: req.parent.as_ref().map(|p| p.run_name.clone()),
    });

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
    //    Follow-up runs hand the planner the parent's notes so the
    //    sub-queries target what is still missing (ADR-0018).
    let prior_notes: Vec<String> = req
        .parent
        .as_ref()
        .map(|p| p.notes.clone())
        .unwrap_or_default();
    let (initial_queries, depth_used, plan_cost) =
        planner::plan(&*llm, &req.query, req.breadth, &req.depth, &prior_notes).await?;
    if let Some(c) = plan_cost {
        spent += c;
        tracked = true;
    }
    tracing::info!(depth_used, subqueries = initial_queries.len(), "plan ready");
    report(format!(
        "plan ready: {} sub-queries, depth {depth_used}",
        initial_queries.len()
    ));
    emit(ResearchEvent::Plan {
        subqueries: initial_queries.clone(),
        depth_used,
    });
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
    let fetcher =
        vygr_providers::EscalatingFetch::new(vygr_providers::HttpFetch::new(http.clone()), http);

    let mut visited: HashSet<String> = HashSet::new();
    let mut sources: Vec<Source> = Vec::new();
    let mut reflections: Vec<String> = Vec::new();
    let mut disagreements: Vec<String> = Vec::new();
    let mut next_queries: Vec<String> = initial_queries.clone();
    let mut budget_exhausted = false;
    let mut iterations_done = 0u32;

    // Follow-up runs (ADR-0018): parent evidence becomes the starting
    // context and its URLs are treated as already visited.
    if let Some(parent) = &req.parent {
        for mut s in parent.sources.clone() {
            visited.insert(vygr_providers::url_key(&s.url));
            s.level = 0;
            sources.push(s);
        }
        reflections.extend(parent.notes.iter().cloned());
        let _ = run_dir::write_file(
            &dir,
            "parent.json",
            &serde_json::to_string_pretty(&serde_json::json!({
                "parent_run": parent.run_name,
                "inherited_sources": parent.sources.len(),
                "inherited_notes": parent.notes.len(),
            }))
            .unwrap_or_default(),
        );
    }

    for level in 0..depth_used {
        // Breadth halves per level, floored at 2 (GPT Researcher model).
        let level_breadth = (req.breadth >> level).max(2) as usize;
        let queries: Vec<String> = next_queries.iter().take(level_breadth).cloned().collect();
        if queries.is_empty() {
            break;
        }
        emit(ResearchEvent::LevelStart {
            level: level + 1,
            total: depth_used,
            queries: queries.clone(),
        });

        // 2a) Search this level's sub-queries.
        let mut level_results: Vec<SearchResult> = Vec::new();
        for sq in &queries {
            let q = SearchQuery::new(sq.clone(), req.max_results_per_query)
                .with_filters(
                    req.time_range,
                    req.include_domains.clone(),
                    req.exclude_domains.clone(),
                )
                .with_language(req.language.clone());
            let (rs, w) = vygr_providers::search_chain(&handle, &q).await;
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
            if level == 0 && sources.is_empty() {
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
        let term_set: HashSet<String> = query_terms.iter().cloned().collect();
        let level_sources: Vec<Source> = fresh
            .into_iter()
            .zip(fetched)
            .enumerate()
            .map(|(i, (r, content))| {
                let best_quote = content
                    .as_deref()
                    .and_then(|c| score::best_passage(c, &term_set));
                Source {
                    title: r.title,
                    url: r.url,
                    snippet: r.snippet,
                    provider: r.providers.join("+"),
                    score: index.score(&query_terms, i),
                    level,
                    content,
                    cluster_id: None,
                    origin_count: None,
                    best_quote,
                }
            })
            .collect();
        let new_count = level_sources.len();
        sources.extend(level_sources);
        iterations_done = level + 1;
        report(format!(
            "level {}/{}: {} new sources ({} total)",
            level + 1,
            depth_used,
            new_count,
            sources.len()
        ));
        emit(ResearchEvent::SourcesFound {
            level: level + 1,
            new: new_count,
            total: sources.len(),
        });

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
            emit(ResearchEvent::Warning {
                message: format!(
                    "budget ${:.2} reached after level {}; stopping before reflection",
                    req.budget_usd.unwrap_or(0.0),
                    level + 1
                ),
            });
            break;
        }
        let digest = sources_digest(&sources, new_count);
        match planner::reflect(&*llm, &req.query, &reflections, &digest).await {
            Ok((reflection, cost)) => {
                if let Some(c) = cost {
                    spent += c;
                    tracked = true;
                }
                let followup_count = reflection.followups.len();
                let has_followups = followup_count > 0;
                reflections.extend(reflection.notes);
                disagreements.extend(reflection.disagreements);
                let _ = run_dir::write_file(
                    &dir,
                    "reflections.json",
                    &serde_json::to_string_pretty(&serde_json::json!({
                        "notes": reflections,
                        "disagreements": disagreements,
                        "satisfied": reflection.satisfied,
                        "level": level + 1,
                    }))
                    .unwrap_or_default(),
                );
                report(format!(
                    "reflection: {} notes, {followup_count} follow-up queries",
                    reflections.len()
                ));
                emit(ResearchEvent::Reflection {
                    level: level + 1,
                    notes: reflections.len(),
                    followups: reflection.followups.clone(),
                    disagreements: disagreements.len(),
                    satisfied: !has_followups,
                });
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
                emit(ResearchEvent::Warning {
                    message: format!("reflection failed: {e}"),
                });
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

    // 3) Rank everything, cluster syndicated copies, and persist the
    // evidence (citations [n] index into sources.json order).
    sources.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    crate::cluster::cluster_sources(&mut sources);
    let sources_json = serde_json::to_string_pretty(&sources).unwrap_or_default();
    let _ = run_dir::write_file(&dir, "sources.json", &sources_json);

    // 4) Abstain rather than fabricate when there is no usable evidence
    // at all (GPTR lesson): every fetch failed and no snippet survived.
    let has_evidence = sources.iter().any(|s| {
        s.content.as_deref().is_some_and(|c| !c.trim().is_empty()) || !s.snippet.trim().is_empty()
    });
    if !has_evidence {
        warnings.push(
            "no usable evidence was gathered (every fetch failed and snippets are empty); \
abstaining from synthesis instead of fabricating"
                .to_string(),
        );
        emit(ResearchEvent::Warning {
            message: "no usable evidence was gathered; abstaining from synthesis".to_string(),
        });
        emit(ResearchEvent::Done {
            cost_usd: if tracked { Some(spent) } else { None },
            degraded: false,
            run_dir: Some(dir.display().to_string()),
        });
        return Ok(ResearchReport {
            query: req.query,
            depth_used,
            iterations: iterations_done,
            subqueries: initial_queries,
            sources,
            answer: None,
            run_dir: Some(dir.display().to_string()),
            warnings,
            reflections,
            budget_exhausted,
            degraded: false,
            schema_valid: None,
            verification: None,
            cost_usd: if tracked { Some(spent) } else { None },
            llm: llm_desc,
        });
    }

    // 5) Budget guard before the expensive call (ADR-0007), now with an
    // upper-bound estimate so the guard trips before spending, not after.
    report(format!(
        "synthesizing report from {} sources",
        sources.len()
    ));
    let estimate = llm.estimate_cost_usd(req.context_max_chars, Some(4_096));
    emit(ResearchEvent::SynthesisStart {
        sources: sources.len(),
        estimated_cost_usd: estimate,
    });
    let mut schema_valid: Option<bool> = None;
    let mut verification_value: Option<crate::verify::Verification> = None;
    let mut degraded = false;
    let answer = if tracked
        && req
            .budget_usd
            .is_some_and(|b| spent + estimate.unwrap_or(0.0) > b)
    {
        budget_exhausted = true;
        degraded = true;
        warnings.push(format!(
            "budget ${:.2} would be exceeded by synthesis (spent {spent:.2}, estimated upper bound {:.2}); \
returning a degraded report built from the gathered evidence",
            req.budget_usd.unwrap_or(0.0),
            estimate.unwrap_or(0.0)
        ));
        emit(ResearchEvent::Warning {
            message: "budget would be exceeded by synthesis; returning a degraded report"
                .to_string(),
        });
        Some(degraded_answer(&req.query, &reflections, &sources))
    } else {
        match synthesize(
            &*llm,
            &req,
            &initial_queries,
            &reflections,
            &disagreements,
            &sources,
        )
        .await
        {
            Ok((content, cost)) => {
                if let Some(c) = cost {
                    spent += c;
                    tracked = true;
                }
                let mut answer = content.trim().to_string();
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
                // 6) Verify citations against the stored evidence and flag
                // weakly supported claims (Mole-inspired).
                let verification = crate::verify::verify(&answer, &sources);
                report(format!(
                    "verification: {} cited sentences checked, {} weakly supported",
                    verification.checked,
                    verification.weak_count()
                ));
                emit(ResearchEvent::Verified {
                    checked: verification.checked,
                    weak: verification.weak_count(),
                });
                if !verification.is_clean() {
                    warnings.push(format!(
                        "{} weakly supported citation(s); see the verification section of answer.md",
                        verification.weak_count()
                    ));
                    answer.push_str(&crate::verify::verification_section(&verification));
                }
                // Claim-level evidence table (ADR-0017).
                let _ = run_dir::write_file(
                    &dir,
                    "claims.json",
                    &serde_json::to_string_pretty(&verification.claims).unwrap_or_default(),
                );
                verification_value = Some(verification);
                let _ = run_dir::write_file(&dir, "answer.md", &answer);
                Some(answer)
            }
            Err(e) => {
                warnings.push(format!("synthesis: {e}"));
                emit(ResearchEvent::Warning {
                    message: format!("synthesis failed: {e}"),
                });
                None
            }
        }
    };
    if let Some(answer) = &answer {
        let _ = run_dir::write_file(&dir, "answer.md", answer);
    }
    let _ = run_dir::write_file(
        &dir,
        "run.json",
        &serde_json::to_string(&crate::runs::RunManifest {
            query: req.query.clone(),
            depth_used,
            iterations: iterations_done,
            subqueries: initial_queries.clone(),
            reflections: reflections.clone(),
            provider_spec: req.provider_spec.clone(),
            llm: llm_desc.clone(),
            parent: req.parent.as_ref().map(|p| p.run_name.clone()),
        })
        .unwrap_or_default(),
    );

    emit(ResearchEvent::Done {
        cost_usd: if tracked { Some(spent) } else { None },
        degraded,
        run_dir: Some(dir.display().to_string()),
    });

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
        degraded,
        schema_valid,
        verification: verification_value,
        cost_usd: if tracked { Some(spent) } else { None },
        llm: llm_desc,
    })
}

/// Deterministic degraded report when the budget will not cover
/// synthesis: research notes and ranked sources, no LLM involved.
fn degraded_answer(query: &str, reflections: &[String], sources: &[Source]) -> String {
    let mut out = String::from("# Research report (degraded)\n\n");
    out.push_str(&format!("**Question:** {query}\n\n"));
    out.push_str("The synthesis budget was exhausted, so this report is built ");
    out.push_str("deterministically from the gathered evidence and research notes.\n");
    if !reflections.is_empty() {
        out.push_str("\n## Research notes\n\n");
        for note in reflections {
            out.push_str(&format!("- {note}\n"));
        }
    }
    out.push_str("\n## Sources (ranked)\n\n");
    for (i, s) in sources.iter().take(15).enumerate() {
        out.push_str(&format!(
            "{}. [{}] {} ({})\n",
            i + 1,
            s.provider,
            s.title,
            s.url
        ));
    }
    out.push_str("\nRe-run with a higher `--budget-usd` for a synthesized analysis.\n");
    out
}

/// One synthesis call over the bounded, ranked context.
async fn synthesize(
    llm: &dyn LlmClient,
    req: &ResearchRequest,
    subqueries: &[String],
    reflections: &[String],
    disagreements: &[String],
    sources: &[Source],
) -> Result<(String, Option<f64>), VygrError> {
    let context = assemble_context(sources, req.context_max_chars);
    let mut user = format!(
        "Question: {}\n\nSub-queries explored:\n{}\n\nResearch notes:\n{}\n\n{prompt_dis}Evidence blocks:\n{context}{prompt_parent}",
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
        prompt_dis = if disagreements.is_empty() {
            String::new()
        } else {
            format!(
                "Reported disagreements between sources (reconcile explicitly, do not average):\n{}\n\n",
                disagreements
                    .iter()
                    .map(|d| format!("- {d}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        },
        prompt_parent = match &req.parent {
            Some(parent) => format!(
                "\n\nThis continues the earlier investigation '{}' ({} inherited evidence blocks, \
{} inherited notes). Emphasize what is NEW or CHANGED relative to that investigation; do not \
repeat its full findings unless needed for context.",
                parent.run_name,
                parent.sources.len(),
                parent.notes.len()
            ),
            None => String::new(),
        },
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
pub(crate) fn assemble_context_public(sources: &[Source], max_chars: usize) -> String {
    assemble_context(sources, max_chars)
}

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
        let syndicated = match s.origin_count {
            Some(n) if n > 1 => format!(" [same story as {n} outlets — one evidence chain]"),
            _ => String::new(),
        };
        let quote = match &s.best_quote {
            Some(q) => format!("\n> {q}"),
            None => String::new(),
        };
        let block = format!(
            "[{}] {} ({}){syndicated}\n{excerpt}{quote}\n\n",
            i + 1,
            s.title,
            s.url
        );
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
            cluster_id: None,
            origin_count: None,
            best_quote: None,
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
