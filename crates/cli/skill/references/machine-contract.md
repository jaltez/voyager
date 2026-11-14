# vygr machine contract

Everything an agent needs to drive vygr programmatically. `vygr schema`
prints this live as JSON (derived from the CLI, so it never drifts) — prefer
it when unsure.

## Output discipline

- Data goes to **stdout**; every diagnostic goes to **stderr** (prefixed
  `vygr:`). The two never mix in JSON modes.
- `--format json` exists on every data command (`search`, `extract`,
  `research`, `providers`, `models`, `config`, `runs`). `search` also offers
  `jsonl` (one JSON object per result — stream/pipe friendly) and `urls`.
- A query argument of `-` reads the query from stdin:
  `echo "..." | vygr search - --format json`.

## Exit codes

| code | meaning |
|---|---|
| 0 | success |
| 1 | internal error (bug, io, json) |
| 2 | usage error (bad flags — clap) |
| 3 | configuration or auth error (missing key, bad config) |
| 4 | provider or network failure (429/5xx/timeouts, no results) |

Exit code 4 with "no results" after a legit search usually means the query
was too narrow — reframe rather than retry verbatim.

## Key commands

- `vygr providers --format json` — which search backends are ready; env var
  names only (values are never printed). Keyless by default: `ddgs` +
  `openalex` (+ `s2` with `S2_API_KEY`).
- `vygr models [provider] --format json` — models.dev catalog: models and
  $/1M pricing for `--llm` specs.
- `vygr doctor --format json` — one-shot environment check: config layers,
  provider readiness, LLM backend reachability, cache/run-dir writability,
  installed skill freshness. Exit 3 when something is broken.
- `vygr plan "q"` — offline preflight of a research run (depth, sub-query
  count, estimated LLM calls); always JSON, no network.
- `vygr eval <run-prefix>` — deterministic quality scorecard for a
  finished run (citation resolvability, coverage, diversity); also
  written to `eval.json` in the run dir.

## Research run options

- `--mode quick|standard|deep|exhaustive` — presets for depth/breadth/
  fetch; explicit `--depth`/`--breadth` flags win over the preset.
- `--depth "3"` or `"2..4"` (LLM picks within range), `--breadth N`,
  `--budget-usd X` (hard cap; vygr degrades to a deterministic report or
  abstains rather than overshooting).
- `--provider "ddgs,brave"` fallback chain; time-range/domain/language
  filters come from config (`[research]` in `.voyager.toml`).
- `--output-schema <file>` — JSON Schema the answer must satisfy; output is
  a JSON object (best-effort enforcement across backends; parse-validated).
- `--events` — NDJSON on stdout: `run_start`, `plan`, `level_start`,
  `sources_found`, `reflection`, `synthesis_start`, `verified`, `done`,
  `warning`; final line `{"event":"report","report":{...}}` with the whole
  report. Use this to render progress or kill long runs early.
- `--brief-only` — print only the report's Summary section.
- `--run-dir <path>` (or `VGR_RUN_DIR`) — where artifacts land; default
  `./.voyager`.

## Runs and artifacts

- `vygr runs list --format json` — past runs (newest first); over MCP the
  same surface exists as the `list_runs` tool, `get_run` (paged artifact by
  name prefix) and `vygr://run/<name>/<artifact>` resources, plus packaged
  prompts (`compare`, `landscape`).
- `vygr runs show <name-prefix>` — print one `answer.md`.
- `vygr runs resume <name-prefix> --llm <spec>` — offline re-synthesis from
  the saved evidence (no new searches); overwrites that run's `answer.md`.
- `vygr runs show <name-prefix> --export report.docx|report.pdf` — export a
  run's answer as a Word document or a PDF (hand-rolled writer).
- `vygr followup <run-prefix> "question"` — continue an earlier
  investigation: parent notes and sources are inherited, parent URLs are
  never refetched, the report emphasizes what changed. `run.json` records
  the `parent` link.
- `vygr monitor add <name> "topic"` / `monitor list` / `monitor run
  <name>` — recurring checks: each run continues from the previous one
  (trend snapshots without any memory machinery).
- Artifact files: `answer.md`, `sources.json` (citations index into this
  order; carries `cluster_id`/`origin_count` for syndicated copies and
  `best_quote` passages), `claims.json` (per-claim verification verdicts),
  `plan.json`, `reflections.json` (notes, follow-ups, disagreements),
  `run.json` (manifest: depth, iterations, cost, warnings, parent),
  `prompt.md`.

## MCP server (`vygr serve`)

stdio, newline-delimited JSON-RPC 2.0. Tools: `search`, `extract`,
`research`, `get_artifact` (offset-paged, size-capped; paths restricted to
the working directory). Long `research` calls emit `notifications/progress`
so clients with request timeouts (e.g. pi's 60s) stay alive. Register once
per harness: `pi mcp add voyager -- vygr serve`, or `.mcp.json`:

```json
{ "mcpServers": { "voyager": { "command": "vygr", "args": ["serve"] } } }
```

Streamable HTTP alternative: `vygr serve --http 127.0.0.1:8080` serves the
same JSON-RPC surface at `POST /mcp` — one stateless request per message;
`initialize` mints an `Mcp-Session-Id` response header — and `GET /mcp`
streams `notifications/progress` (for `research` calls carrying a
`progressToken`) plus `: keepalive` comments every ~15s. With `--token` or
`VGR_MCP_TOKEN` set (prefer the env var: a CLI flag is visible in `ps`
and shell history), every request must carry `Authorization: Bearer
<token>` (401 with a `WWW-Authenticate: Bearer` challenge); a non-loopback
bind without a token is refused (exit 3), and requests naming a
non-loopback `Host` or `Origin` get 403 (DNS-rebinding guard).

## Config

Precedence: CLI flags > `VGR_DEFAULT_PROVIDER` / `VGR_LLM_BACKEND` /
`VGR_LLM_MODEL` / `VGR_RUN_DIR` env > project `.voyager.toml` >
`~/.config/voyager/config.toml` > defaults. Sections merge per field: a
project file setting one key does not reset other layers' keys. API keys
are read from the environment only — never store them in config files.
`vygr config --format json` shows the effective result and which files
contributed.
