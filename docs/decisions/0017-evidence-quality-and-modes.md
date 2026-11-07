# ADR 0017: Evidence quality pipeline and research modes

- **Date:** 2026-11-06
- **Status:** Accepted (implemented in 0.7.0)
- **Background:** the reports were good-but-generic. The 2025-26
  deep-research landscape (STORM's multi-perspective questioning, GPT
  Researcher's depth/breadth knobs, claim-level verification passes,
  tvly's mini/pro/auto presets) converges on a set of cheap,
  deterministic techniques a single-binary CLI can adopt without a judge
  model or a multi-agent runtime.

## Decision

1. **Mode presets.** `research --mode quick|standard|deep|exhaustive`
   maps to depth/breadth/fetch-top starting points. Precedence is
   explicit flags > preset > config defaults. Presets stay in code
   rather than gaining a `[research.modes]` config section: flags are
   already the customization mechanism, and a third precedence layer
   would buy nothing.
2. **Claim-level evidence table.** Citation verification now records
   every checked claim (not only weak ones) as `claims.json`:
   `{claim, citation, cited_url, best_source, overlap, verdict}`.
   `answer.md` keeps the weak-claim section; `claims.json` is the
   machine-readable audit trail.
3. **Disagreement reconciliation.** The reflection prompt reports
   contradictions between sources as `disagreements[]` (persisted in
   `reflections.json`); the synthesis prompt lists them and requires a
   `## Disagreements` section explaining each conflict and its
   resolution (definitions, units, time periods) — the skill's
   "reconcile, don't average" discipline becomes enforced by the prompt.
4. **Syndication clustering.** After ranking, sources with
   near-identical titles (Jaccard > 0.7 on content tokens, union-find)
   share a `cluster_id` and `origin_count` in `sources.json`; evidence
   blocks are annotated "[same story as N outlets — one evidence chain]"
   so the model cannot count syndication as confirmation. Threshold is
   deliberately high: a false merge hides a real source.
5. **Best quotes and an executive brief.** Each source stores the
   highest term-overlap sentence (`best_quote`) rendered as a quote line
   in its evidence block, improving citation precision. Synthesis must
   open with `## Summary` (≤200 words); `research --brief-only` prints
   just that section for context-tight agents.
6. **`vygr eval <run>`.** A deterministic scorecard over a finished run
   dir: citation resolvability, lexical verification, sub-query coverage
   (term overlap between plan sub-queries and sources), whether any
   sub-query searched for disconfirming evidence, and host/cluster
   diversity. Score = 0.4·resolvability + 0.3·coverage + 0.3·diversity,
   penalized up to 50% by the weak-claim share — transparent by design,
   not a benchmark. Written to `eval.json` next to the artifacts (not
   into `run.json`, which stays a pure manifest).

## Consequences

- `Source` gains three optional fields (`cluster_id`, `origin_count`,
  `best_quote`); older `sources.json` files deserialize unchanged
  (`#[serde(default)]`), so `runs resume` and `eval` work on pre-0.7
  runs.
- The event stream gains additive fields only (`Reflection.disagreements`,
  `RunStart.parent`).
- Eval is heuristic: term overlap cannot judge truth, only traceability.
  Its role is regression detection across runs, not grading.
