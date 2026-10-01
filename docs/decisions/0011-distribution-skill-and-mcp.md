# ADR 0011: Distribution — binary, skill and MCP

- **Date:** 2026-10-01
- **Status:** Accepted (skill + binary shipped; MCP is roadmap phase 3)

## Context

Three integration surfaces emerged from the survey: tvly's `init --agent`
installs skills into detected harnesses; Web Forager distributes exclusively
as MCP + Agent Skills (50+ agents); Librarium's MCP server is deliberately
token-safe (cursor-paged results, 64KB caps, evidence on disk). The
agentskills.io spec + vercel-labs `npx skills` registry make one SKILL.md
portable across pi, Claude Code, Codex and others; pi is an MCP **client**
(there is no pi-as-server mode), so MCP exposure is something vygr must
provide itself.

## Decision

1. **Binary first**: static `vygr` via cargo / release assets. No runtime,
   no daemon.
2. **Skill**: `skills/deep-research/SKILL.md` (agentskills.io format) that
   teaches agents the command surface, the LLM-spec grammar and the research
   methodology. `vygr init --agent pi|claude-code|codex|cursor|generic
   [--project]` embeds and installs it. Once public:
   `npx skills add <owner>/voyager`.
3. **MCP server** (`vygr serve`, stdio, rmcp): expose `search`, `extract`,
   `research`, and `get_results` cursor-paged with size caps — small
   summaries in-context, full evidence via the run artifacts of ADR-0009.
   Not in phase 1; the command exists and fails with a pointer to this ADR.

Harness LLM reuse is explicitly **not** routed through MCP — it is the
shell-out backend of ADR-0004.

## Consequences

- One artifact (the skill markdown) covers every harness that reads
  SKILL.md; the binary covers everything else.
- MCP work is isolated behind `serve` and cannot complicate the core loop.
- Skill versioning rides the repo tags; `npx skills update` flows naturally.
