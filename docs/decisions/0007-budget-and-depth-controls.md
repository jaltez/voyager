# ADR 0007: Depth and budget controls (user-bounded, LLM-decided)

- **Date:** 2026-10-01
- **Status:** Accepted

## Context

The requirement: let the user pin minimums and maximums of depth/searches, or
leave the decision to the LLM. GPT Researcher exposes `BREADTH`/`DEPTH`/
`CONCURRENCY` with breadth halving per level; Librarium adds the missing
half: **hard cost budgets** with preflight estimates and confirmation, and
the rule that unknown cost is never treated as zero.

## Decision

- `--depth "3"` (fixed) or `--depth "2..4"` (range): within a range the
  **planner LLM chooses** the actual depth; it is clamped to the range and
  recorded in `plan.json`. Config defaults live in `[research] depth_min/
  depth_max`.
- `--breadth N` bounds sub-queries per pass.
- `--budget-usd X` is a hard stop on synthesis: LLM costs are accumulated
  per call (where the backend knows its price table) and if the budget is
  already exceeded, vygr returns sources without an answer plus a warning;
  it never silently overspends.
- `vygr plan` is the offline preflight: sub-queries, searches, pages and
  LLM calls the run would perform, with no network traffic.

The scaffold enforces the budget at one checkpoint (before synthesis);
per-call enforcement lands with iterative depth in phase 2.

## Consequences

- "Leave it to the LLM" and "pin it down" are the same flag with a range vs
  a fixed value; no separate auto mode to document.
- Cost tracking only exists for backends with known prices (catalog-based);
  harness and Ollama runs report no cost, not zero cost.
