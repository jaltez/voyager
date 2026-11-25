# Roadmap

Version tags mark phase boundaries: `v0.1.0` = phase 0 scaffold,
`v0.2.0` = phase 1 complete. Effort: S (< half day) · M (~a day) · L
(multi-day).

## Phase 0: scaffold (done, 2026-10-01, `v0.1.0`)

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
- [x] Agent skill `skills/vygr/SKILL.md`.

## Phase 1: search quality of life (done, `v0.2.0`)

Design fixed by [ADR-0010](decisions/0010-caching-and-politeness.md).

### M1.1 Per-provider rate limiting and retry with backoff (effort M)

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

### M1.2 Disk cache with query-class TTLs (effort M)

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

### M1.3 Query filters: time range and domains (effort S)

- [x] `SearchQuery` gains `time_range` (day/week/month/year) and
      include/exclude domain lists.
- [x] Provider mapping: ddg `df=`, brave `freshness`, tavily native
      `time_range`/`include_domains`/`exclude_domains`; others post-filter
      by domain suffix and ignore unsupported time ranges (documented).
- [x] CLI flags on `search` (and config passthrough for `research`);
      filters are part of the cache key.
- **Acceptance**: `--include-domains rust-lang.org` returns only that
  domain; per-provider param construction is unit-tested.

### M1.4 Providers: searxng, exa, serper, jina, kagi (effort M/L)

- [x] `ProviderConf` gains `base_url` (searxng self-hosted instances).
- [x] One backend per provider + registry/aliases/env requirements:
      searxng (`format=json`, keyless, needs base_url), exa (`EXA_API_KEY`),
      serper (`SERPER_API_KEY`), jina (`JINA_API_KEY`, markdown response,
      tolerant parser), kagi (`KAGI_API_KEY`).
- [x] `vygr providers` readiness includes searxng base_url detection;
      README/docs provider table updated.
- **Acceptance**: `vygr providers` lists all 8 backends with their env var
  or base_url requirement; every parser has a fixture-based unit test.

### M1.5 Structured extraction (effort M)

- [x] Rewrite `HttpFetch::extract_html`: prefer `article` / `[role=main]` /
      `main`, drop nav/footer/aside/header/script/style/form, and walk
      blocks (p -> paragraph, h1-h6 -> markdown headings, li -> list items,
      pre/code -> fenced blocks).
- [x] Fixture with boilerplate noise proving removal + structure retention.
- **Acceptance**: `vygr extract <article-url>` produces readable structured
  markdown instead of one collapsed line.

## Phase 2: the full iterative loop (done, `v0.3.0`)

- [x] Depth levels with breadth halving per level (GPT Researcher model).
- [x] Reflection buffer: distilled notes feed the next iteration, not raw
      tool outputs (Tavily deep-research lesson).
- [x] Follow-up query generation from gaps found in the current evidence.
- [x] BM25 context scoring replacing term-overlap placeholder.
- [x] Real cost accounting: accumulate LLM costs, enforce `--budget-usd`
      before every expensive call, report `cost_usd` in reports.
- [x] `--output-schema` (JSON Schema-constrained answers).
- [x] `HarnessLlm` passes the prompt via stdin instead of argv (ARG_MAX).

## Phase 3: MCP server (done, `v0.4.0`)

- [x] `vygr serve`: stdio MCP server exposing `search`, `extract`,
      `research`, `get_artifact` (offset-paged, size-capped like Librarium);
      hand-rolled newline-delimited JSON-RPC 2.0, no SDK dependency
      (ADR-0011).
- [x] Full evidence stays on disk; the MCP surface only pages summaries.

## Phase 4: distribution

