# ADR 0020: Deferred items delivered (safe_search, MCP schemas, HTTP transport, PDF export, browser validation)

- **Date:** 2026-11-07
- **Status:** Accepted (implemented in 0.9.0)
- **Background:** the deferred list after phases 7-9 (ADR-0019 carried
  three of them forward with "researched, low priority" verdicts): Tavily
  `safe_search` passthrough, MCP `outputSchema`, the Streamable HTTP
  transport for `vygr serve`, PDF report export (rejected as typst, kept
  as a need), and real-world validation of the headless-browser fetch
  rung. Each had a known shape; this phase ships them and closes the
  list except for full OAuth on the MCP HTTP server.

## Decision

1. **Tavily `safe_search` passthrough.** `SearchQuery` gains a tri-state
   `safe_search: Option<bool>` (`with_safe_search`); `vygr search
   --safe-search` sets it. Tavily adds the native `safe_search` body
   parameter only when the field is set (omitted otherwise, so the API
   default applies); every other provider ignores the field. The value
   joins the disk-cache key, so a safe and an unsafe search of the same
   query never share an entry. The CLI flag is presence-based (on only);
   the core field stays tri-state so config or programmatic callers can
   pass an explicit `false`.
2. **MCP `outputSchema` + `structuredContent`, additive.** The four tools
   whose results are pure JSON — `search`, `extract`, `list_runs`,
   `get_run` — declare an `outputSchema` in `tools/list`, and their
   results carry the payload twice: serialized as text and mirrored as
   `structuredContent` from the same JSON `Value`, so the two cannot
   drift. MCP requires `structuredContent` to be a JSON object, so
   array-shaped results ride in a wrapper key (`extract` ->
   `{"pages": [...]}`). `research` (markdown answer) and `get_artifact`
   (paged text) stay text-only. Clients that ignore `structuredContent`
   keep working.
3. **Streamable HTTP transport.** `vygr serve --http <ip:port>` serves
   the same JSON-RPC surface as stdio over a hand-rolled HTTP/1.1
   listener on tokio (no SDK, no new dependencies, same style as the
   stdio loop). `POST /mcp` is stateless per request: one message in,
   routed through the shared `handle_message`, `application/json` out —
   notifications (no id) are acknowledged `202 Accepted`. `initialize`
   mints an opaque `Mcp-Session-Id` (pid + counter + clock nanos)
   returned as a response header; later requests carry it back so
   progress reaches the right stream. `GET /mcp` is the server→client
   `text/event-stream`: `: keepalive` comments every ~15s plus
   `notifications/progress` fanned out per session; a client that never
   connects GET just misses progress, which the spec allows. With
   `--token` / `VGR_MCP_TOKEN` every request must carry
   `Authorization: Bearer <secret>` (checked without early exit on
   mismatch; 401 with a JSON-RPC error body otherwise), and a
   non-loopback bind without a token is refused at startup
   (`VygrError::Config`, exit 3). Hardening from the transport review:
   every request passes a DNS-rebinding guard — a non-loopback `Host` on
   loopback binds, or any foreign `Origin`, is refused with 403, per the
   Streamable HTTP security warning that makes Origin validation a MUST —
   the 401 carries a `WWW-Authenticate: Bearer` challenge, and the parser
   refuses conflicting duplicate `Content-Length` values and any
   `Transfer-Encoding` (request-smuggling classes) while handing
   pipelined bytes to the next request instead of dropping them.

   **Consequences, honestly:** a full OAuth Resource-Server dance
   (authorization-code flow, client registration, token introspection,
   RFC 9728 protected-resource discovery) remains open — and remains
   open deliberately. Hand-rolling OAuth in a research CLI is a security
   liability, not a feature gap: a bespoke implementation is where
   subtleties (redirect validation, state handling, scope downgrading)
   go quietly wrong. The static bearer token covers the realistic
   deployment — local and LAN use, where the operator owns both ends and
   TLS terminates elsewhere or not at all. One cheaper channel it does
   not cover: a secret passed as `--token SECRET` is visible in `ps` and
   shell history to every local user, so `VGR_MCP_TOKEN` is the
   recommended channel (`--token-file` remains open if a flag-only
   deployment ever needs one).
4. **PDF report export.** `vygr runs show <name> --export report.pdf`
   writes the report with a minimal hand-rolled PDF 1.4 writer — zero
   new dependencies, the typst toolchain ADR-0019 deemed disproportionate
   stays out. The object graph is the smallest conforming one (catalog,
   pages tree, one page object + content stream per page) and the fonts
   are the base-14 Type1 trio (Helvetica, Helvetica-Bold, Courier,
   WinAnsiEncoding), which need no embedding; glyphs outside WinAnsi
   render as `?`. Text is drawn with the base operators (BT/Tf/Td/Tj/ET)
   and byte offsets are tracked during serialization so the xref table
   is exact. The markdown subset covered is the same one the DOCX
   converter handles: headings, lists, fenced code, block quotes,
   paragraphs; inline markup is flattened.
5. **Browser rung validated against real Chrome.** ADR-0019's acceptance
   test is now run rather than compile-verified:
   `browser::tests::missing_browser_degrades_cleanly` probes the usual
   chrome binary names and is decisive in both directions — with a
   binary installed it must actually render `example.com` end-to-end
   (Ok, non-empty extracted text); without one it must degrade to a
   clean `Config`/`Network` error. Run against `google-chrome-stable`
   (Chrome 151), the Ok arm holds: the last escalation rung is real, not
   just compiled.

## Consequences

- `vygr search --safe-search` is one more flag the cache keys on;
  repeating a search with a different safe-search setting is a miss, not
  a stale hit.
- MCP clients see richer `tools/list` metadata and can consume
  `structuredContent` when they want typed results; older clients are
  unaffected (the change is strictly additive).
- `vygr serve --http` lets browser-based and remote MCP clients attach
  to the same tool surface stdio clients use; token-less deployments are
  loopback-only by construction.
- Run reports export to Word and PDF from the same `--export` flag,
  dispatched by extension, with no new dependencies in either writer.
- The only remaining deferred item is full OAuth Resource-Server support
  for the MCP HTTP server, intentionally left for when a deployment
  actually needs multi-user auth.
