# ADR 0005: Research loop (plan-then-execute, bounded budget of work)

- **Date:** 2026-10-01
- **Status:** Accepted (scaffold implements a single pass; iteration is phase 2)

## Context

GPT Researcher's founding insight is determinism: generate sub-queries
**up-front** from an initial look at the question, then execute; no
open-ended AutoGPT loop. Tavily's engineering blog adds the token lesson:
feed iterations **distilled reflections**, never raw tool outputs (~66%
token reduction). Librarium demonstrates the counter-shape: pure fan-out
with no loop cannot dig.

## Decision

The orchestrator runs a fixed pipeline with LLM decisions at the seams:

1. **Plan**: the LLM produces ≤ breadth sub-queries (diverse framings,
   including one disconfirming query) and picks a depth within the configured
   `[min, max]` range. Sub-queries exist before any search executes.
2. **Search**: each sub-query through the provider chain; results deduped
   across sub-queries with shared provenance.
3. **Fetch**: top-N pages; failures degrade to snippets, never abort.
4. **Score & assemble**: rank sources against question + sub-queries; build
   a bounded context (`context_max_chars`).
5. **Synthesize**: one LLM call: markdown report, `[n]` citations mapped to
   `sources.json`, mandatory "Caveats & open questions".

Phase 2 adds iteration over depth levels: breadth halves each level, and the
planner generates follow-up queries from reflection notes distilled out of
the previous level's evidence, not from raw pages.

## Consequences

- Predictable cost/time: work is bounded by breadth × results × fetch_top.
- Degradation is graceful at every step (missing keys, dead pages, empty
  results all produce warnings).
- The loop quality depends on the planner prompt; it lives in
  `vygr-research/src/planner.rs` and is unit-tested against fenced JSON.
