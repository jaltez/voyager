//! Inline markdown flattening shared by the report exporters (`docx`,
//! `pdf`): heading detection plus the strip logic that turns inline
//! markup into plain text so no markup leaks into exported documents.

/// Number of leading `#`s (1..=6) when the line is a heading.
pub fn heading_level(line: &str) -> Option<usize> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes) && line[hashes..].starts_with(' ') {
        Some(hashes)
    } else {
        None
    }
}

/// Flatten inline markdown: links to `text (url)`, emphasis and code
/// markers stripped, citation markers `[n]` preserved.
pub fn strip_inline(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        out.push_str(&flatten_emphasis(&rest[..open]));
        let after = &rest[open + 1..];
        match after.find(']') {
            Some(close) => {
                let label = &after[..close];
                // Link or citation?
                if let Some(url) = after[close + 1..].strip_prefix('(') {
                    if let Some(end) = url.find(')') {
                        out.push_str(&format!("{} ({})", flatten_emphasis(label), &url[..end]));
                        rest = &url[end + 1..];
                        continue;
                    }
                }
                if label.parse::<usize>().is_ok() || label.chars().count() <= 4 {
                    out.push_str(&format!("[{label}]")); // citation marker: keep
                } else {
                    out.push_str(&flatten_emphasis(label));
                }
                rest = &after[close + 1..];
            }
            None => {
                out.push_str(&flatten_emphasis(after));
                rest = "";
            }
        }
    }
    out.push_str(&flatten_emphasis(rest));
    out
}

fn flatten_emphasis(text: &str) -> String {
    text.replace("**", "")
        .replace(['*', '`'], "")
        .replace("__", "")
        .replace('_', "")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_flattening_keeps_citations_and_links() {
        assert_eq!(strip_inline("a **b** `c` [1] end"), "a b c [1] end");
        assert_eq!(
            strip_inline("see [the docs](https://x.io/a) now"),
            "see the docs (https://x.io/a) now"
        );
        assert_eq!(heading_level("## Two"), Some(2));
        assert_eq!(heading_level("#NoSpace"), None);
        assert_eq!(heading_level("plain"), None);
    }
}
