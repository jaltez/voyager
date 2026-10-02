//! `vygr init` — install the voyager skill into an agent harness
//! (tvly-style guided setup; `npx skills add jaltez/voyager` for agents).

use std::fs;
use std::path::PathBuf;

use clap::Args as ClapArgs;
use vygr_core::VygrError;

/// Canonical skill file, packaged with the crate. The copy at
/// `skills/deep-research/SKILL.md` in the repository exists for the
/// `npx skills` ecosystem; a drift test in this crate keeps them equal.
const SKILL_MD: &str = include_str!("../../skill/SKILL.md");

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
enum Agent {
    Pi,
    ClaudeCode,
    Codex,
    Cursor,
    Generic,
}

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Harness to install for
    #[arg(value_enum, long)]
    agent: Agent,

    /// Install into ./.agents/skills (project) instead of the home directory
    #[arg(long)]
    project: bool,
}

pub fn run(args: Args) -> Result<(), VygrError> {
    let home = dirs::home_dir()
        .ok_or_else(|| VygrError::Config("cannot determine home directory".to_string()))?;
    let base = if args.project {
        PathBuf::from(".agents").join("skills")
    } else {
        match args.agent {
            Agent::ClaudeCode => home.join(".claude").join("skills"),
            Agent::Cursor => home.join(".cursor").join("skills"),
            Agent::Pi | Agent::Codex | Agent::Generic => home.join(".agents").join("skills"),
        }
    };
    let dir = base.join("voyager-deep-research");

    fs::create_dir_all(&dir).map_err(VygrError::Io)?;
    fs::write(dir.join("SKILL.md"), SKILL_MD).map_err(VygrError::Io)?;
    println!("installed skill: {}", dir.join("SKILL.md").display());
    eprintln!("vygr: `npx skills add jaltez/voyager` installs it across agents");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The repository-root copy exists for the `npx skills` ecosystem and
    /// must never drift from the packaged canonical file. Skipped when the
    /// repository root is not present (e.g. the published crate).
    #[test]
    fn root_skill_copy_matches_packaged_canonical() {
        let root_copy = PathBuf::from("../../skills/deep-research/SKILL.md");
        if !root_copy.exists() {
            return;
        }
        let on_disk = fs::read_to_string(&root_copy).unwrap();
        assert_eq!(
            on_disk, SKILL_MD,
            "skills/deep-research/SKILL.md drifted from crates/cli/skill/SKILL.md"
        );
    }
}
