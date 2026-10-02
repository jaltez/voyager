# ADR 0001: Product scope, operating modes and non-goals

- **Date:** 2026-10-01
- **Status:** Accepted

## Context

We compared five reference tools (see `docs/research/comparative-analysis.md`):
Tavily's `tvly` CLI, GPT Researcher, hsearch (anygen-search-cli), Web Forager
and Librarium. They cluster into two shapes: thin multi-provider dispatchers
over paid research APIs (tvly, hsearch) and self-contained research engines
(GPT Researcher, Librarium). We want the configurability of the former with
the loop ownership of the latter, as a fast static Rust binary.

## Decision

voyager is a **configurable deep-research CLI** with three operating modes:

1. **Search-only**: `vygr search` / `vygr extract`. Provider chains and
   fan-out, machine-friendly output. Must work keyless out of the box
   (DuckDuckGo default).
2. **Full research with an LLM**: `vygr research` runs plan → search →
   fetch → score → synthesize over any `LlmClient` backend: models.dev
   providers, Ollama, custom OpenAI-compatible endpoints, or the harness
   itself (mode 3).
3. **Harness-driven**: invoked from an agent (pi, Claude Code, Codex…)
   either as a plain CLI (the agent reads `--format json` output) or with
   `--llm pi` so the research loop **reuses the harness's configured LLM**
   via shell-out. MCP server mode is roadmap.

Depth, breadth and budget are user-bounded but LLM-decided within bounds
(`--depth 2..4`, `--budget-usd`).

**Non-goals:** being a wrapper over any single vendor's research API; a web
UI; multi-agent orchestration frameworks; embeddings/vector stores.

## Consequences

- The loop is ours: quality and cost depend on our orchestration plus
  whatever LLM the user points at, not on a vendor's server-side agent.
- Keyless-first forces DuckDuckGo HTML scraping with its fragility; the
  chain/fan-out design absorbs outages.
- Harness reuse must stay a thin shell-out (ADR-0004), not a protocol.
