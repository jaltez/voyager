# ADR 0019: Compatibility and reach (musl, arXiv, cmd: LLMs, MCP surface, export, browser)

- **Date:** 2026-11-06
- **Status:** Accepted (implemented in 0.8.0)
- **Background:** the ROADMAP's phase 4/5 leftovers plus the deferred
  compatibility list. Each item unblocks a concrete audience: static
  binaries (older hosts), paper-heavy research (arXiv), arbitrary local
  LLM wrappers (gemini-cli, aider, scripts), richer MCP clients, and
  Word-consuming report readers.

## Decision

1. **musl static targets.** `x86_64-unknown-linux-musl` joins the
   release matrix; rustls already avoids OpenSSL, so no cross linker is
   needed for the x86_64 musl triple.
2. **arXiv provider.** Keyless Atom API (`export.arxiv.org`), parsed
   with a focused hand-rolled extractor (the feed is machine-generated;
   no XML dependency). `canonical("arxiv")`, `BUILTIN`, 3s politeness
   spacing (the API's stated courtesy rate). Entries without a usable
   id are skipped, not guessed.
3. **Generic LLM shell-out.** `--llm "cmd:<template>"` runs the template
   via `sh -c`, pipes the prompt to stdin, reads the answer from stdout.
   Cost is untracked (the template owns its pricing). This retires the
   deferred "persistent RPC/attach backend": any wrapper — gemini-cli,
   aider, a local script — is now one spec away.
4. **MCP surface expansion.** Two new tools: `list_runs` (recent runs)
   and `get_run` (paged read of a run's artifact by name prefix, no
   paths needed). Run artifacts are exposed as MCP **resources**
   (`vygr://run/<name>/<artifact>`, whitelist of the six artifact files,
   size-capped reads). Two packaged **prompts** (`compare`, `landscape`)
   hand the host agent ready-to-run research instructions. Tools keep
   returning JSON text; `outputSchema` stays deferred because every
   client handles text content today and structured content would add a
   second result format to keep honest.
5. **DOCX report export.** `vygr runs show <name> --export report.docx`
   converts the answer with a focused markdown→docx converter (headings,
   lists, fenced code, block quotes; inline markup flattened, `[n]`
   citations preserved). Heading styles are defined by the converter —
   docx-rs ships no default style table. PDF export stays out: a
   markdown report printed via typst/HTML adds a toolchain dependency
   disproportionate to the need. **Status note:** PDF export has since
   shipped too — `--export report.pdf` runs a hand-rolled minimal PDF 1.4
   writer (three base-14 fonts, no embedding, zero new dependencies), so
   the toolchain stays out while the gap closes.
6. **Headless-browser fetch.** The escalation ladder's last rung, behind
   the `browser` feature: after plain HTTP *and* the Jina reader fail,
   headless Chrome (chromiumoxide 0.9, CDP) renders the page and the
   plain fetcher's extraction runs on the result. One browser process
   per escalated URL (escalations are rare by design); without a chrome
   binary the rung fails cleanly and the ladder keeps the previous
   result. **Status note:** exercised against a real browser
   (`google-chrome-stable` renders example.com end-to-end; the decisive
   test `browser::tests::missing_browser_degrades_cleanly` asserts the
   Ok arm when a chrome binary is probed and the degradation arm
   otherwise). chromiumoxide is handed the probed binary path
   explicitly (its own canonicalization does not search PATH) and runs
   sandbox-less when the process is root, where Chrome refuses to
   sandbox.

## Consequences

- The release matrix grows by one target; `install.sh` needs no change
  (it already prefers the platform archive).
- `EscalatingFetch` gains a `browser` toggle (default: on when compiled
  with the feature), keeping the ladder testable without chrome.
- Eleven search providers now ship in the binary; `vygr providers` and
  `--all` see arXiv automatically via `BUILTIN`.
- MCP clients built before 0.8 see the extra tools/resources in
  discovery and ignore what they do not use — the change is additive.
