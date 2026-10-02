# Architecture

voyager is a Rust workspace producing a single static binary, `vygr`: a
configurable deep-research CLI with pluggable search providers and LLM
backends.

## Workspace layout

```
voyager/
├── crates/
│   ├── core/        vygr-core       types, provider traits, config, error contract
│   ├── providers/   vygr-providers  ddgs / brave / tavily search, HTTP fetch, chains
│   ├── llm/         vygr-llm        LlmClient trait, OpenAI-compat, models.dev, harness
│   ├── research/    vygr-research   planner, scoring, orchestrator, run artifacts
│   └── cli/         vygr (binary)   command surface, output rendering, exit codes
├── skills/vygr/SKILL.md    agent skill shipped with the repo
└── docs/            ADRs, roadmap, background research
```

Dependency direction is strictly downward: `cli -> research -> {providers,
llm} -> core`. Nothing below `cli` knows clap exists.

## The three seams

Everything pluggable hangs off three traits in `vygr-core`:

| trait              | implementations                                | role                       |
|--------------------|------------------------------------------------|----------------------------|
| `SearchProvider`   | `ddgs`, `brave`, `tavily` (+future: searxng, exa, serper, jina) | normalized web search |
| `FetchProvider`    | `HttpFetch` (browser escalation planned)       | URL -> plain text          |
| `LlmClient`        | `OpenAiCompatible` (incl. Ollama), `HarnessLlm` | chat completions         |

Combinators in `vygr-providers` give the multi-provider behavior:

- **Fallback chain** (`"ddgs,brave"`): first provider with a non-empty result
  set wins; failures degrade to the next.
- **Fan-out** (`--all`): concurrent search across providers, merged and
  deduplicated by normalized URL, recording which providers returned each hit.

## LLM backends (ADR-0004)

`--llm <spec>` resolves to a backend:

- `pi` / `claude` / `codex` — **harness shell-out**: vygr runs the harness in
  print mode and pipes the prompt through stdin (argv carries a short
  instruction), reusing the harness's configured model and credentials with
  no MCP pass-through. `codex` keeps the argv channel with a size guard.
- `ollama:<model>` — **native** `/api/chat` client with `think: false` and
  `format: "json"` (ADR-0013); the OpenAI-compatible endpoint ignores
  thinking control, which reasoning models need disabled.
- `openai-compat` + `[llm] base_url/model` — any custom endpoint; reasoning
  models answered via the `reasoning` field are supported (`<think>`
  blocks stripped).
- `<provider>:<model>` — any models.dev provider with an OpenAI-compatible
  base URL (e.g. `openrouter:anthropic/claude-sonnet-4.5`). The models.dev
  catalog (cached 24h) also supplies per-token pricing used for cost
  accounting.

## Research loop (ADR-0005, ADR-0012)

```
query ──> planner (LLM: sub-queries up-front + depth in [min,max])
      ┌─> level i: search (chain) ──> fresh-URL dedup ──> fetch top-N
      │            └─> BM25 score ──> reflect (LLM: notes + followups)
      └── repeat up to depth_used levels, breadth halves per level
            └─> synthesize (LLM: markdown report, [n] citations)
```

Only distilled reflection notes feed later stages — raw pages never
re-enter the loop (Tavily lesson). The budget guard runs before every
reflection and before synthesis. A `--output-schema` file switches
synthesis to JSON-constrained output with parse-level validation.

Every run writes an artifact directory (ADR-0009):

```
agents/voyager/20261001-142233-rust-vs-zig/
├── prompt.md        the question + run parameters
├── plan.json        sub-queries and chosen depth
├── reflections.json distilled notes per level (when depth > 1)
├── sources.json     scored sources (with fetched content)
└── answer.md        synthesized report
```

## Configuration precedence

```
CLI flags > env (VGR_DEFAULT_PROVIDER, VGR_LLM_BACKEND, VGR_LLM_MODEL)
         > project .voyager.toml (walked up from cwd)
         > user ~/.config/voyager/config.toml
         > defaults
```

API keys are never written by vygr; providers read their own env vars
(`BRAVE_API_KEY`, `TAVILY_API_KEY`, …). `vygr providers` reports readiness.

## CLI machine contract (ADR-0008)

- `--format json` on every command: machine output on **stdout**.
- Diagnostics, warnings and logs: **stderr** only.
- Exit codes: `0` ok, `1` internal, `2` usage (clap), `3` config/auth,
  `4` provider/network.
- Queries accept `-` to read from stdin.
- `vygr schema` prints a self-describing JSON of the whole interface.

## Distribution (ADR-0011)

- Single static binary via `cargo install` / GitHub releases.
- Agent skill at `skills/vygr/SKILL.md`; `vygr init --agent <harness>`
  (pi, claude-code, codex, cursor, omp, opencode, generic) installs it into
  the harness's skills directory.
- `npx skills add <owner>/voyager` once public (vercel-labs/skills registry).
- **MCP server** (`vygr serve`, stdio): hand-rolled newline-delimited
  JSON-RPC 2.0 exposing `search`, `extract`, `research` and `get_artifact`
  (paged, size-capped; paths restricted to the working directory). Register
  it with `pi mcp add voyager -- vygr serve`.
