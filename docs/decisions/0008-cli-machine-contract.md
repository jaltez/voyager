# ADR 0008: CLI machine contract

- **Date:** 2026-10-01
- **Status:** Accepted

## Context

`tvly` is the reference for agent-facing CLI ergonomics: `--json` on every
command, diagnostics on stderr, defined exit codes, stdin queries. hsearch
adds `--agent`-style presets and a self-describing `schema` subcommand so
LLM agents can discover the interface at runtime.

## Decision

- **Machine output is a format, not a flag**: `--format json` on every
  command that produces data (`search`, `extract`, `research`, `providers`,
  `models`, `config`). Other formats: `table` (human default), `md`, `urls`,
  `text`.
- **stdout carries data only.** Warnings, logs and run-metadata notes go to
  **stderr** (tracing writes to stderr; `RUST_LOG` respected, `-v/-vv`).
- **Exit codes**: `0` success · `1` internal · `2` usage (clap default) ·
  `3` config/auth (missing key or backend) · `4` provider/network failure.
  The mapping lives in `VygrError::exit_code`, one source of truth.
- **Stdin queries**: any command taking `<QUERY>` accepts `-`.
- **Self-description**: `vygr schema` prints a JSON description of every
  command, the LLM spec grammar and the exit-code table; agents embed it
  in prompts (the hsearch trick, one command instead of a manual).

## Consequences

- Pipelines (`vygr search - --format urls | …`) and agent loops compose
  without parsing human output.
- New commands must honor the contract to be useful; CI can lint for it
  later by checking format plumbing.
