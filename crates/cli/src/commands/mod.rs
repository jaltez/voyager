//! Subcommand implementations.

pub mod cache;
pub mod config;
pub mod extract;
pub mod init;
pub mod models;
pub mod plan;
pub mod providers;
pub mod research;
pub mod schema;
pub mod search;

use std::io::Read;

use vygr_core::VygrError;

/// Split a comma-separated domain list into normalized entries.
pub fn parse_domain_list(spec: Option<&str>) -> Vec<String> {
    vygr_core::provider::normalize_domains(
        spec.unwrap_or_default()
            .split(',')
            .map(str::to_string)
            .collect(),
    )
}

/// Resolve a query argument: `"-"` reads the query from stdin.
pub fn read_query(raw: &str) -> Result<String, VygrError> {
    let raw = raw.trim();
    if raw != "-" {
        return Ok(raw.to_string());
    }
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .map_err(VygrError::Io)?;
    let query = buf.trim().to_string();
    if query.is_empty() {
        return Err(VygrError::Config("empty query on stdin".to_string()));
    }
    Ok(query)
}