- [x] Release workflow builds linux/macOS x64/arm64 tarballs on tag push;
      `install.sh` prefers prebuilt binaries and falls back to cargo. Pushing
      a tag to GitHub is all that remains (needs the user's repo).
- [ ] Add x86_64-unknown-linux-musl targets so release binaries are fully
      static (glibc on older hosts emits weak-symbol warnings otherwise).
- [ ] Publish skill: `npx skills add <owner>/voyager` (needs a public repo).
- [ ] `vygr init --agent <id>` covering more harnesses; detect running agent.

## Phase 5: fetch escalation

- [ ] Optional headless-browser fetch (chromiumoxide) for JS-heavy pages,
      behind a feature flag, only when plain HTTP fails.
- [x] PDF/DOCX report export (DOCX in phase 9; PDF in Deferred).

## Phase 6: quality and reliability pass (done, `v0.5.0`, ADR-0015)

- [x] Progress events (CLI stderr + MCP notifications/progress; fixes
      pi's 60s MCP request timeout on long research calls)
- [x] Citation verification against stored sources (weak claims flagged);
      abstention when no usable evidence; degraded deterministic report
      on exhausted budgets; pre-call cost estimates
- [x] Fetch escalation ladder (Jina reader for thin/JS pages) and PDF
      text extraction
- [x] Structured harness output: claude JSON envelope with total_cost_usd,
      codex --output-last-message
- [x] Academic keyless providers (OpenAlex, Semantic Scholar) and a
      result-language filter
- [x] Provider circuit breaker (3 outage failures -> 60s skip)
- [x] run.json manifests and vygr runs list|show|resume with offline
      re-synthesis

## Phase 7: agent interface maturity (done, `v0.6.0`, ADR-0016)

Design fixed by [ADR-0016](decisions/0016-agent-interface-maturity.md).

- [x] `vygr schema` v2: full command/flag tree derived from the clap
      definition (never drifts), plus machine contract, artifact
      inventory, per-command examples
- [x] Machine-contract hygiene: exit code `1` documented everywhere;
      CI tests pin `--format` on every data command and the exit-code
      taxonomy in docs
- [x] Structured events: `research --events` NDJSON lifecycle on stdout
      (final `report` line carries the full report); `search --format
      jsonl`
- [x] `research --run-dir <path>` + `VGR_RUN_DIR` (artifacts out of the
      repo when the agent wants that)
- [x] `vygr doctor`: config layers, provider readiness, LLM backend
      reachability, dir writability, skill freshness, catalog age
      (exit 3 on failure)
- [x] Skills overhaul: lean SKILL.md + `references/`
      (machine-contract, methodology, recipes) installed by `vygr init`;
      `version` frontmatter; drift tests cover every packaged file
- [x] `vygr init --detect` (probe running harness), `--mcp` (merge
      `.mcp.json` / print registration), gemini-cli target
- [x] Per-field config merging (project files no longer reset
      user-layer settings); openalex/s2 join `BUILTIN` (`--all`,
      `vygr providers`); AGENTS.md for contributors
- [x] Stale ADR status parentheticals refreshed (0005, 0010)

## Phase 8: evidence quality and research modes (done, `v0.7.0`, ADR-0017/0018)

Design fixed by [ADR-0017](decisions/0017-evidence-quality-and-modes.md)
and [ADR-0018](decisions/0018-followup-and-monitor.md).

- [x] `--mode quick|standard|deep|exhaustive` presets (flags > preset >
      config); `vygr plan` understands them too
- [x] Claim-level evidence table: every checked claim recorded in
      `claims.json` (claim, citation, best source, overlap, verdict)
- [x] Disagreement reconciliation: reflection reports source conflicts,
      synthesis must reconcile them in a `## Disagreements` section
- [x] Syndication clustering: near-identical titles share a cluster id +
      origin count; evidence blocks annotated as one evidence chain
- [x] Best quotes per source (BM25-top passage in the evidence block)
      and a required `## Summary` section with `--brief-only` output
- [x] `vygr eval <run>`: deterministic scorecard (citation resolvability,
      verification, sub-query coverage, disconfirmation attempt,
      diversity) written to `eval.json`
- [x] `vygr followup <run> "q"`: continue a finished investigation from
      its saved evidence; no refetching of parent URLs; `parent` recorded
      in run.json
- [x] `vygr monitor add|list|run`: recurring research with trend
      snapshots; each check continues from the previous run and advances
      the baseline

## Phase 9: compatibility and reach (done, `v0.8.0`, ADR-0019)

Design fixed by [ADR-0019](decisions/0019-compatibility-and-reach.md).

- [x] musl static release target (`x86_64-unknown-linux-musl`)
- [x] arXiv provider: keyless Atom API, hand-rolled parser, 3s
      politeness spacing, joins `BUILTIN`
- [x] Generic LLM shell-out: `--llm "cmd:<template>"` (prompt on stdin,
      answer on stdout) — replaces the deferred RPC/attach backend
- [x] MCP expansion: `list_runs` + `get_run` tools, run artifacts as
      MCP resources (`vygr://run/...`), packaged prompts (`compare`,
      `landscape`)
- [x] DOCX report export: `runs show <name> --export report.docx`
- [x] Headless-browser fetch escalation behind `--features browser`
      (validated against real Chrome in phase 10, ADR-0020)

## Phase 10: deferred items delivered (done, `v0.9.1`, ADR-0020)

Design fixed by
[ADR-0020](decisions/0020-deferred-items-delivered.md).

- [x] Tavily `--safe-search` passthrough (`SearchQuery` field, part of
      the cache key; other providers ignore it)
- [x] MCP `outputSchema` + `structuredContent` on the four JSON tools
      (`search`, `extract`, `list_runs`, `get_run`); text content
      unchanged, purely additive. `structuredContent` is always a JSON
      object (MCP requirement — `extract` wraps its page list in
      `pages`), built from the same `Value` as the text so the two
      cannot drift
- [x] Streamable HTTP transport: `vygr serve --http <ip:port>` —
      hand-rolled HTTP/1.1, stateless `POST /mcp` + SSE `GET /mcp`
      (keepalives + progress keyed by `Mcp-Session-Id`), bearer-token
      auth via `--token`/`VGR_MCP_TOKEN`, non-loopback binds refused
      without one. Hardening from the transport review: DNS-rebinding
      guard (non-loopback `Host`/`Origin` -> 403), `WWW-Authenticate:
      Bearer` challenge on 401, conflicting duplicate `Content-Length`
      and any `Transfer-Encoding` refused, pipelined requests answered
      instead of dropped
- [x] PDF report export: hand-rolled PDF 1.4 writer (three base-14
      fonts, no font embedding, zero new dependencies — no typst)
- [x] Browser fetch rung validated against real Chrome (ADR-0019's
      acceptance test run; renders example.com end-to-end)
- [x] Default run-artifact base moved from `agents/voyager/` to
      `.voyager/` (`--run-dir`, `VGR_RUN_DIR`, `[research] run_dir`
      overrides unchanged)

## Unreleased

- [x] Fast-failing harness (`pi`/`claude`/`codex`) and `cmd:` backends
      report the command's stderr; a prompt written to a stdin the
      command already closed is no longer a broken-pipe error (which the
      harness backend classified as retriable)

## Deferred (researched, low priority)

- [ ] Full OAuth Resource-Server support for the MCP HTTP server (the
      static bearer token covers local/LAN; hand-rolled OAuth is a
      security liability, per ADR-0020)
