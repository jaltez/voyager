# vygr-llm

LLM backends for the [vygr](https://crates.io/crates/vygr) deep research
CLI, behind one async trait (`LlmClient`):

- **OpenAI-compatible** endpoints, auto-configured from the
  [models.dev](https://models.dev) catalog (provider id, key env var, base
  URL, per-token pricing) — with reasoning-model fallback (`reasoning`
  field, `<think>` stripping)
- **Native Ollama client** (`/api/chat`) with `think: false`, auto-sized
  `num_ctx` and JSON-constrained output via `format: "json"`
- **Harness shell-out** — reuse the LLM already configured in an agent
  harness (`pi --print`, `claude -p`, `codex exec`), prompts piped through
  stdin to stay under argv limits

```rust
let llm = vygr_llm::resolve(Some("ollama:qwen3:8b"), &Default::default(), &http).await?;
let reply = llm.complete(&request).await?;
```

- Repository: <https://github.com/jaltez/voyager>
- Built on [`vygr-core`](https://crates.io/crates/vygr-core)

MIT licensed.
