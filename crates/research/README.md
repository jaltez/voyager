# vygr-research

The research engine of the [vygr](https://crates.io/crates/vygr) CLI:
a plan-then-execute deep research loop: LLM-planned sub-queries up
front, iterative levels with breadth halving, distilled reflection notes
feeding follow-up searches (raw pages never re-enter the loop), BM25
source ranking, per-call budget enforcement, optional JSON-Schema
constrained synthesis, and a full evidence trail on disk
(`plan.json`, `reflections.json`, `sources.json`, `answer.md`).

```rust
let report = vygr_research::run(request, llm, http).await?;
// report.answer, report.sources, report.cost_usd, report.run_dir
```

- Repository: <https://github.com/jaltez/voyager>
- Built on [`vygr-core`](https://crates.io/crates/vygr-core),
  [`vygr-providers`](https://crates.io/crates/vygr-providers) and
  [`vygr-llm`](https://crates.io/crates/vygr-llm)

MIT licensed.
