# ADR 0016: Agent interface maturity (schema v2, events, doctor, skills)

- **Date:** 2026-11-06
- **Status:** Accepted (implemented in 0.6.0)
- **Background:** the research loop and provider stack are mature, but the
  agent-facing surface lags: `vygr schema` was one string per command,
  progress was prose-only, artifact placement polluted repositories, the
  skill had drifted from the shipped feature set, and there was no way to
  answer "what works in this environment?" without several probe calls.

## Decision

1. **Schema v2 is clap-derived.** `vygr schema` walks the built
   `clap::Command` tree (`crate::cli_command()`), so every command, flag,
   default, choice (ValueEnum) and nested subcommand is emitted from the
   same source the parser uses — the schema cannot drift from the
   interface. Static sections (machine contract, artifact inventory,
   LLM specs, per-command examples) live beside it.
2. **Structured event stream.** `vygr research --events` emits NDJSON
   lifecycle events on stdout (`run_start`, `plan`, `level_start`,
   `sources_found`, `reflection`, `synthesis_start`, `verified`, `done`,
   `warning`), with a final `{"event": "report"}` line carrying the whole
   `ResearchReport`. Human progress lines stay on stderr; the orchestrator
   gains a parallel `EventSink` parameter. `search --format jsonl` emits
   one JSON object per result for pipe-friendly consumption.
3. **Run-dir control.** `--run-dir <path>` / `VGR_RUN_DIR` override the
   artifact base (config `[research] run_dir` still works), so agents
   working in user repositories can keep evidence trails out of the tree.
4. **`vygr doctor`.** One command answers "what works right now": config
   layers, provider readiness, LLM backend reachability (harness binaries
   probed via `--version`, Ollama over HTTP, keyed backends via key-env
   presence), cache/run-dir writability (probe without side effects),
   installed-skill freshness, catalog cache age. Exit 3 when anything is
   broken; `--format json` for agents.
5. **Skills with progressive disclosure.** The canonical skill
   (`crates/cli/skill/`) stays lean; deep detail moves to
   `references/{machine-contract,methodology,recipes}.md` that agents load
   on demand. `vygr init` installs the full directory, carries a
   `version` frontmatter key, and the drift tests pin every packaged file
   to its repository copy. `vygr init --detect` probes harness
   environment variables (best-effort); `--mcp` merges the server into a
   project `.mcp.json` where the harness reads one and prints the
   registration command otherwise.

## Consequences

- Machine-contract regressions are caught by tests (every data command
  must keep `--format`; documented exit codes must cover `0..4`).
- The event enum is additive-only from here: new events may appear, but
  existing event shapes and the `report` final line are a stability
  promise to harnesses.
- Skill copies beyond the canonical (repo copy, installed copies) are
  re-installable and doctor-detectable; three-way drift is fixed by
  re-running `vygr init`.
- Config files merge per field now: a project `.voyager.toml` no longer
  resets user-level settings it does not mention (the old "whole section
  replaces" gotcha is gone).
