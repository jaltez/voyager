# ADR 0009: Run artifacts (the evidence trail)

- **Date:** 2026-10-01
- **Status:** Accepted

## Context

Librarium's most underrated idea: every run writes a directory of artifacts
(`prompt.md`, `sources.json`, `answer.md`…), which keeps the MCP surface
token-safe and gives audits a trail. GPT Researcher embeds cost/source
metadata in report frontmatter for the same reason.

## Decision

Every `vygr research` run creates `agents/voyager/<timestamp>-<slug>/`
(overridable via `[research] run_dir`):

```
prompt.md     the question and every run parameter (chain, depth, budget, llm)
plan.json     sub-queries and the depth the planner chose
sources.json  scored, deduped sources with fetched content and provenance
answer.md     the synthesized report (when synthesis ran)
```

The command prints the artifact path to **stderr** so stdout stays parseable.
Citations in `answer.md` (`[n]`) index into `sources.json`.

## Consequences

- Answers are verifiable: every claim traces to a fetched page on disk.
- A future MCP server pages summaries and points at these files instead of
  dumping evidence into the agent's context.
- Directories accumulate in the project; `agents/` is gitignored by default
  and a `vygr runs prune` command is an obvious later addition.
