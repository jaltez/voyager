# vygr-core

Shared foundation for the [vygr](https://crates.io/crates/vygr) deep
research CLI: normalized search/fetch types, the `SearchProvider` and
`FetchProvider` traits, layered configuration (user file → project file →
environment) and the error → exit-code contract.

Usable standalone to build your own provider-backed tools:

```rust
use vygr_core::provider::{SearchProvider, SearchQuery};

// implement SearchProvider for your backend, then:
// provider.search(&SearchQuery::new("rust wasm", 5)).await?
```

- Repository: <https://github.com/jaltez/voyager>
- Sibling crates: [`vygr-providers`](https://crates.io/crates/vygr-providers),
  [`vygr-llm`](https://crates.io/crates/vygr-llm),
  [`vygr-research`](https://crates.io/crates/vygr-research),
  [`vygr`](https://crates.io/crates/vygr) (the CLI binary)

MIT licensed.
