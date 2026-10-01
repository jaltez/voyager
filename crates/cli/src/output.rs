//! Output rendering: table / markdown / json / urls (ADR-0008).

use vygr_core::types::{truncate_chars, SearchResult};

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum SearchFormat {
    Table,
    Json,
    Md,
    Urls,
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum ExtractFormat {
    Text,
    Json,
    Md,
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum ReportFormat {
    Md,
    Json,
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum GenericFormat {
    Table,
    Json,
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum ConfigFormat {
    Toml,
    Json,
}

pub fn render_search(
    format: SearchFormat,
    query: &str,
    provider: &str,
    results: &[SearchResult],
) -> String {
    match format {
        SearchFormat::Table => search_table(results),
        SearchFormat::Json => serde_json::to_string_pretty(&serde_json::json!({
            "query": query,
            "provider": provider,
            "results": results,
        }))
        .unwrap_or_default(),
        SearchFormat::Md => results
            .iter()
            .enumerate()
            .map(|(i, r)| {
                format!(
                    "### {}. {}\n[{}]({})\n{}\n",
                    i + 1,
                    r.title,
                    r.url,
                    r.url,
                    r.snippet
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
        SearchFormat::Urls => results
            .iter()
            .map(|r| r.url.clone())
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn search_table(results: &[SearchResult]) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{:<3}  {:<34}  {:<44}  {}\n",
        "#", "TITLE", "URL", "SNIPPET"
    ));
    out.push_str(&"-".repeat(110));
    out.push('\n');
    for (i, r) in results.iter().enumerate() {
        let mut snippet = truncate_chars(&r.snippet.replace('\n', " "), 56);
        if let Some(content) = &r.content {
            let head: String = content.trim().chars().take(40).collect();
            if !head.is_empty() {
                snippet = format!("{}…", head);
            }
        }
        out.push_str(&format!(
            "{:<3}  {:<34}  {:<44}  {}\n",
            i + 1,
            truncate_chars(&r.title.replace('\n', " "), 32),
            truncate_chars(&r.url, 42),
            snippet
        ));
    }
    out
}

pub fn render_pages(format: ExtractFormat, pages: &[vygr_core::types::FetchPage]) -> String {
    match format {
        ExtractFormat::Json => serde_json::to_string_pretty(pages).unwrap_or_default(),
        ExtractFormat::Text | ExtractFormat::Md => pages
            .iter()
            .map(|p| {
                let title = p.title.clone().unwrap_or_else(|| p.url.clone());
                format!("# {title}\nURL: {}\n\n{}\n", p.final_url, p.text.trim())
            })
            .collect::<Vec<_>>()
            .join("\n---\n\n"),
    }
}
