---
name: voyager-deep-research
description: Deep, multi-source web research using the vygr CLI (voyager). Use when a task needs exhaustive, well-cited investigation of a topic — comparing options, mapping a technology or market landscape, or fact-finding that benefits from multiple independent sources — rather than a single quick lookup.
license: MIT
---

# Voyager deep research (vygr)

`vygr` is a command-line research tool: it searches the web through provider
chains (keyless DuckDuckGo by default), fetches and reduces pages to text, and
can run a full plan-search-synthesize research loop over any configured LLM
backend — including *your own harness LLM* when invoked with `--llm pi` (or
`claude` / `codex`).

## Quick lookups

```sh
vygr search "rust vs zig wasm benchmarks" --max-results 5
vygr search "q" --format urls                    # pipe-friendly
vygr search "q" --extract-top 3 --format json    # results + inlined page content
vygr extract https://example.com/page --max-chars 12000
```

## Deep research

```sh
vygr plan "question"                       # offline preflight: what will run
vygr research "question" --llm pi          # reuse the harness LLM
vygr research "question" --llm ollama:qwen3:8b --depth 2..4 --budget-usd 0.50
```

Every run leaves an evidence trail under `agents/voyager/<timestamp>-<slug>/`
(`prompt.md`, `plan.json`, `sources.json`, `answer.md`). Cite those files when
the user asks where conclusions came from.

## Research methodology

When driving vygr through a full research run, follow this discipline:

1. **Reframe before searching.** Generate sub-queries with genuinely different
   framings (terminology, domain angle, time period). vygr's planner does this
   automatically, but check `plan.json` and re-run with adjusted breadth if the
   framings are redundant.
2. **Search for disconfirmation.** At least one sub-query should look for
   evidence *against* the working hypothesis ("X problems", "X vs Y criticism",
   "X deprecated").
3. **Treat syndication as one source.** Many outlets repeating one press
   release is a single evidence chain. `vygr search --all` reports which
   providers returned each URL; prefer independent origins.
4. **Reconcile, don't average.** If numbers disagree, compare definitions,
   units and time periods, and say why they differ instead of splitting the
   difference.
5. **Cite by number.** The synthesized answer cites evidence blocks as `[n]`
   mapping to `sources.json`. Keep those citations when summarizing further.
6. **Budget explicitly.** Pass `--budget-usd` for expensive backends; vygr
   skips synthesis and returns sources only if the budget is blown.

## Notes for agents

- Machine output: `--format json` on every command; diagnostics go to stderr.
- Exit codes: 0 ok · 2 usage · 3 config/auth · 4 provider/network.
- `vygr schema` prints a self-describing JSON of all commands and flags.
- `vygr providers` shows which search backends have credentials; `vygr models`
  browses the models.dev catalog for `--llm` specs.
- When you (the harness) already have a capable LLM, prefer `--llm pi` /
  `--llm claude` / `--llm codex` — vygr will shell out to you and reuse your
  configured model, keys and session.
- As an MCP server (`vygr serve` over stdio) vygr exposes `search`,
  `extract`, `research` and `get_artifact`; page long artifacts with
  `get_artifact` instead of reading whole files into context.
