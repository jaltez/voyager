# ADR 0003: Search and fetch provider abstraction

- **Date:** 2026-10-01
- **Status:** Accepted

## Context

hsearch's best ideas are cheap and proven: modes as **fallback chains** that
degrade gracefully when credentials are missing, `--all` concurrent fan-out
with URL dedup, and `--extract-top N` inlining page content in the same call.
Web Forager contributes the tool ladder: plain HTTP fetch first, browser only
when needed.

## Decision

Two small traits in `vygr-core`:

```rust
trait SearchProvider { fn id(); fn requires_env(); async fn search(&SearchQuery) -> Vec<SearchResult> }
trait FetchProvider  { async fn fetch(url, max_chars) -> FetchPage }
```

plus combinators in `vygr-providers`:

- **Chain** (`"ddgs,brave"`): ordered fallback; first non-empty result set
  wins; failures become warnings, not errors.
- **Fan-out** (`--all`): all credentialed providers concurrently, merged by
  normalized URL with provider provenance recorded per result.
- **Fetch ladder**: `HttpFetch` (plain HTTP + HTML-to-text) now;
  headless-browser escalation is a future `FetchProvider` behind a feature
  flag, attempted only when plain fetch fails (Web Forager lesson).

Provider registry is compile-time (`BUILTIN`), with alias normalization
(`ddg`/`duckduckgo` → `ddgs`). Env requirements are declarative so
`vygr providers` and fan-out filtering share one source of truth.

## Consequences

- New providers are one file + one registry entry.
- Chain semantics make the default (`ddgs,brave`) resilient without config.
- Browser escalation is deferred without blocking the current design.
