//! `vygr init`: install the voyager skill into an agent harness
//! (tvly-style guided setup; `npx skills add jaltez/voyager` for agents).

use std::fs;
use std::path::PathBuf;

use clap::Args as ClapArgs;
use vygr_core::VygrError;

/// Canonical skill file, packaged with the crate. The copy at
/// `skills/vygr/SKILL.md` in the repository exists for the
/// `npx skills` ecosystem; a drift test in this crate keeps them equal.
const SKILL_MD: &str = include_str!("../../skill/SKILL.md");

/// Progressive-disclosure references installed next to SKILL.md (ADR-0016).
const SKILL_REFERENCES: &[(&str, &str)] = &[
    (
        "machine-contract.md",
        include_str!("../../skill/references/machine-contract.md"),
    ),
    (
        "methodology.md",
        include_str!("../../skill/references/methodology.md"),
    ),
    (
        "recipes.md",
        include_str!("../../skill/references/recipes.md"),
    ),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
enum Agent {
    Pi,
    ClaudeCode,
    Codex,
    Cursor,
    /// Oh My Pi (pi fork by Can Bölük): ~/.omp/agent/skills
    Omp,
    /// OpenCode (Anomaly): ~/.config/opencode/skills
    Opencode,
    /// Gemini CLI: ~/.gemini/skills (best-effort; falls back to the
    /// shared `.agents/skills` convention)
    GeminiCli,
    Generic,
}

impl Agent {
    /// User-level skills directory for the harness.
    fn global_dir(self, home: &std::path::Path) -> PathBuf {
        match self {
            Agent::ClaudeCode => home.join(".claude").join("skills"),
            Agent::Cursor => home.join(".cursor").join("skills"),
            Agent::Omp => home.join(".omp").join("agent").join("skills"),
            Agent::Opencode => home.join(".config").join("opencode").join("skills"),
            Agent::GeminiCli => home.join(".gemini").join("skills"),
            Agent::Pi | Agent::Codex | Agent::Generic => home.join(".agents").join("skills"),
        }
    }

    /// Project-level skills directory for the harness. The shared
    /// `.agents/skills` convention covers pi, Claude Code, Codex, Cursor
    /// and generic agents; omp and opencode read their own directories.
    fn project_dir(self) -> PathBuf {
        match self {
            Agent::Omp => PathBuf::from(".omp").join("skills"),
            Agent::Opencode => PathBuf::from(".opencode").join("skills"),
            _ => PathBuf::from(".agents").join("skills"),
        }
    }

    /// Stable id used in CLI output (`vygr init --agent <id>`).
    fn id(self) -> &'static str {
        match self {
            Agent::Pi => "pi",
            Agent::ClaudeCode => "claude-code",
            Agent::Codex => "codex",
            Agent::Cursor => "cursor",
            Agent::Omp => "omp",
            Agent::Opencode => "opencode",
            Agent::GeminiCli => "gemini-cli",
            Agent::Generic => "generic",
        }
    }

    /// MCP registration the harness understands: write a project-level
    /// `.mcp.json`, or print the command that registers one.
    fn mcp_registration(self) -> McpRegistration {
        match self {
            Agent::ClaudeCode | Agent::Cursor | Agent::Generic => McpRegistration::ProjectJson,
            _ => McpRegistration::PrintCommand,
        }
    }
}

/// How `--mcp` registers the server for a harness.
enum McpRegistration {
    /// Merge into `./.mcp.json` (`{"mcpServers": {"voyager": ...}}`).
    ProjectJson,
    /// Print the harness's registration command/snippet.
    PrintCommand,
}

/// Every harness skill directory this binary knows how to install into,
/// for `vygr doctor`'s installed-skill check.
pub(crate) fn candidate_skill_dirs(home: &std::path::Path) -> Vec<(&'static str, PathBuf)> {
    [
        Agent::Pi,
        Agent::ClaudeCode,
        Agent::Codex,
        Agent::Cursor,
        Agent::Omp,
        Agent::Opencode,
        Agent::GeminiCli,
        Agent::Generic,
    ]
    .into_iter()
    .map(|a| (a.id(), a.global_dir(home).join("vygr")))
    .collect()
}

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Harness to install for (omit with --detect to probe the environment)
    #[arg(value_enum, long)]
    agent: Option<Agent>,

    /// Install into the harness's project skills directory instead of the
    /// user-level one
    #[arg(long)]
    project: bool,

    /// Detect the running agent from environment probes and install for it
    #[arg(long)]
    detect: bool,

    /// Also register the vygr MCP server: merges into ./.mcp.json for
    /// harnesses that read it, prints the command for the rest
    #[arg(long)]
    mcp: bool,
}

