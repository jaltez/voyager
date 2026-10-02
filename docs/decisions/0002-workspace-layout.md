# ADR 0002: Rust workspace layout

- **Date:** 2026-10-01
- **Status:** Accepted

## Context

The tool has three natural seams (search providers, LLM backends, the
research loop) plus a CLI surface. GPT Researcher's monolithic Python package
shows what happens without enforced boundaries; its LangChain dependency tree
is the main argument for the rewrite.

## Decision

A Cargo workspace of five crates with strictly downward dependencies:

```
cli -> research -> { providers, llm } -> core
```

- **vygr-core**: types, `SearchProvider`/`FetchProvider` traits,
  configuration, error taxonomy. No I/O beyond config files.
- **vygr-providers**: search backends, fetcher, chain/fan-out combinators.
- **vygr-llm**: `LlmClient` trait + backends + models.dev catalog.
- **vygr-research**: planner, scoring, orchestrator, run artifacts.
- **vygr** (in `crates/cli/`): the `vygr` binary; the only crate that knows clap. Published to crates.io as `vygr`.

Alternatives rejected: a single crate (no seams, slow incremental builds) and
separate repos (cross-cutting trait changes would need version churn).

## Consequences

- Trait evolution is centralized in core; providers and llm stay swappable.
- The research engine is embeddable as a library by third parties.
- One more layer of ceremony for small changes; accepted.
