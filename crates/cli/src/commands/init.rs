//! `vygr init` — install the voyager skill into an agent harness
//! (tvly-style guided setup; `npx skills add jaltez/voyager` for agents).

use std::fs;
use std::path::PathBuf;

use clap::Args as ClapArgs;
use vygr_core::VygrError;

/// Canonical skill file, packaged with the crate. The copy at
/// `skills/vygr/SKILL.md` in the repository exists for the
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
    let dir = base.join("vygr");

    fs::create_dir_all(&dir).map_err(VygrError::Io)?;
    fs::write(dir.join("SKILL.md"), SKILL_MD).map_err(VygrError::Io)?;
    println!("installed skill: {}", dir.join("SKILL.md").display());
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
    }
}
