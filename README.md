# voyager

Configurable deep-research CLI, written in Rust. Binary: **`vygr`**.

Search the web through provider chains (keyless DuckDuckGo out of the box,
plus Brave, Tavily, Exa, Serper, Jina and Kagi behind env keys, and
self-hosted SearXNG), fetch and reduce pages to text, and run a
full plan-search-synthesize research loop over any LLM backend — a
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
cargo install --path crates/cli --bin vygr   # from a checkout
cargo build --workspace --release            # ./target/release/vygr
```

## Commands

| command | purpose |
|---|---|
| `vygr search <q>` | provider-chain search; `--all` concurrent fan-out with dedup, `--extract-top N` inlines content, `--time-range day\|week\|month\|year`, `--include-domains`/`--exclude-domains`, `--no-cache`/`--cache-ttl` |
| `vygr extract <urls>` | fetch pages, reduce to plain text |
| `vygr research <q>` | full loop: plan → search → fetch → score → synthesize; artifacts under `agents/voyager/<run>/` |
| `vygr plan <q>` | offline preflight of a research run |
| `vygr providers` | search providers, required env vars, readiness |
| `vygr models [provider]` | browse the models.dev catalog and pricing |
| `vygr schema` | machine-readable self-description (for agents) |
| `vygr config` | effective configuration + file paths |
| `vygr init --agent pi` | install the agent skill (see below) |
| `vygr cache dir\|clear` | inspect or clear the search cache |
| `vygr serve` | MCP server over stdio — not yet implemented (roadmap) |

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

- **Skill**: [skills/deep-research/SKILL.md](skills/deep-research/SKILL.md)
  follows the agentskills.io format. Install it with
  `vygr init --agent pi|claude-code|codex|cursor|generic [--project]`, or —
  once this repo is public — `npx skills add <owner>/voyager`.
- **Reuse the harness LLM**: `vygr research … --llm pi` shells out to
  `pi --print` (likewise `claude -p`, `codex exec`), so the research loop
  uses the harness's configured model and credentials. No MCP pass-through.
- **MCP**: `vygr serve` (stdio) is planned with token-safe paged results —
  see [ADR-0011](docs/decisions/0011-distribution-skill-and-mcp.md).

## Documentation

- [Architecture](docs/ARCHITECTURE.md) — crates, traits, loop, config
  precedence, machine contract.
- [Decisions](docs/decisions/) — 11 ADRs covering scope, provider
  abstraction, LLM backends, the research loop, budgets, the CLI contract,
  run artifacts, caching and distribution.
- [Comparative analysis](docs/research/comparative-analysis.md) — what was
  borrowed from tvly, GPT Researcher, hsearch, Web Forager and Librarium.
- [Roadmap](docs/ROADMAP.md) — cache/TTL + more providers, iterative depth
  loop with reflections, MCP server, releases, browser escalation.

## Status

Phase 0 scaffold (see roadmap): search/extract/models/plan/schema/config/
init are working; `research` runs a single full pass and needs an LLM
backend (`--llm …` or config). Not yet: iteration across depth levels,
caching, MCP.

License: MIT.
