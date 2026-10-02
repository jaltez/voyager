# voyager

Configurable deep-research CLI, written in Rust. Binary: **`vygr`**.

Search the web through provider chains (keyless DuckDuckGo out of the box,
plus Brave, Tavily, Exa, Serper, Jina and Kagi behind env keys, and
self-hosted SearXNG), fetch and reduce pages to text, and run a
full plan-search-synthesize research loop over any LLM backend: a
models.dev provider, local Ollama, a custom OpenAI-compatible endpoint, or
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
| `vygr search <q>` | provider-chain search; `--all` concurrent fan-out with dedup, `--extract-top N` inlines content, `--time-range day\|week\|month\|year`, `--include-domains`/`--exclude-domains`, `--no-cache`/`--cache-ttl` |
| `vygr extract <urls>` | fetch pages, reduce to plain text |
| `vygr research <q>` | iterative loop: plan → levels (search → fetch → BM25 → reflect) → synthesize; `--depth 2..4` (LLM picks within range), `--breadth`, `--budget-usd`, `--output-schema <file>`, `--llm <spec>`, artifacts under `agents/voyager/<run>/` |
| `vygr plan <q>` | offline preflight of a research run |
| `vygr providers` | search providers, required env vars, readiness |
| `vygr models [provider]` | browse the models.dev catalog and pricing |
| `vygr schema` | machine-readable self-description (for agents) |
| `vygr config` | effective configuration + file paths |
| `vygr init --agent pi\|omp\|opencode\|…` | install the agent skill (see below) |
| `vygr cache dir\|clear` | inspect or clear the search cache |
| `vygr serve` | MCP server over stdio (`search`, `extract`, `research`, `get_artifact`) |
| `vygr update` | self-update from crates.io (`--check` reports only) |

Machine contract: `--format json` everywhere, diagnostics on stderr, exit
codes `0` ok / `2` usage / `3` config-auth / `4` provider-network, `-`
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
  agentskills.io format. Install it in any supported agent with:

  ```sh
  npx skills add jaltez/voyager        # general install (pi, Claude Code, Codex, Cursor, …)
  npx skills add jaltez/voyager -g     # global (user-level)
  npx skills add jaltez/voyager -a pi  # target a specific agent
  ```

  or, from a checkout of this repo, with the bundled installer:
  `vygr init --agent pi|claude-code|codex|cursor|omp|opencode|generic [--project]`.
- **Reuse the harness LLM**: `vygr research … --llm pi` shells out to
  `pi --print` (likewise `claude -p`, `codex exec`), so the research loop
  uses the harness's configured model and credentials. No MCP pass-through.
- **MCP**: `vygr serve` runs a stdio MCP server exposing `search`,
  `extract`, `research` and `get_artifact` (token-safe paged reads of run
  artifacts). Register with your harness, e.g.
  `pi mcp add voyager -- vygr serve`; see
  [ADR-0011](docs/decisions/0011-distribution-skill-and-mcp.md).

## Documentation

- [Architecture](docs/ARCHITECTURE.md): crates, traits, loop, config
  precedence, machine contract.
- [Decisions](docs/decisions/): ADRs covering scope, provider
  abstraction, LLM backends, the research loop, budgets, the CLI contract,
  run artifacts, caching and distribution.
- [Comparative analysis](docs/research/comparative-analysis.md): what was
  borrowed from tvly, GPT Researcher, hsearch, Web Forager and Librarium.
- [Roadmap](docs/ROADMAP.md): cache/TTL + more providers, iterative depth
  loop with reflections, MCP server, releases, browser escalation.

## Status

Phase 3 complete (`v0.4.0`): the research command runs a real iterative
loop (breadth halving, distilled reflections, BM25, per-call budget guard,
`--output-schema`), and `vygr serve` exposes the tool as an MCP server
over stdio with token-safe paged artifact reads. Search/extract/cache and
eight providers all working. Release tarballs build
automatically on tag push (CI) with an install script. Pending user
action: push to a public GitHub repo (releases + `npx skills`), and
provider API keys for live search beyond DuckDuckGo.

License: MIT.
