# ADR 0006: Context filtering without embeddings

- **Date:** 2026-10-01
- **Status:** Accepted

## Context

GPT Researcher historically required embeddings + a vector store for context
selection. In v3.7 (2026) they moved to usefulness-based filtering (LLM
scoring or plain **BM25 keyword ranking**), explicitly dropping the
embeddings requirement. Research runs are short-lived: the corpus dies with
the run, so an index buys nothing.

## Decision

No embeddings, no vector store. Source ranking uses lexical scoring against
the union of question + sub-query terms. The scaffold ships a term-overlap
placeholder (Jaccard-flavored, length-normalized) in `score.rs`; BM25 is a
drop-in replacement in phase 2. Per-source excerpts are hard-capped
(2,000 chars) and the assembled context is bounded by `context_max_chars`.

## Consequences

- Zero extra credentials, services or model downloads; keyless research runs
  remain keyless for scoring.
- Ranking quality below BM25 until phase 2; acceptable at scaffold scale
  (≤ breadth × max_results sources).
- If reranking-by-LLM proves valuable later, it slots in after lexical
  ranking without changing the contract.