/// Best-effort detection of a running agent harness from its environment
/// variables (set inside the harness's own tool sessions).
fn agent_from_env(name: &str, value: Option<&str>) -> Option<Agent> {
    match (name, value) {
        ("CLAUDECODE", Some(v)) if v == "1" || v == "true" => Some(Agent::ClaudeCode),
        ("CLAUDE_CODE_ENTRYPOINT", Some(_)) => Some(Agent::ClaudeCode),
        ("CURSOR_TRACE_ID", Some(_)) => Some(Agent::Cursor),
        _ => None,
    }
}

fn detect_agent() -> Option<Agent> {
    ["CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT", "CURSOR_TRACE_ID"]
        .iter()
        .find_map(|k| agent_from_env(k, std::env::var(k).ok().as_deref()))
}

/// Write SKILL.md plus the references/ directory into `base/vygr`,
/// returning the installed paths.
fn install_into(base: &std::path::Path) -> Result<Vec<PathBuf>, VygrError> {
    let dir = base.join("vygr");
    fs::create_dir_all(dir.join("references")).map_err(VygrError::Io)?;
    let mut installed = vec![dir.join("SKILL.md")];
    fs::write(dir.join("SKILL.md"), SKILL_MD).map_err(VygrError::Io)?;
    for (name, contents) in SKILL_REFERENCES {
        let path = dir.join("references").join(name);
        fs::write(&path, contents).map_err(VygrError::Io)?;
        installed.push(path);
    }
    Ok(installed)
}

/// Merge the voyager MCP server into a project `.mcp.json`, preserving
/// every other key and server entry.
fn merge_project_mcp_json(path: &std::path::Path) -> Result<(), VygrError> {
    let mut root: serde_json::Value = match fs::read_to_string(path) {
        Ok(s) => serde_json::from_str(&s)
            .map_err(|e| VygrError::Config(format!("{} is not valid JSON: {e}", path.display())))?,
        Err(_) => serde_json::json!({}),
    };
    let servers = root.as_object_mut().ok_or_else(|| {
        VygrError::Config(format!("{} must contain a JSON object", path.display()))
    })?;
    let entry = servers
        .entry("mcpServers".to_string())
        .or_insert_with(|| serde_json::json!({}));
    entry
        .as_object_mut()
        .ok_or_else(|| VygrError::Config("mcpServers must be an object".to_string()))?
        .insert(
            "voyager".to_string(),
            serde_json::json!({ "command": "vygr", "args": ["serve"] }),
        );
    fs::write(
        path,
        serde_json::to_string_pretty(&root).map_err(VygrError::Json)?,
    )
    .map_err(VygrError::Io)?;
    Ok(())
}

