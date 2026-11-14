# Architecture

voyager is a Rust workspace producing a single static binary, `vygr`: a
configurable deep-research CLI with pluggable search providers and LLM
backends.

## Workspace layout

```
voyager/
├── crates/
│   ├── core/        vygr-core       types, provider traits, config, error contract
│   ├── providers/   vygr-providers  10 search backends, fetch escalation, cache/throttle, chains
│   ├── llm/         vygr-llm        LlmClient trait, OpenAI-compat, models.dev, harness
│   ├── research/    vygr-research   planner, scoring, orchestrator, run artifacts
│   └── cli/         vygr (binary)   command surface, output rendering, exit codes, MCP
├── skills/vygr/            agent skill shipped with the repo (SKILL.md + references/)
├── AGENTS.md        contributor instructions for AI coding agents
└── docs/            ADRs, roadmap, background research
```

Dependency direction is strictly downward: `cli -> research -> {providers,
llm} -> core`. Nothing below `cli` knows clap exists.

## The three seams

Everything pluggable hangs off three traits in `vygr-core`:

| trait              | implementations                                | role                       |
|--------------------|------------------------------------------------|----------------------------|
| `SearchProvider`   | `ddgs`, `brave`, `tavily`, `searxng`, `exa`, `serper`, `jina`, `kagi` + keyless academic `openalex`, `semanticscholar`, `arxiv` | normalized web search |
| `FetchProvider`    | `HttpFetch` + escalation ladder (Jina reader, PDF; headless browser behind the `browser` feature, validated against real Chrome) | URL -> plain text |
| `LlmClient`        | `OpenAiCompatible` (incl. Ollama), `HarnessLlm`, `CmdLlm` (`cmd:<template>`) | chat completions         |

Combinators in `vygr-providers` give the multi-provider behavior:

- **Fallback chain** (`"ddgs,brave"`): first provider with a non-empty result
  set wins; failures degrade to the next.
- **Fan-out** (`--all`): concurrent search across providers, merged and
  deduplicated by normalized URL, recording which providers returned each hit.

## LLM backends (ADR-0004)

`--llm <spec>` resolves to a backend:

- `pi` / `claude` / `codex`: **harness shell-out**. vygr runs the harness in
  print mode and pipes the prompt through stdin (argv carries a short
  instruction), reusing the harness's configured model and credentials with
  no MCP pass-through. `codex` keeps the argv channel with a size guard.
- `cmd:<template>`: **generic shell-out** (ADR-0019). The template runs via
  `sh -c`, receives the prompt on stdin, and its stdout is the completion —
  gemini-cli, aider, or any local wrapper works with zero glue code. Cost
  is untracked.
- `ollama:<model>`: **native** `/api/chat` client with `think: false` and
  `format: "json"` (ADR-0013); the OpenAI-compatible endpoint ignores
  thinking control, which reasoning models need disabled.
- `openai-compat` + `[llm] base_url/model`: any custom endpoint; reasoning
  models answered via the `reasoning` field are supported (`<think>`
  blocks stripped).
- `<provider>:<model>`: any models.dev provider with an OpenAI-compatible
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

Only distilled reflection notes feed later stages; raw pages never
re-enter the loop (Tavily lesson). The budget guard runs before every
reflection and before synthesis. A `--output-schema` file switches
synthesis to JSON-constrained output with parse-level validation.
After ranking, syndicated copies (near-identical titles) are clustered
and annotated as one evidence chain; each source carries its
best-matching passage for citation-precise synthesis (ADR-0017).
`--mode quick|standard|deep|exhaustive` presets fill depth/breadth/fetch
defaults under the explicit flags (ADR-0017).

A run can continue an earlier one: `vygr followup <run> "q"` and
`vygr monitor run <name>` seed the loop with the parent's notes,
sources and visited URLs, so searches only hunt for what is missing and
synthesis emphasizes what changed (ADR-0018).

Every run writes an artifact directory (ADR-0009):

