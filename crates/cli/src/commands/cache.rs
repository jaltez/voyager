//! `vygr cache` — inspect and clear the on-disk search cache (M1.2).

use clap::Args as ClapArgs;
use vygr_core::VygrError;

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum Action {
    /// Print the cache directory path
    Dir,
    /// Delete every cached search entry
    Clear,
}

#[derive(Debug, ClapArgs)]
pub struct Args {
    #[arg(value_enum, default_value = "dir")]
    pub action: Action,
}

pub fn run(args: Args) -> Result<(), VygrError> {
    let Some(root) = vygr_core::config::Config::cache_dir() else {
        eprintln!("vygr: no cache directory on this platform");
        return Ok(());
    };
    match args.action {
        Action::Dir => println!("{}", root.join("search").display()),
        Action::Clear => {
            let search_dir = root.join("search");
            if search_dir.exists() {
                let entries = count_files(&search_dir);
                std::fs::remove_dir_all(&search_dir).map_err(VygrError::Io)?;
                println!(
                    "cleared {entries} cache entries from {}",
                    search_dir.display()
                );
            } else {
                println!("cache is empty ({})", search_dir.display());
            }
        }
    }
    Ok(())
}

fn count_files(dir: &std::path::Path) -> usize {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .filter_map(|e| std::fs::read_dir(e.path()).ok())
                .map(|rd| rd.filter_map(|e| e.ok()).count())
                .sum()
        })
        .unwrap_or(0)
}
