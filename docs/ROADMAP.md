# Roadmap

## Phase 0 — scaffold (done, 2026-10-01)

- [x] Workspace: core / providers / llm / research / cli crates.
- [x] Keyless search out of the box (ddgs) + brave + tavily behind env keys.
- [x] Fallback chains and concurrent fan-out with URL dedup.
- [x] HTTP page fetch with text extraction; `vygr extract`.
- [x] LLM backends: OpenAI-compatible (incl. Ollama) + harness shell-out
      (pi / claude / codex) + models.dev catalog with pricing (`vygr models`).
- [x] Research run: plan -> search -> fetch -> score -> synthesize, with run
      artifacts on disk and a coarse budget guard.
- [x] CLI machine contract: `--format json`, stderr diagnostics, exit codes,
      stdin queries, `vygr schema`, `vygr plan`, `vygr config`, `vygr init`.
- [x] Agent skill `skills/deep-research/SKILL.md`.

## Phase 1 — search quality of life

- [ ] Disk cache with mode-aware TTLs (news: minutes, reference: hours).
- [ ] Rate limiting / backoff per provider (429-aware).
- [ ] More providers: searxng, exa, serper, jina, kagi.
- [ ] Better extraction fidelity (readability-style, preserve headings).
- [ ] `--time-range`, `--include-domains` / `--exclude-domains`.

## Phase 2 — the full iterative loop

- [ ] Depth levels with breadth halving per level (GPT Researcher model).
- [ ] Reflection buffer: distilled notes feed the next iteration, not raw
      tool outputs (Tavily deep-research lesson).
- [ ] Follow-up query generation from gaps found in the current evidence.
- [ ] BM25 context scoring replacing term-overlap placeholder.
- [ ] Real cost accounting: accumulate LLM costs, enforce `--budget-usd`
      before every expensive call, report `cost_usd` in reports.
- [ ] `--output-schema` (JSON Schema-constrained answers).

## Phase 3 — MCP server

- [ ] `vygr serve`: stdio MCP server (rmcp) exposing `search`, `extract`,
      `research`, `get_results` (cursor-paged, size-capped like Librarium),
      `check_run`.
- [ ] Full evidence stays on disk; the MCP surface only pages summaries.

## Phase 4 — distribution

- [ ] GitHub release binaries (linux/macOS x64/arm64) + install script.
- [ ] Publish skill: `npx skills add <owner>/voyager`.
- [ ] `vygr init --agent <id>` covering more harnesses; detect running agent.

## Phase 5 — fetch escalation

- [ ] Optional headless-browser fetch (chromiumoxide) for JS-heavy pages,
      behind a feature flag, only when plain HTTP fails.
- [ ] PDF/DOCX report export.
