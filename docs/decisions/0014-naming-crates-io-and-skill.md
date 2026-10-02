# ADR 0014: Naming (crates.io packages and skill name)

- **Date:** 2026-10-02
- **Status:** Accepted
- **Amends:** [ADR-0011](0011-distribution-skill-and-mcp.md)

## Context

Two naming decisions were open: the crates.io package names (the binary
crate is `vygr-cli` with bin `vygr`) and the agent-skill name
(`voyager-deep-research`). crates.io availability was verified via its
API on 2026-10-02: `voyager` is taken, while `vygr`, `vygr-cli`,
`voyager-cli`, `voyager-research` and the whole `vygr-*` family are free.

For the skill, the name is what agents use for manual invocation
(`/skill:<name>`), but auto-triggering is driven almost entirely by the
`description`; skill names are global per agent skills directory, so a
generic word risks collisions.

## Decision

- **crates.io**: the CLI package is named **`vygr`** (so `cargo install
  vygr` works), with the library family `vygr-core`, `vygr-providers`,
  `vygr-llm`, `vygr-research`. Workspace path dependencies carry
  `version = "0.4.0"` so they resolve both as path and registry deps.
- **Skill**: renamed `voyager-deep-research` → **`vygr`**, matching the
  binary the skill drives; short to invoke, collision-free, and the
  description still governs auto-triggering.
- **Packaging**: the canonical `SKILL.md` lives inside the cli package
  (`crates/cli/skill/SKILL.md`) so `cargo package` embeds it (files
  outside the package root cannot be packaged); the repository copy at
  `skills/vygr/SKILL.md` serves the `npx skills` ecosystem and a drift
  test fails CI if the two diverge. `vygr init` installs to a `vygr/`
  directory.

## Consequences

- `cargo install vygr` and `npx skills add jaltez/voyager` produce
  consistently named artifacts.
- Editing the skill requires updating the repo copy and syncing the
  canonical (or vice versa); the drift test catches forgetting.
- Publishing itself still requires a crates.io account token
  (`cargo login`), then `cargo publish -p` in dependency order.
