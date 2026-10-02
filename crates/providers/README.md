# vygr-providers

Search and fetch providers for the [vygr](https://crates.io/crates/vygr)
deep research CLI: eight web-search backends (keyless DuckDuckGo, Brave,
Tavily, SearXNG, Exa, Serper, Jina, Kagi), an HTTP page fetcher with
structured markdown extraction, plus the multi-provider combinators:
ordered fallback chains, concurrent fan-out with URL dedup, disk cache
with query-class TTLs and per-provider rate limiting with retry/backoff.

```rust
use vygr_core::config::{CacheConf, PolitenessConf, SearchStackConf};
use vygr_providers::{build_chain, search_chain, CacheOptions};
use vygr_core::provider::SearchQuery;

let stack = SearchStackConf {
    politeness: PolitenessConf::default(),
    cache: CacheConf::default(),
    providers: Default::default(),
};
let handle = build_chain("ddgs,brave", reqwest::Client::new(), &stack, &CacheOptions::default())?;
let (results, warnings) = search_chain(&handle.providers, &SearchQuery::new("rust wasm", 5)).await?;
```

- Repository: <https://github.com/jaltez/voyager>
- Built on [`vygr-core`](https://crates.io/crates/vygr-core)

MIT licensed.
