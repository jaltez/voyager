# Comparative analysis: the five reference tools

- **Date:** 2026-10-01
- **Purpose:** background for the ADRs — what voyager borrows from each
  tool and what it deliberately rejects.

## Summary table

| | tvly (Tavily CLI) | GPT Researcher | hsearch | Web Forager | Librarium |
|---|---|---|---|---|---|
| What it is | Official CLI for the Tavily API | Self-contained research agent | Unified CLI over 6 search APIs | MCP + skills toolkit (DDG search + Jina fetch) | Fan-out dispatcher over 34 providers |
| Language / maturity | Python, v0.1.x, young | Python, ~30k stars, very active | Python, 1 star, pre-release | Python, ~10 stars | TypeScript/Node, 133 stars |
| Search providers | Tavily only | 20+, comma-combinable | 6 (tavily/brave/serper/exa/firecrawl/jina) | DuckDuckGo + r.jina.ai | 34 providers / 41 profiles, 3 latency tiers |
| LLM | Server-side only | ~24 providers via LangChain, fast/smart/strategic tiers, Ollama | None of its own | None (the host agent) | LLMs as typed providers; no Ollama/models.dev |
| Iterative loop | No (async API call) | **Yes**: plan → parallel search → filter → write; deep mode with halving breadth | No | No (host agent follows the skill) | No: fan-out → merge → synthesize |
| Depth / budget | model mini/pro/auto | BREADTH/DEPTH/CONCURRENCY + per-step cost | mode presets | host-decided | **Best**: max-cost, estimates, preflight confirm |
| Interfaces | CLI+REPL, MCP, skills, keyless search | lib, CLI, web UI, MCP, skill | CLI, MCP, skill, `schema` | MCP + skills for 50+ agents | CLI, token-safe paged MCP, skill |
| Outputs | json/md, output-schema, citation styles | MD/PDF/DOCX + cost frontmatter | table/md/json/jsonl/urls | text to the agent | run dir artifacts, json/html |
| Lock-in | Total (vendor + credits) | Low | Paid APIs only | None | Paid profiles; no local |

## What voyager takes from each

- **tvly** — the machine contract: `--format json`, stderr diagnostics,
  exit-code taxonomy, stdin queries, async-job patterns; and their
  engineering lesson that loops should consume distilled reflections, not
  raw tool outputs. Rejected: single-vendor lock-in, server-side-only LLM.
- **GPT Researcher** — the loop itself: sub-queries planned up-front
  (determinism), bounded fan-out with URL dedup, tiered LLMs with cost
  accounting, breadth/depth knobs with per-level halving, BM25-style
  filtering without embeddings. Rejected: LangChain dependency weight,
  Python footprint, config sprawl.
- **hsearch** — multi-provider ergonomics: fallback chains, `--all`
  fan-out + dedup, `--extract-top N`, mode-aware cache TTLs, `schema`
  self-description, `usage`/`providers` introspection. Rejected: thin
  wrappers over paid research APIs as the "research" feature.
- **Web Forager** — the fetch ladder (HTTP text first, browser only on
  failure) and the methodology-as-prompt: multi-framing, disconfirming
  queries, syndication-as-one-source, reconcile-don't-average. Note: the
  "Web Forager paper" sometimes cited online does not exist; the
  toolkit is real, the architecture to port is not.
- **Librarium** — hard budgets with preflight estimates and
  unknown-cost-is-never-zero; run-artifact directories; token-safe paged
  MCP; provider profiles grouped by latency tier. Rejected: no iterative
  digging, no local LLM.

## Key sources

- Tavily: [tavily-cli](https://github.com/tavily-ai/tavily-cli) ·
  [CLI docs](https://docs.tavily.com/documentation/tavily-cli) ·
  [Building Deep Research](https://www.tavily.com/blog/research-en)
- GPT Researcher: [repo](https://github.com/AssafElovic/gpt-researcher) ·
  [config docs](https://docs.gptr.dev/docs/gpt-researcher/gptr/config) ·
  [deep research](https://docs.gptr.dev/docs/gpt-researcher/gptr/deep_research)
- hsearch: [AnyGenIO/anygen-search-cli](https://github.com/AnyGenIO/anygen-search-cli)
- Web Forager: [CyranoB/web-forager](https://github.com/CyranoB/web-forager)
- Librarium: [jkudish/librarium](https://github.com/jkudish/librarium)
- Integration context: [pi-mono](https://github.com/earendil-works/pi-mono)
  ([cli-integration](https://github.com/earendil-works/pi-mono/blob/main/packages/coding-agent/docs/cli-integration.md),
  [skills](https://pi.dev/docs/latest/skills)) ·
  [vercel-labs/skills](https://github.com/vercel-labs/skills) ·
  [models.dev/api.json](https://models.dev/api.json)
