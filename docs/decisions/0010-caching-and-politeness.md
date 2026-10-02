# ADR 0010: Caching, rate limiting and politeness (planned)

- **Date:** 2026-10-01
- **Status:** Accepted (design); implementation is roadmap phase 1

## Context

hsearch ships the smartest cheap trick we found: **mode-aware cache TTLs**
(news results expire in minutes, reference/academic results in a day), plus
exponential backoff on 429/5xx. DuckDuckGo HTML scraping (our keyless
default) needs both politeness and resilience.

## Decision

- A disk cache keyed by `(provider, normalized query, params)` under the
  voyager cache directory, with per-query-class TTLs: short for
  news-flavored queries, long for reference-like ones. `--cache-ttl`
  overrides; `--no-cache` bypasses.
- Per-provider rate limiters (token bucket) and retry with exponential
  backoff on 429/5xx, capped attempts.
- A serialized UA policy: identify as `vygr/<version>` on APIs, use a
  browser UA only on the DDG HTML endpoint where non-browser UAs are
  refused.

Current state: the scaffold performs no caching and single-attempt requests;
this ADR fixes the shape so phase 1 lands without redesign.

## Consequences

- Repeat research on adjacent questions gets dramatically cheaper.
- Cache staleness becomes a correctness knob users must see (TTL surfaced in
  JSON metadata, hsearch-style).
