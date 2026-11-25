# voyager

Configurable deep-research CLI, written in Rust. Binary: **`vygr`**.

Search the web through provider chains (keyless DuckDuckGo out of the box,
plus Brave, Tavily, Exa, Serper, Jina and Kagi behind env keys, keyless
OpenAlex, Semantic Scholar and arXiv for papers, and self-hosted SearXNG), fetch and reduce pages to text, and run a
full plan-search-synthesize research loop over any LLM backend: a
models.dev provider, local Ollama, a custom OpenAI-compatible endpoint,
any command that reads stdin and writes stdout (`--llm "cmd:…"`), or
the LLM already configured in your agent harness (`--llm pi`, `--llm
claude`, `--llm codex`). Every research run leaves a verifiable evidence
trail on disk.

```sh
# keyless search, no config needed
vygr search "rust vs zig webassembly 2026" --max-results 5

# results + inlined page content, machine output
vygr search "oxidized borrows" --extract-top 3 --format json

# full research: reuse the LLM configured in your pi harness
vygr research "state of WebAssembly GC in 2026" --llm pi --depth 2..4

# local and cheap
vygr research "same question" --llm ollama:qwen3:8b --budget-usd 0.10

# any OpenAI-compatible provider from the models.dev catalog
vygr models openrouter          # browse models + pricing
vygr research "q" --llm openrouter:deepseek/deepseek-chat
```

## Install

```sh
cargo install vygr                          # from crates.io (installs the vygr binary)
                                            # workspace crates: vygr-core, vygr-providers, vygr-llm, vygr-research
./install.sh                                # prebuilt from GitHub releases
cargo install --path crates/cli --bin vygr  # from a checkout
```

## Commands

| command | purpose |
|---|---|
| `vygr search <q>` | provider-chain search; `--all` concurrent fan-out with dedup, `--extract-top N` inlines content, `--format jsonl` streams one JSON per result, `--time-range day\|week\|month\|year`, `--include-domains`/`--exclude-domains`, `--safe-search` (Tavily only; other providers ignore it), `--no-cache`/`--cache-ttl` |
| `vygr extract <urls>` | fetch pages, reduce to plain text |
| `vygr research <q>` | iterative loop: plan → levels (search → fetch → BM25 → reflect) → synthesize with citation verification; `--mode quick\|standard\|deep\|exhaustive`, `--depth 2..4`, `--breadth`, `--budget-usd`, `--output-schema <file>`, `--events` (NDJSON lifecycle stream), `--brief-only`, `--run-dir <path>`, `--language`, `--llm <spec>`; artifacts under `.voyager/<run>/` |
| `vygr plan <q>` | offline preflight of a research run |
| `vygr providers` | search providers, required env vars, readiness |
| `vygr models [provider]` | browse the models.dev catalog and pricing |
| `vygr schema` | machine-readable self-description derived from the CLI itself (every flag, default and choice) |
| `vygr config` | effective configuration + file paths |
| `vygr init --agent pi\|claude-code\|… [--detect] [--mcp]` | install the agent skill (+`references/`); `--detect` probes the running harness; `--mcp` registers the MCP server (see below) |
| `vygr doctor` | environment self-check: providers, LLM backend, dirs, skill freshness |
| `vygr cache dir\|clear` | inspect or clear the search cache |
| `vygr serve [--http IP:PORT] [--token SECRET]` | MCP server: stdio by default, Streamable HTTP with `--http` (`search`, `extract`, `research`, `get_artifact`) |
| `vygr update` | self-update from crates.io (`--check` reports only) |
| `vygr runs list\|show\|resume` | inspect past runs; re-synthesize one offline from its saved evidence; `show <name> --export report.docx\|report.pdf` exports a Word or PDF report |
| `vygr followup <run> <q>` | continue a finished investigation from its saved evidence; only hunts what's missing, emphasizes what changed |
| `vygr monitor add\|list\|run` | recurring research: each check continues from the previous run (trend snapshots) |
| `vygr eval <run>` | deterministic quality scorecard for a run (citations, coverage, diversity) → `eval.json` |

Machine contract: `--format json` everywhere, diagnostics on stderr, exit
codes `0` ok / `1` internal / `2` usage / `3` config-auth / `4` provider-network, `-`
reads the query from stdin. `vygr schema` explains itself.

## Configuration

`~/.config/voyager/config.toml`, project `.voyager.toml`, or env
(`VGR_DEFAULT_PROVIDER`, `VGR_LLM_BACKEND`, `VGR_LLM_MODEL`). Provider keys
are env-only (`BRAVE_API_KEY`, `TAVILY_API_KEY`). See
`vygr config` for the effective layering and
[ARCHITECTURE.md](docs/ARCHITECTURE.md) for details.

```toml
default_provider = "ddgs,brave"

[llm]
backend = "openrouter"
model = "anthropic/claude-sonnet-4.5"

[research]
depth_min = 2
depth_max = 4
breadth = 3
budget_usd = 0.50
```

## Agent harness integration

