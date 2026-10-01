//! Run artifact directories (ADR-0009): every research run leaves an
//! evidence trail on disk under `./agents/voyager/<timestamp>-<slug>/` —
//! `prompt.md`, `plan.json`, `sources.json` and `answer.md`.

use std::fs;
use std::io;
use std::path::PathBuf;

use chrono::Utc;

pub fn create_run_dir(base: Option<&str>, query: &str) -> io::Result<PathBuf> {
    let base = base
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("agents").join("voyager"));
    let dir = base.join(format!(
        "{}-{}",
        Utc::now().format("%Y%m%d-%H%M%S"),
        slugify(query)
    ));
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn write_file(dir: &std::path::Path, name: &str, contents: &str) -> io::Result<()> {
    fs::write(dir.join(name), contents)
}

fn slugify(query: &str) -> String {
    let slug: String = query
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let compact: Vec<&str> = slug.split('-').filter(|s| !s.is_empty()).collect();
    compact.join("-").chars().take(40).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_is_compact_and_bounded() {
        assert_eq!(
            slugify("Rust vs. Zig -- WebAssembly!! (2026)"),
            "rust-vs-zig-webassembly-2026"
        );
        let long = "x".repeat(200);
        assert!(slugify(&long).len() <= 40);
    }
}
