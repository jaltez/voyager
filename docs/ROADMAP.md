# Roadmap

Version tags mark phase boundaries: `v0.1.0` = phase 0 scaffold,
`v0.2.0` = phase 1 complete. Effort: S (< half day) · M (~a day) · L
(multi-day).

## Phase 0 — scaffold (done, 2026-10-01, `v0.1.0`)

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

## Phase 1 — search quality of life (done, `v0.2.0`)

Design fixed by [ADR-0010](decisions/0010-caching-and-politeness.md).

### M1.1 Per-provider rate limiting and retry with backoff — effort M

- [x] `VygrError::Provider` carries `status: Option<u16>`; helper
      `should_retry(&VygrError)` matches 429/5xx/connection errors.
- [x] `throttle.rs`: `RateLimiter` with per-provider minimum interval,
      enforced in-process and across processes via a
      `<cache_dir>/politeness/<provider>.last` timestamp file.
- [x] `RetryDecorator` around every provider: exponential backoff with
      jitter (base 500ms, factor 2, max 2 retries), retriable errors only.
- [x] Config: `[politeness]` (default 250ms interval, 2 retries; override
      `ddgs = 1200ms` to avoid the HTML endpoint's anti-bot 202).
- **Acceptance**: two consecutive `vygr search` invocations observe the
  minimum interval (visible with `-vv`); a mocked 429-then-success provider
  is recovered by the decorator (unit test).

### M1.2 Disk cache with query-class TTLs — effort M

- [x] `cache.rs`: `CachedSearchProvider` decorator; key =
      sha256(provider + normalized query + max_results + filters) stored at
      `<cache_dir>/search/<provider>/<hash>.json` (`{created_at, ttl,
      results}`).
- [x] Query classifier: News (default 300s), Standard (3600s), Reference
      (86400s) by keyword heuristics; all TTLs configurable in `[cache]`.
- [x] Cache report plumbing: `search_chain`/`search_all` return hits/misses;
      `--format json` includes a `"meta": {"cache": {...}}` block.
- [x] CLI: `--no-cache`, `--cache-ttl <secs>`, `vygr cache clear`,
      `vygr cache dir`.
- **Acceptance**: repeating a search serves the second call from cache
  (instant, `meta.cache.hits = 1`); corrupted cache entries are ignored and
  re-fetched.

### M1.3 Query filters: time range and domains — effort S

- [x] `SearchQuery` gains `time_range` (day/week/month/year) and
      include/exclude domain lists.
- [x] Provider mapping: ddg `df=`, brave `freshness`, tavily native
      `time_range`/`include_domains`/`exclude_domains`; others post-filter
      by domain suffix and ignore unsupported time ranges (documented).
- [x] CLI flags on `search` (and config passthrough for `research`);
      filters are part of the cache key.
- **Acceptance**: `--include-domains rust-lang.org` returns only that
  domain; per-provider param construction is unit-tested.

### M1.4 Providers: searxng, exa, serper, jina, kagi — effort M/L

- [x] `ProviderConf` gains `base_url` (searxng self-hosted instances).
- [x] One backend per provider + registry/aliases/env requirements:
      searxng (`format=json`, keyless, needs base_url), exa (`EXA_API_KEY`),
      serper (`SERPER_API_KEY`), jina (`JINA_API_KEY`, markdown response —
      tolerant parser), kagi (`KAGI_API_KEY`).
- [x] `vygr providers` readiness includes searxng base_url detection;
      README/docs provider table updated.
- **Acceptance**: `vygr providers` lists all 8 backends with their env var
  or base_url requirement; every parser has a fixture-based unit test.

### M1.5 Structured extraction — effort M

- [x] Rewrite `HttpFetch::extract_html`: prefer `article` / `[role=main]` /
      `main`, drop nav/footer/aside/header/script/style/form, and walk
      blocks (p -> paragraph, h1-h6 -> markdown headings, li -> list items,
      pre/code -> fenced blocks).
- [x] Fixture with boilerplate noise proving removal + structure retention.
- **Acceptance**: `vygr extract <article-url>` produces readable structured
  markdown instead of one collapsed line.

## Phase 2 — the full iterative loop (done, `v0.3.0`) — the full iterative loop

- [x] Depth levels with breadth halving per level (GPT Researcher model).
- [x] Reflection buffer: distilled notes feed the next iteration, not raw
      tool outputs (Tavily deep-research lesson).
- [x] Follow-up query generation from gaps found in the current evidence.
- [x] BM25 context scoring replacing term-overlap placeholder.
- [x] Real cost accounting: accumulate LLM costs, enforce `--budget-usd`
      before every expensive call, report `cost_usd` in reports.
- [x] `--output-schema` (JSON Schema-constrained answers).
- [x] `HarnessLlm` passes the prompt via stdin instead of argv (ARG_MAX).

## Phase 3 — MCP server (done, `v0.4.0`) — MCP server

- [x] `vygr serve`: stdio MCP server (rmcp) exposing `search`, `extract`,
      `research`, `get_results` (cursor-paged, size-capped like Librarium),
      `check_run`.
- [x] Full evidence stays on disk; the MCP surface only pages summaries.

## Phase 4 — distribution

- [x] Release workflow builds linux/macOS x64/arm64 tarballs on tag push;
      `install.sh` prefers prebuilt binaries and falls back to cargo. Pushing
      a tag to GitHub is all that remains (needs the user's repo).
- [ ] Publish skill: `npx skills add <owner>/voyager` (needs a public repo).
- [ ] `vygr init --agent <id>` covering more harnesses; detect running agent.

## Phase 5 — fetch escalation

- [ ] Optional headless-browser fetch (chromiumoxide) for JS-heavy pages,
      behind a feature flag, only when plain HTTP fails.
- [ ] PDF/DOCX report export.
