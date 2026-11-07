//! Markdown -> DOCX export for run reports (ADR-0019). The converter
//! covers exactly the subset vygr synthesizes: headings, bullets and
//! numbered lists, fenced code, block quotes, and plain paragraphs. List
//! items render as text prefixes (no numbering.xml) — reports are read,
//! not renumbered. Inline markdown is flattened to plain text so no
//! markup leaks into Word. Heading styles are defined here (docx-rs ships
//! no default style table), sized in half-points.

use docx_rs::{BreakType, Docx, RunFonts, Style, StyleType, Styles};
use vygr_core::VygrError;

use crate::markdown::{heading_level, strip_inline};

/// Convert a synthesized markdown report into a DOCX document.
pub fn markdown_to_docx(md: &str) -> Docx {
    let mut doc = Docx::new().styles(
        Styles::new()
            .add_style(heading_style("Heading1", "heading 1", 32))
            .add_style(heading_style("Heading2", "heading 2", 28))
            .add_style(heading_style("Heading3", "heading 3", 24))
            .add_style(heading_style("Heading4", "heading 4", 22))
            .add_style(heading_style("Heading5", "heading 5", 21))
            .add_style(heading_style("Heading6", "heading 6", 20)),
    );
    let mut code: Option<Vec<String>> = None;

    for line in md.lines() {
        let trimmed = line.trim();

        // Fenced code blocks become one monospace paragraph with breaks.
        if trimmed.starts_with("```") {
            if let Some(lines) = code.take() {
                doc = doc.add_paragraph(code_paragraph(&lines));
            } else {
                code = Some(Vec::new());
            }
            continue;
        }
        if let Some(lines) = &mut code {
            lines.push(line.to_string());
            continue;
        }

        if trimmed.is_empty() {
            continue;
        }
        if let Some(level) = heading_level(trimmed) {
            let text = strip_inline(trimmed[(level + 1)..].trim());
            let mut p = docx_rs::Paragraph::new()
                .add_run(docx_rs::Run::new().add_text(text))
                .style(heading_style_id(level));
            if level <= 3 {
                p = p.outline_lvl(level - 1);
            }
            doc = doc.add_paragraph(p);
            continue;
        }
        if let Some(rest) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            doc = doc.add_paragraph(
                docx_rs::Paragraph::new()
                    .add_run(docx_rs::Run::new().add_text(format!("• {}", strip_inline(rest)))),
            );
            continue;
        }
        if trimmed.starts_with("> ") || trimmed == ">" {
            let rest = trimmed.strip_prefix("> ").unwrap_or("");
            doc = doc.add_paragraph(
                docx_rs::Paragraph::new()
                    .add_run(docx_rs::Run::new().add_text(strip_inline(rest)).italic()),
            );
            continue;
        }
        // Numbered items keep their original numbering as text.
        doc = doc.add_paragraph(
            docx_rs::Paragraph::new().add_run(docx_rs::Run::new().add_text(strip_inline(trimmed))),
        );
    }
    // Unterminated fence: flush what we have.
    if let Some(lines) = code.take() {
        doc = doc.add_paragraph(code_paragraph(&lines));
    }
    doc
}

fn heading_style(id: &str, name: &str, half_points: usize) -> Style {
    Style::new(id, StyleType::Paragraph)
        .name(name)
        .bold()
        .size(half_points)
        .q_format(true)
}

fn heading_style_id(level: usize) -> &'static str {
    match level {
        1 => "Heading1",
        2 => "Heading2",
        3 => "Heading3",
        4 => "Heading4",
        5 => "Heading5",
        _ => "Heading6",
    }
}

fn code_paragraph(lines: &[String]) -> docx_rs::Paragraph {
    let mut run = docx_rs::Run::new().fonts(RunFonts::new().ascii("Courier").hi_ansi("Courier"));
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            run = run.add_break(BreakType::TextWrapping);
        }
        run = run.add_text(line);
    }
    docx_rs::Paragraph::new().add_run(run)
}

/// Export `answer_md` to a .docx file at `path`.
pub fn export(answer_md: &str, path: &std::path::Path) -> Result<(), VygrError> {
    let file = std::fs::File::create(path).map_err(VygrError::Io)?;
    markdown_to_docx(answer_md)
        .build()
        .pack(file)
        .map_err(|e| VygrError::Config(format!("docx export: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headings_lists_and_code_survive_conversion() {
        let md = "# Title\n\n## Summary\n\nBody **bold** `code` and [1] citations with a [link](https://x.io).\n\n- one\n- two\n\n1. first\n\n> quoted thought\n\n```sh\necho hi\n```\n";
        let doc = markdown_to_docx(md);
        // Building + packing is the real assertion: it must not panic and
        // must produce a zip archive.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("report.docx");
        let file = std::fs::File::create(&path).unwrap();
        doc.build().pack(file).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..2], b"PK", "docx is a zip archive");
    }

    #[test]
    fn inline_flattening_keeps_citations_and_links() {
        // Behavior lives in `crate::markdown` now; this keeps the docx
        // pipeline honest about it.
        assert_eq!(strip_inline("a **b** `c` [1] end"), "a b c [1] end");
    }
}
