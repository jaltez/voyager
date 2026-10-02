# ADR 0012: Iterative research loop with distilled reflections

- **Date:** 2026-10-01
- **Status:** Accepted (implemented in phase 2)

## Context

The phase-0 scaffold executed a single plan-search-synthesize pass: the
`depth` setting had no effect. Two reference insights were still unused:
GPT Researcher's breadth-halving depth tree and Tavily's lesson that
iterations should consume **distilled reflections**, not raw tool outputs
(~66% token reduction).

## Decision

`vygr research` now runs up to `depth_used` levels (the planner LLM picks
`depth_used` within `--depth min..max`):

1. **Search** the level's sub-queries (level breadth = `breadth >> level`,
   floored at 2), keeping only URLs not seen in any previous level
   (global dedup via `vygr_providers::url_key`).
2. **Fetch & score** the top fresh pages (BM25 against question + level
   queries + accumulated notes).
3. **Reflect** (skipped on the last level): the LLM receives the question,
   prior notes and a small digest of the newest evidence (≤6 KB, excerpts
   only), and returns `{notes, followups, satisfied}`. Notes (≤8, ≤200
   chars each) join the reflection buffer; followups (≤3) seed the next
   level's searches.

Graceful stops, in order of preference: satisfied reflection (no
followups), no fresh sources found, reflection failure (the run warns and
synthesizes what it has), budget exhausted (checked before every
reflection and before synthesis). Raw pages never enter the loop; only
the reflection buffer and the final bounded context window.

Synthesis consumes the ranked evidence, the reflection buffer and the
initial sub-queries; citations `[n]` index the (ranked) `sources.json`.

## Consequences

- Depth is now real: `--depth 3` means up to 3 search rounds with
  narrowing focus; `iterations` in the report records what actually ran.
- Reflection calls double the LLM call count minus one; budget tracking
  covers them, and local/free backends pay nothing.
- The evidence trail gains `reflections.json`; the answer's Caveats
  section is anchored in what the reflection pass flagged as unverified.