pub fn run(args: Args) -> Result<(), VygrError> {
    let agent = match (args.agent, args.detect) {
        (Some(a), _) => a,
        (None, true) => detect_agent().ok_or_else(|| {
            VygrError::Config(
                "could not detect a running agent (probed CLAUDECODE, CLAUDE_CODE_ENTRYPOINT, \
CURSOR_TRACE_ID); pass --agent explicitly"
                    .to_string(),
            )
        })?,
        (None, false) => {
            return Err(VygrError::Config(
                "vygr init needs --agent <id> (or --detect); try: vygr init --agent pi".to_string(),
            ));
        }
    };

    let home = dirs::home_dir()
        .ok_or_else(|| VygrError::Config("cannot determine home directory".to_string()))?;
    let base = if args.project {
        agent.project_dir()
    } else {
        agent.global_dir(&home)
    };
    let installed = install_into(&base)?;
    println!("installed skill: {}", installed[0].display());
    for path in &installed[1..] {
        println!("installed: {}", path.display());
    }

    match agent.mcp_registration() {
        McpRegistration::ProjectJson if args.mcp => {
            let path = PathBuf::from(".mcp.json");
            merge_project_mcp_json(&path)?;
            println!("registered MCP server: {}", path.display());
        }
        McpRegistration::ProjectJson => {
            eprintln!("vygr: add `--mcp` to also register the MCP server (.mcp.json)");
        }
        McpRegistration::PrintCommand => {
            eprintln!("vygr: register the MCP server with: pi mcp add voyager -- vygr serve");
        }
    }
    eprintln!("vygr: `npx skills add jaltez/voyager` installs it across agents");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Files shipped as repository copies for GitHub / `npx skills` must
    /// never drift from the packaged canonicals. Skipped when the
    /// repository root is not present (e.g. the published crate).
    #[test]
    fn repository_copies_match_packaged_canonicals() {
        let pairs = [
            (
                PathBuf::from("../../skills/vygr/SKILL.md"),
                SKILL_MD,
                "skills/vygr/SKILL.md drifted from crates/cli/skill/SKILL.md",
            ),
            (
                PathBuf::from("../../README.md"),
                include_str!("../../README.md"),
                "repository README.md drifted from crates/cli/README.md",
            ),
        ];
        for (root_copy, packaged, message) in pairs {
            if !root_copy.exists() {
                continue;
            }
            let on_disk = fs::read_to_string(&root_copy).unwrap();
            assert_eq!(on_disk, packaged, "{message}");
        }
        // Progressive-disclosure references: every packaged file must have
        // a byte-identical repository copy.
        for (name, contents) in SKILL_REFERENCES {
            let root_copy = PathBuf::from(format!("../../skills/vygr/references/{name}"));
            if !root_copy.exists() {
                panic!("repository is missing skills/vygr/references/{name}");
            }
            let on_disk = fs::read_to_string(&root_copy).unwrap();
            assert_eq!(
                on_disk, *contents,
                "skills/vygr/references/{name} drifted from the packaged canonical"
            );
        }
    }

    #[test]
    fn install_writes_skill_and_references() {
        let tmp = tempfile::tempdir().unwrap();
        let installed = install_into(tmp.path()).unwrap();
        assert_eq!(installed.len(), 1 + SKILL_REFERENCES.len());
        for path in &installed {
            assert!(path.exists(), "{} missing", path.display());
        }
        let skill = fs::read_to_string(tmp.path().join("vygr").join("SKILL.md")).unwrap();
        assert_eq!(skill, SKILL_MD);
        assert!(
            skill.contains("version: 0."),
            "skill must carry a version frontmatter key"
        );
    }

    #[test]
    fn mcp_json_merge_preserves_existing_servers() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(".mcp.json");
        fs::write(
            &path,
            r#"{"mcpServers":{"other":{"command":"x"}},"custom":1}"#,
        )
        .unwrap();
        merge_project_mcp_json(&path).unwrap();
        let merged: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(merged["custom"], 1);
        assert_eq!(merged["mcpServers"]["other"]["command"], "x");
        assert_eq!(merged["mcpServers"]["voyager"]["command"], "vygr");

        // Idempotent second run.
        merge_project_mcp_json(&path).unwrap();
        let merged: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(merged["mcpServers"]["voyager"]["command"], "vygr");
    }

    #[test]
    fn detection_maps_harness_env_vars() {
        assert_eq!(
            agent_from_env("CLAUDECODE", Some("1")),
            Some(Agent::ClaudeCode)
        );
        assert_eq!(
            agent_from_env("CLAUDE_CODE_ENTRYPOINT", Some("cli")),
            Some(Agent::ClaudeCode)
        );
        assert_eq!(
            agent_from_env("CURSOR_TRACE_ID", Some("abc")),
            Some(Agent::Cursor)
        );
        assert_eq!(agent_from_env("CLAUDECODE", Some("0")), None);
        assert_eq!(agent_from_env("TERM", Some("xterm")), None);
    }

    #[test]
    fn agent_skill_directories_match_documented_paths() {
        let home = PathBuf::from("/home/tester");
        assert_eq!(
            Agent::Pi.global_dir(&home),
            home.join(".agents").join("skills")
        );
        assert_eq!(
            Agent::ClaudeCode.global_dir(&home),
            home.join(".claude").join("skills")
        );
        // omp (Oh My Pi) and opencode per their official docs.
        assert_eq!(
            Agent::Omp.global_dir(&home),
            home.join(".omp").join("agent").join("skills")
        );
        assert_eq!(
            Agent::Opencode.global_dir(&home),
            home.join(".config").join("opencode").join("skills")
        );

        assert_eq!(
            Agent::Omp.project_dir(),
            PathBuf::from(".omp").join("skills")
        );
        assert_eq!(
            Agent::Opencode.project_dir(),
            PathBuf::from(".opencode").join("skills")
        );
        assert_eq!(
            Agent::Pi.project_dir(),
            PathBuf::from(".agents").join("skills")
        );
    }
}