```
.voyager/20261001-142233-rust-vs-zig/
├── prompt.md        the question + run parameters
├── plan.json        sub-queries and chosen depth
├── reflections.json distilled notes, follow-ups, disagreements per level
├── sources.json     scored sources (with content, clusters, best quotes)
├── claims.json      claim-level verification table (synthesis runs)
├── answer.md        synthesized report
├── run.json         manifest: query, depth, subqueries, reflections, parent
├── parent.json      inherited-evidence marker (follow-up runs only)
└── eval.json        scorecard (written by `vygr eval`)
```

The base directory is `./.voyager` by default, overridable per run
with `--run-dir`, the `VGR_RUN_DIR` env var, or `[research] run_dir`.
`vygr runs list|show|resume` reads these manifests; resume re-synthesizes
a finished run offline from its saved evidence.

## Configuration precedence

```
CLI flags > env (VGR_DEFAULT_PROVIDER, VGR_LLM_BACKEND, VGR_LLM_MODEL, VGR_RUN_DIR)
         > project .voyager.toml (walked up from cwd)
         > user ~/.config/voyager/config.toml
         > defaults
```

Config file sections merge **per field**: a file only overrides the keys
it actually sets, so a project `.voyager.toml` cannot reset user-level
settings it does not mention. API keys are never written by vygr;
providers read their own env vars (`BRAVE_API_KEY`, `TAVILY_API_KEY`, …).
`vygr providers` reports readiness.

## CLI machine contract (ADR-0008, ADR-0016)

- `--format json` on every data command (`jsonl` on search): machine
  output on **stdout**; a `main.rs` test pins this per command.
- Diagnostics, warnings and logs: **stderr** only.
- Exit codes: `0` ok, `1` internal, `2` usage (clap), `3` config/auth,
  `4` provider/network — docs are drift-tested against `error.rs`.
- Queries accept `-` to read from stdin.
- `vygr schema` walks the built clap tree (`crate::cli_command()`), so the
  self-description (commands, flags, defaults, enum choices) is generated
  from the parser definition and cannot drift.
- `research --events` streams NDJSON lifecycle events on stdout through a
  dedicated `EventSink` beside the human `ProgressSink` (stderr); the
  final `report` line carries the whole `ResearchReport`.
- `vygr doctor` runs environment self-checks (config, providers, LLM
  reachability, dir writability, skill freshness) and exits 3 when
  something is broken.

## Distribution (ADR-0011, ADR-0016)

- Single static binary via `cargo install` / GitHub releases.
- Agent skill at `skills/vygr/` (SKILL.md + `references/`, progressive
  disclosure); the canonical copy lives in `crates/cli/skill/` and drift
  tests pin the repo copy to it. `vygr init --agent <harness>`
  (pi, claude-code, codex, cursor, omp, opencode, gemini-cli, generic)
  installs the full directory; `--detect` probes the running harness;
  `--mcp` merges the MCP server into a project `.mcp.json` where the
  harness reads one.
- `npx skills add <owner>/voyager` once public (vercel-labs/skills registry).
- **MCP server** (`vygr serve`, stdio): hand-rolled newline-delimited
  JSON-RPC 2.0 exposing `search`, `extract`, `research`, `get_artifact`,
  `list_runs` and `get_run` (paged, size-capped; paths restricted to the
  working directory), run artifacts as MCP resources
  (`vygr://run/<name>/<artifact>`), and packaged prompts (`compare`,
  `landscape`). Register it with `pi mcp add voyager -- vygr serve` or
  `vygr init --agent claude-code --mcp`.
- **MCP over HTTP** (`vygr serve --http <ip:port>`, ADR-0020): the same
  JSON-RPC surface as Streamable HTTP — a stateless `POST /mcp` takes
  one message (`initialize` mints an `Mcp-Session-Id` response header),
  and `GET /mcp` is the server→client SSE stream (keepalive comments
  plus `notifications/progress`, fanned out per session). With
  `--token` / `VGR_MCP_TOKEN` every request must carry
  `Authorization: Bearer <secret>` (401 otherwise); a non-loopback bind
  without a token is refused at startup (exit 3).
- Reports export to Word or PDF with
  `vygr runs show <name> --export report.docx|report.pdf` (ADR-0019;
  the PDF writer is hand-rolled — base-14 fonts, no new dependencies,
  ADR-0020).
