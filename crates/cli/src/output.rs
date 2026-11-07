//! Output rendering: table / markdown / json / urls (ADR-0008).

use vygr_core::types::{truncate_chars, SearchResult};

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum SearchFormat {
    Table,
    Json,
    /// One JSON object per line: pipe-friendly and streamable (`| jq`,
    /// `| xargs`). Result lines only; cache meta stays in `--format json`.
    Jsonl,
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
    meta: &serde_json::Value,
) -> String {
    match format {
        SearchFormat::Table => search_table(results),
        SearchFormat::Json => serde_json::to_string_pretty(&serde_json::json!({
            "query": query,
            "provider": provider,
            "meta": meta,
            "results": results,
        }))
        .unwrap_or_default(),
        SearchFormat::Jsonl => results
            .iter()
            .map(|r| serde_json::to_string(r).unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\n"),
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

/// The body of the report's first `## Summary` section (used by
/// `research --brief-only`); `None` when the report has none.
pub fn extract_summary(answer: &str) -> Option<String> {
    let mut lines = answer.lines().peekable();
    for line in lines.by_ref() {
        if line.trim().eq_ignore_ascii_case("## Summary") {
            break;
        }
    }
    let mut body: Vec<&str> = Vec::new();
    for line in lines {
        let trimmed = line.trim();
        if trimmed.starts_with("## ") {
            break;
        }
        body.push(line);
    }
    let text = body.join("\n").trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_summary_pulls_the_first_summary_section() {
        let answer = "## Summary\n\nShort answer with [1] citations.\n\n## Details\n\nBody.\n";
        assert_eq!(
            extract_summary(answer).as_deref(),
            Some("Short answer with [1] citations.")
        );
        // Summary to EOF.
        assert_eq!(
            extract_summary("## Summary\n\nJust this.\n").as_deref(),
            Some("Just this.")
        );
        // No summary section.
        assert!(extract_summary("## Findings\n\nbody").is_none());
    }

    fn hit(url: &str) -> SearchResult {
        SearchResult {
            title: "t".into(),
            url: url.into(),
            snippet: "s".into(),
            provider: "ddgs".into(),
            providers: vec!["ddgs".into()],
            content: None,
        }
    }

    #[test]
    fn jsonl_emits_one_parseable_object_per_result() {
        let results = vec![hit("https://a.example/1"), hit("https://b.example/2")];
        let out = render_search(
            SearchFormat::Jsonl,
            "q",
            "ddgs",
            &results,
            &serde_json::json!({}),
        );
        let lines: Vec<serde_json::Value> = out
            .lines()
            .map(|l| serde_json::from_str(l).expect("each jsonl line must parse"))
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["url"], "https://a.example/1");
        assert_eq!(lines[1]["url"], "https://b.example/2");
    }
}
