# ADR 0004: LLM backends — models.dev, Ollama and harness shell-out

- **Date:** 2026-10-01
- **Status:** Accepted

## Context

None of the surveyed tools solves "use any LLM" well: tvly has no client-side
LLM; Librarium lacks Ollama and generic endpoints; GPT Researcher drags
LangChain for ~24 providers. Separately, when vygr runs inside an agent
harness (pi, Claude Code, Codex), the best LLM is the one **already
configured there** — keys, model and session included. pi exposes no MCP
server mode, but it does expose print-mode subprocesses (`pi --print`,
`claude -p`, `codex exec`) and models.dev publishes a machine-readable
catalog of providers, key env-vars, base URLs, limits and prices.

## Decision

One trait, `LlmClient::complete(&CompletionRequest) -> CompletionResponse`
(with usage and cost when known), resolved from a spec string:

| spec | backend |
|---|---|
| `pi` / `claude` / `codex` | `HarnessLlm`: shell-out to the harness in print mode; stdout is the completion |
| `ollama:<model>` | Ollama's OpenAI-compatible `/v1` endpoint |
| `openai-compat` (+ `[llm] base_url/model`) | any custom endpoint |
| `<provider>:<model>` | any models.dev provider exposing an `api` base URL; env-var name, default model and per-token pricing come from the catalog (cached 24h) |

Native non-OpenAI-compatible APIs (Anthropic, Google) are **not** implemented
in phase 1 — route them through openrouter instead; native clients are
roadmap if needed.

Known limitation: `HarnessLlm` passes the prompt as an argv element; very
large contexts can hit `ARG_MAX`. Switching to stdin piping is tracked in the
roadmap (phase 2).

## Consequences

- "Any provider" support is a JSON download away — no provider matrix to
  maintain by hand.
- Harness reuse needs no MCP pass-through and no key duplication; the same
  spec works from a terminal or from inside an agent session.
- Cost accounting is only as good as the catalog's prices; unknown cost is
  reported as absent, never as zero (Librarium rule).