- **Skill**: [skills/vygr/SKILL.md](skills/vygr/SKILL.md) follows the
  agentskills.io format with progressive disclosure
  (`references/machine-contract.md`, `methodology.md`, `recipes.md` load
  on demand). Install it in any supported agent with:

  ```sh
  npx skills add jaltez/voyager        # general install (pi, Claude Code, Codex, Cursor, …)
  npx skills add jaltez/voyager -g     # global (user-level)
  npx skills add jaltez/voyager -a pi  # target a specific agent
  ```

  or, from a checkout of this repo, with the bundled installer:
  `vygr init --agent pi|claude-code|codex|cursor|omp|opencode|gemini-cli|generic [--project]`.
  `--detect` probes the environment for a running harness; `--mcp` also
  registers the MCP server (`vygr doctor` verifies the install later).
- **Reuse the harness LLM**: `vygr research … --llm pi` shells out to
  `pi --print` (likewise `claude -p`, `codex exec`), so the research loop
  uses the harness's configured model and credentials. No MCP pass-through.
- **Stream long runs**: `vygr research … --events` emits NDJSON lifecycle
  events on stdout (`plan`, `level_start`, `sources_found`, …, `done`),
  with the full report as the final line — render progress or kill early
  without parsing prose.
- **MCP**: `vygr serve` runs a stdio MCP server exposing `search`,
  `extract`, `research`, `get_artifact`, `list_runs` and `get_run`
  (token-safe paged reads), run artifacts as MCP **resources**
  (`vygr://run/…`), and packaged research **prompts** (`compare`,
  `landscape`). The four JSON tools declare `outputSchema` and mirror
  their payload as `structuredContent`; text content is unchanged.
  Register with your harness, e.g.
  `pi mcp add voyager -- vygr serve`, or
  `vygr init --agent claude-code --mcp` to write `./.mcp.json`; see
  [ADR-0011](docs/decisions/0011-distribution-skill-and-mcp.md),
  [ADR-0019](docs/decisions/0019-compatibility-and-reach.md) and
  [ADR-0020](docs/decisions/0020-deferred-items-delivered.md).
- **MCP over HTTP**: `vygr serve --http 127.0.0.1:8080` serves the same
  tool surface as Streamable HTTP — `POST /mcp` takes one JSON-RPC message
  (stateless per request; `initialize` mints an `Mcp-Session-Id`), `GET
  /mcp` streams server notifications (`: keepalive` every ~15s and
  `notifications/progress` for the session). With `--token` (or
  `VGR_MCP_TOKEN` — prefer the env var, a flag is visible in `ps` and
  shell history) every request must carry `Authorization: Bearer <token>`
  (401 with a `WWW-Authenticate: Bearer` challenge); a non-loopback bind
  without a token is refused (exit 3), and requests naming a non-loopback
  `Host` or `Origin` get 403 (DNS-rebinding guard).

## Documentation

- [Architecture](docs/ARCHITECTURE.md): crates, traits, loop, config
  precedence, machine contract.
- [Decisions](docs/decisions/): ADRs covering scope, provider
  abstraction, LLM backends, the research loop, budgets, the CLI contract,
  run artifacts, caching and distribution.
- [Comparative analysis](docs/research/comparative-analysis.md): what was
  borrowed from tvly, GPT Researcher, hsearch, Web Forager and Librarium.
- [Roadmap](docs/ROADMAP.md): phases 0-3 and 6-10 shipped; open items:
  skill publishing (needs a public repo) and full OAuth Resource-Server
  support for the MCP HTTP server.

## Status

Phase 10 complete (`v0.9.1`, ADR-0020): deferred items delivered —
`--safe-search` passthrough (Tavily, cache-keyed), MCP
`outputSchema`/`structuredContent` on the four JSON tools, the
Streamable HTTP transport for `vygr serve` (`--http`, bearer-token
auth), hand-rolled PDF report export alongside DOCX, and the
headless-browser fetch rung validated against real Chrome.

Phase 9 (`v0.8.0`, ADR-0019): compatibility and reach — musl
static release target, keyless arXiv provider, `--llm "cmd:<template>"`
for any local LLM wrapper, MCP `list_runs`/`get_run` tools + artifact
resources + packaged prompts, DOCX report export, and headless-browser
fetch escalation behind `--features browser`.

Phase 8 (`v0.7.0`, ADR-0017/0018): evidence quality and research
modes — `--mode` presets, claim-level `claims.json`, enforced
disagreement reconciliation, syndication clustering, best quotes +
`--brief-only`, `vygr eval` scorecards, and `vygr followup`/`vygr
monitor` for continuing and recurring investigations.

Phase 7 (`v0.6.0`, ADR-0016): agent interface maturity — clap-derived
`vygr schema` v2, NDJSON `--events` + `search --format jsonl`,
`--run-dir`/`VGR_RUN_DIR`, `vygr doctor`, the skill rewritten with
progressive disclosure (`references/`), `vygr init --detect/--mcp`, and
per-field config merging.

Phase 6 (`v0.5.0`): citation verification, MCP progress notifications,
fetch escalation (Jina reader) and PDF extraction, academic providers,
circuit breaker, offline run resume.

Historic: phase 3 (`v0.4.0`): the research command runs a real iterative
loop (breadth halving, distilled reflections, BM25, per-call budget guard,
`--output-schema`), and `vygr serve` exposes the tool as an MCP server
over stdio with token-safe paged artifact reads. Search/extract/cache and
eight providers all working. Release tarballs build
automatically on tag push (CI) with an install script. Pending user
action: push to a public GitHub repo (releases + `npx skills`), and
provider API keys for live search beyond DuckDuckGo.

License: MIT.
