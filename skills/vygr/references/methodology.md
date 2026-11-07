# Research methodology

Discipline that separates a defensible report from a plausible one. vygr's
planner does some of this automatically; check its output and improve on it.

## 1. Reframe before searching

Generate sub-queries with genuinely different framings — terminology
("EOL" vs "deprecated" vs "maintenance mode"), domain angle (engineering
blog vs standards doc vs issue tracker), time period. Check `plan.json`:
if the framings are near-synonyms, re-run with a hand-written angle or
adjust `--breadth`.

## 2. Search for disconfirmation

At least one sub-query must look for evidence *against* the working
hypothesis: "X problems", "X vs Y criticism", "X deprecated", "X failed
production". A research loop that never risks being wrong only confirms
its first page of results.

## 3. Treat syndication as one source

Many outlets repeating one press release is a single evidence chain.
`vygr search --all --format json` reports which providers returned each
URL (`providers` array) — prefer independent origins over same-story
clusters. Keyless chains (ddgs + openalex) are independent by construction.

## 4. Reconcile, don't average

When numbers disagree, compare definitions, units, time periods and
populations before deciding what to report. Say why they differ; never
split the difference between incompatible figures. Flag the conflict in
the answer's caveats section.

## 5. Cite by number

The synthesized answer cites evidence blocks as `[n]` mapping to
`sources.json`. When summarizing a vygr report further (for the user or
another agent), keep the `[n]` markers so every claim stays traceable.
`vygr runs show` / `get_artifact` (MCP) read artifacts without re-running.

## 6. Budget explicitly

Pass `--budget-usd` whenever the backend bills per call. vygr skips
synthesis and returns ranked sources only if the budget is blown — that is
a feature, not a failure: you can still cite `sources.json`. Check
`run.json`'s `cost_usd` after expensive runs.

## Academic questions

Route paper-shaped queries through the keyless academic providers:
`--provider "openalex,s2"` (OpenAlex works keyless; Semantic Scholar's
anonymous tier is tight, add `S2_API_KEY` for real use). They return
structured abstracts and citation counts — better evidence than blog
summaries for any "what does the literature say" question.

## When to use what

- One fact, one URL: `vygr search ... --extract-top 2 --format json`.
- A comparison or landscape: `vygr research` with a hand-framed question.
- "Has anything changed since…": re-run with `--time-range` filters
  (config `[research] time_range`) or a narrower question, then diff
  against the old run's `sources.json`.
