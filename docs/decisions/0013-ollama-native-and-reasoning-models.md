# ADR 0013: Native Ollama client and reasoning-model handling

- **Date:** 2026-10-01
- **Status:** Accepted (implemented in phase 2)
- **Amends:** [ADR-0004](0004-llm-backends.md)

## Context

Live validation against Ollama surfaced three real problems with routing
Ollama through the OpenAI-compatible endpoint:

1. Reasoning models (qwen3.5, deepseek-r1, …) return `content: ""` and put
   everything in a `reasoning` field, so planner/reflection JSON parsing got
   nothing usable.
2. The `think: false` control is **ignored** by Ollama's
   OpenAI-compatible endpoint, so reasoning burned the whole token budget
   before producing a single JSON byte.
3. The shared 30 s HTTP client timeout is far too short for local models
   that cold-load and reason for minutes.

## Decision

- vygr ships a **native Ollama client** (`/api/chat`) used for the
  `ollama` backend: `think: false` keeps structured outputs affordable,
  `format: "json"` implements `--output-schema` natively, and
  `options.num_predict`/`options.temperature` map the request knobs.
  Local tokens report `cost_usd: None` (never zero, per ADR-0007).
- The OpenAI-compatible client gains a reasoning fallback: `content` is
  authoritative; when empty, the `reasoning` field is used; inline
  `<think>…</think>` blocks are stripped from both. Per-request timeout
  raised to 600 s for chat completions.
- Planner/reflection token budgets raised (2500/2000): reasoning models
  spend budget thinking before emitting JSON.

## Consequences

- Local reasoning models work out of the box: validated live with
  `qwen3.5:9b` through plan → reflect → synthesize and
  `--output-schema`.
- API-hosted reasoning models still burn tokens thinking; that cost shows
  up in the report and is covered by the budget guard.
