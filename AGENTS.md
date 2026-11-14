# AGENTS.md — working on voyager (vygr)

Instructions for AI coding agents (and humans) contributing to this
repository. vygr is a Rust workspace producing a single binary, `vygr`:
a configurable deep-research CLI designed to be consumed by agents.

## Build & test

```sh
cargo test --workspace                 # everything must pass
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

CI runs exactly these three plus a release build. No clippy.toml or
rustfmt.toml: default behavior only, stable toolchain
(`rust-toolchain.toml`).

## Layout

- `crates/core` — types, provider traits, config (`config.rs`), the error
  taxonomy and exit-code contract (`error.rs`, ADR-0008).
- `crates/providers` — 10 search backends (`ddgs`, `brave`, `tavily`,
  `searxng`, `exa`, `serper`, `jina`, `kagi`, `openalex`, `s2`), fetch +
  escalation ladder, cache, politeness throttle, circuit breaker.
- `crates/llm` — `LlmClient` backends: harness shell-out (pi/claude/codex),
  native Ollama, OpenAI-compatible, models.dev catalog.
- `crates/research` — planner, BM25 scoring, orchestrator, citation
  verification, run artifacts.
- `crates/cli` — clap command surface, output rendering, MCP server,
  skill installer. Dependency direction is strictly downward:
  `cli -> research -> {providers, llm} -> core`; nothing below `cli`
  knows clap exists.

## Conventions that tests enforce

- **Machine contract (ADR-0008):** every data-producing command accepts
  `--format json`; data on stdout, diagnostics on stderr; exit codes
  0/1/2/3/4 with `VygrError::exit_code` as the single source of truth.
  `main.rs` tests fail if a data command loses `--format` or docs lose an
  exit code.
- **`vygr schema` (ADR-0016)** is derived from the clap tree via
  `crate::cli_command()` — add flags to the clap definitions, never to
  the schema by hand.
- **Run artifacts (ADR-0009):** every research run writes
  `prompt.md`, `plan.json`, `reflections.json`, `sources.json`,
  `answer.md`, `run.json` under the run dir (default `./.voyager`,
  override `--run-dir` / `VGR_RUN_DIR` / `[research] run_dir`).
- **Skill copies must not drift:** `crates/cli/skill/SKILL.md` (+ its
  `references/`) is canonical; `skills/vygr/` mirrors it byte-for-byte
  and the drift tests fail otherwise. After editing the canonical, run
  `cp crates/cli/skill/SKILL.md skills/vygr/SKILL.md` and
  `cp crates/cli/skill/references/*.md skills/vygr/references/`.
  `README.md` and `crates/cli/README.md` are likewise twins.
- **Config layers merge per field** (`crates/core/config.rs`): a file's
  section only overrides the keys it sets.
- **Tests live inline** (`#[cfg(test)] mod tests`) following the
  surrounding module's style; parser tests use fixtures under
  `crates/providers/tests/fixtures/`; the full research loop has an
  integration test with a scripted LLM + local fake HTTP servers in
  `crates/research/tests/loop_integration.rs` (no network).

## Docs discipline

- Behavior changes update `docs/ROADMAP.md` phases and, when the design
  is load-bearing, get an ADR in `docs/decisions/` (next number, Status
  line says what shipped it). ADR status parentheticals are kept current
  when a phase ships.
- `README.md` documents every user-facing command and flag; it is drift-
  tested against `crates/cli/README.md`.

## Machine use of this repo's own tool

Run research from a scratch dir if you do not want `.voyager/` in
the tree, or pass `--run-dir /tmp/...`. `vygr doctor` and `vygr schema`
are the fastest way to see what the tool can do from a fresh checkout:
`cargo run -p vygr -- doctor --format json`.
