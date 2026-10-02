# ADR 0015: Quality and reliability pass (progress, verification, escalation)

- **Date:** 2026-10-02
- **Status:** Accepted (implemented in 0.5.0)
- **Background:** a 2026-10 survey of the deep-research tool landscape
  (Mole, LangChain deep agents, Librarium v2, gh-aw, pi MCP semantics)
  converged on three gaps in vygr: silent long runs, unverified
  citations, and JS/PDF blind spots.

## Decision

1. **Progress events everywhere.** The orchestrator reports through an
   injectable `ProgressSink`; the CLI prints to stderr and the MCP
   server maps each line to a `notifications/progress` JSON-RPC
   notification addressed to the client's `progressToken`. This is also
   a correctness fix: pi's MCP client kills requests after 60 s unless
   progress notifications arrive.
2. **Citation verification.** After synthesis, every sentence carrying
   `[n]` is checked by lexical overlap against the stored source texts;
   weakly supported claims are flagged in a verification section of
   `answer.md` and in the report. Deterministic heuristic, no judge
   model. Runs with no usable evidence abstain instead of fabricating;
   an exhausted budget returns a deterministic degraded report rather
   than nothing.
3. **Fetch escalation ladder.** Plain HTTP first; thin or failed HTML
   retries through the Jina reader (`r.jina.ai`, keyless under rate
   limits). `application/pdf` responses extract text via `pdf-extract`
   (silenced at the fd level so library noise cannot reach stdout).
4. **Structured harness output.** `claude -p --output-format json`
   envelopes carry the answer and `total_cost_usd` (real cost in
   harness mode); `codex exec --output-last-message` files carry the
   final answer.
5. **Resilience and reproducibility.** Academic keyless providers
   (OpenAlex, Semantic Scholar), a language filter, a per-provider
   circuit breaker (3 outage failures open for 60 s), `run.json`
   manifests, and `vygr runs list|show|resume` with offline
   re-synthesis from saved evidence.

## Consequences

- The MCP surface stays alive during minutes-long research on pi, omp
  and other progress-aware clients.
- Reports own their weaknesses: flagged claims and degraded modes are
  visible, not hidden.
- The evidence trail is now replayable: sources plus the manifest
  reproduce a synthesis without touching the network.
