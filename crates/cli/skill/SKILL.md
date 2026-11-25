---
name: vygr
description: Deep, multi-source web research using the vygr CLI (voyager). Use when a task needs exhaustive, well-cited investigation of a topic, such as comparing options, mapping a technology or market landscape, or fact-finding that benefits from multiple independent sources, rather than a single quick lookup.
license: MIT
version: 0.9.1
---

# Voyager deep research (vygr)

`vygr` is a command-line research tool: it searches the web through provider
chains (keyless DuckDuckGo + OpenAlex by default), fetches and reduces pages
to text, and runs full plan-search-synthesize research loops over any LLM
backend — including *your own harness LLM* with `--llm pi` (or `claude` /
`codex`).

## Quick lookups

```sh
vygr search "rust vs zig wasm benchmarks" --max-results 5
vygr search "q" --extract-top 3 --format json    # results + inlined page content
vygr search "q" --format jsonl                   # one JSON object per result (pipe-friendly)
vygr extract https://example.com/page --max-chars 12000
vygr doctor --format json                        # what works right now (providers, LLM, dirs)
```

## Deep research

```sh
vygr plan "question"                        # offline preflight: what will run, cost estimate
vygr research "question" --llm pi           # reuse the harness LLM
vygr research "question" --llm ollama:qwen3:8b --depth 2..4 --budget-usd 0.50
```

Every run leaves an evidence trail under `.voyager/<timestamp>-<slug>/`
(override location with `--run-dir` or `VGR_RUN_DIR`):

- `answer.md` — synthesized report with `[n]` citations
- `sources.json` — scored sources incl. fetched content; `[n]` indexes into this order
- `plan.json`, `reflections.json`, `run.json`, `prompt.md` — the full trail

Cite those files when the user asks where conclusions came from. Re-synthesize
offline with `vygr runs resume <name> --llm <spec>` (no new searches).

## Notes for agents

- Machine output: `--format json` on every command (`jsonl` on search);
  diagnostics go to stderr. Exit codes: 0 ok · 1 internal · 2 usage ·
  3 config/auth · 4 provider/network.
- `vygr schema` prints the authoritative interface as JSON (every command,
  flag, default and choice — derived from the CLI itself).
- Stream long runs: `vygr research "q" --llm pi --events` emits NDJSON
  lifecycle events on stdout; the final `report` line carries everything.
- Keep artifacts out of the user's repo: pass `--run-dir /tmp/vygr-runs`.
- When you (the harness) already have a capable LLM, prefer `--llm pi` /
  `--llm claude` / `--llm codex` — vygr shells out and reuses your model,
  keys and session.
- As an MCP server (`vygr serve` over stdio) vygr exposes `search`,
  `extract`, `research` and `get_artifact`; page long artifacts with
  `get_artifact` instead of reading whole files into context.

## References (load on demand)

- `references/recipes.md` — copy-paste recipes: compare X vs Y, structured
  extraction with `--output-schema`, cheap deep dives, artifact paging.
- `references/methodology.md` — research discipline: reframing,
  disconfirmation, syndication de-duplication, reconciling conflicts.
- `references/machine-contract.md` — full flag surface, exit codes, event
  schema, MCP tools, config precedence.
