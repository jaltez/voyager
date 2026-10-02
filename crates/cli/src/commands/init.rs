//! `vygr init` — install the voyager skill into an agent harness
//! (tvly-style guided setup; `npx skills add` once the repo is public).

use std::fs;
use std::path::PathBuf;

use clap::Args as ClapArgs;
use vygr_core::VygrError;

const SKILL_MD: &str = include_str!("../../../../skills/deep-research/SKILL.md");

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
    eprintln!(
        "vygr: once this repo is public, `npx skills add jaltez/voyager` installs it across agents"
    );
    Ok(())
}
