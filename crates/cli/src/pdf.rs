//! Markdown -> PDF export for run reports (ADR-0019): a minimal hand-rolled
//! PDF 1.4 writer, zero new dependencies (the typst toolchain ADR-0019
//! deemed disproportionate stays out). The object graph is the smallest
//! conforming one: catalog, pages tree, one page object + content stream
//! per page, and the three base-14 Type1 fonts (Helvetica, Helvetica-Bold,
//! Courier, WinAnsiEncoding) — base-14 fonts need no embedding. Text is
//! drawn with BT/Tf/Td/Tj/ET; byte offsets are tracked while serializing
//! so the xref table is exact. The converter covers the same report subset
//! as the DOCX one (`crate::docx`): headings, bullets and numbered lists
//! (text prefixes), fenced code, block quotes, plain paragraphs; inline
//! markdown is flattened by `crate::markdown`. Characters outside WinAnsi
//! render as `?`.

use vygr_core::VygrError;

use crate::markdown::{heading_level, strip_inline};

/// A4 portrait in PostScript points (1/72 inch).
const PAGE_W: f32 = 595.0;
const PAGE_H: f32 = 842.0;
const MARGIN: f32 = 56.0;
/// Shear for synthetic italics on block quotes (tan 12°): applied via the
/// text matrix so the font set stays at the three base-14 faces.
const ITALIC_SHEAR: f32 = 0.2126;
/// Baseline-to-baseline distance, as a factor of the font size.
const LEADING: f32 = 1.35;
/// Vertical space between blocks (blank lines in the markdown).
const BLOCK_GAP: f32 = 6.0;
const BODY_SIZE: f32 = 11.0;
const QUOTE_SIZE: f32 = 10.5;
const CODE_SIZE: f32 = 9.0;
/// Indentation of block quotes.
const QUOTE_INDENT: f32 = 14.0;

#[derive(Clone, Copy, PartialEq)]
enum Font {
    Helvetica,
    HelveticaBold,
    Courier,
}

impl Font {
    /// Resource name inside each page's /Resources /Font dict.
    fn resource(self) -> &'static str {
        match self {
            Font::Helvetica => "F1",
            Font::HelveticaBold => "F2",
            Font::Courier => "F3",
        }
    }
}

/// Adobe AFM advance widths (1/1000 em units) for ASCII 32..=126 — used
/// for word wrapping only; viewers know the real base-14 metrics. Bytes
/// outside the table measure 600, close enough for wrapping.
const HELVETICA_WIDTHS: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556,
    556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, 1015, 667, 667, 722, 722, 667,
    611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667,
    667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500,
    222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584,
];
const HELVETICA_BOLD_WIDTHS: [u16; 95] = [
    278, 333, 474, 556, 556, 889, 722, 238, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556,
    556, 556, 556, 556, 556, 556, 556, 333, 333, 584, 584, 584, 611, 975, 722, 722, 722, 722, 667,
    611, 778, 722, 278, 556, 722, 611, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667,
    667, 611, 333, 278, 333, 584, 556, 333, 556, 611, 556, 611, 556, 333, 611, 611, 278, 278, 556,
    278, 889, 611, 611, 611, 611, 389, 556, 333, 611, 556, 778, 556, 556, 500, 389, 280, 389, 584,
];

fn char_width(font: Font, byte: u8) -> u16 {
    match font {
        Font::Courier => 600,
        Font::Helvetica | Font::HelveticaBold => {
            let table = if font == Font::Helvetica {
                &HELVETICA_WIDTHS
            } else {
                &HELVETICA_BOLD_WIDTHS
            };
            if (32..=126).contains(&byte) {
                table[(byte - 32) as usize]
            } else {
                600
            }
        }
    }
}

/// WinAnsiEncoding (CP1252) byte for `c`, or None when the character has
/// no mapping (it renders as `?`).
fn win_ansi_byte(c: char) -> Option<u8> {
    match c {
        '\u{20}'..='\u{7E}' | '\u{A0}'..='\u{FF}' => Some(c as u8),
        '\t' => Some(b' '),
        // CP1252's specials occupying the C1 control range.
        '\u{20AC}' => Some(0x80), // €
        '\u{201A}' => Some(0x82), // ‚
        '\u{0192}' => Some(0x83), // ƒ
        '\u{201E}' => Some(0x84), // „
        '\u{2026}' => Some(0x85), // …
        '\u{2020}' => Some(0x86), // †
        '\u{2021}' => Some(0x87), // ‡
        '\u{02C6}' => Some(0x88), // ˆ
        '\u{2030}' => Some(0x89), // ‰
        '\u{0160}' => Some(0x8A), // Š
        '\u{2039}' => Some(0x8B), // ‹
        '\u{0152}' => Some(0x8C), // Œ
        '\u{017D}' => Some(0x8E), // Ž
        '\u{2018}' => Some(0x91), // '
        '\u{2019}' => Some(0x92), // '
        '\u{201C}' => Some(0x93), // "
        '\u{201D}' => Some(0x94), // "
        '\u{2022}' => Some(0x95), // •
        '\u{2013}' => Some(0x96), // –
        '\u{2014}' => Some(0x97), // —
        '\u{02DC}' => Some(0x98), // ˜
        '\u{2122}' => Some(0x99), // ™
        '\u{0161}' => Some(0x9A), // š
        '\u{203A}' => Some(0x9B), // ›
        '\u{0153}' => Some(0x9C), // œ
        '\u{017E}' => Some(0x9E), // ž
        '\u{0178}' => Some(0x9F), // Ÿ
        _ => None,
    }
}

/// Map text to WinAnsi bytes; unmappable characters become `?`.
fn to_win_ansi(text: &str) -> Vec<u8> {
    text.chars()
        .map(|c| win_ansi_byte(c).unwrap_or(b'?'))
        .collect()
}

/// Escape a PDF literal string body: backslash and the `(...)` delimiters.
fn escape_string(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    for &b in bytes {
        match b {
            b'\\' | b'(' | b')' => {
                out.push(b'\\');
                out.push(b);
            }
            _ => out.push(b),
        }
    }
    out
}

/// Greedy word wrap measured in `font`/`size` against `max_width` points.
/// A word that cannot fit a line by itself is hard-broken so nothing can
/// overflow the margin.
fn wrap_line(text: &[u8], font: Font, size: f32, max_width: f32) -> Vec<Vec<u8>> {
    let limit = (max_width / size * 1000.0) as u32; // advance budget in font units
    let width =
        |bytes: &[u8]| -> u32 { bytes.iter().map(|&b| u32::from(char_width(font, b))).sum() };
    let space = u32::from(char_width(font, b' '));
    let mut lines: Vec<Vec<u8>> = Vec::new();
    let mut current: Vec<u8> = Vec::new();
    let mut current_width = 0;
    for mut word in text.split(|&b| b == b' ') {
        loop {
            let needed = if current.is_empty() {
                width(word)
            } else {
                current_width + space + width(word)
            };
            if needed <= limit {
                if !current.is_empty() {
                    current.push(b' ');
                    current_width += space;
                }
                current.extend_from_slice(word);
                current_width += width(word);
                break;
            }
            if !current.is_empty() {
                // Flush and retry the word on a fresh line.
                lines.push(std::mem::take(&mut current));
                current_width = 0;
                continue;
            }
            // The word alone exceeds the line: hard-break it.
            let mut take = 0;
            let mut acc = 0;
            for (i, &b) in word.iter().enumerate() {
                if acc + u32::from(char_width(font, b)) > limit {
                    break;
                }
                acc += u32::from(char_width(font, b));
                take = i + 1;
            }
            if take == 0 {
                take = 1; // degenerate limit: emit single oversized bytes
            }
            lines.push(word[..take].to_vec());
            word = &word[take..];
        }
    }
    lines.push(current);
    lines
}

/// One rendered line: font, size, x position, WinAnsi-encoded text.
struct Line {
    font: Font,
    size: f32,
    x: f32,
    italic: bool,
    text: Vec<u8>,
}

/// Page accumulator: flows placed lines into content streams, breaking
/// pages when the next line would cross the bottom margin. Streams are
/// raw bytes: WinAnsi text is not UTF-8.
struct Pages {
    streams: Vec<Vec<u8>>,
    y: f32,
    pending_gap: bool,
}

impl Pages {
    fn new() -> Self {
        Pages {
            streams: vec![Vec::new()],
            y: PAGE_H - MARGIN,
            pending_gap: false,
        }
    }

    /// Mark a block boundary; the next placed line opens with a gap
    /// (blank lines in the markdown), unless the page is still empty.
    fn separate(&mut self) {
        self.pending_gap = true;
    }

    /// Place one line: `BT /Fn size Tf x y Td (text) Tj ET`. Italic lines
    /// (block quotes) set a skewed text matrix instead of `Td` for the
    /// slant. Each line is its own BT/ET block, so `Td` positions
    /// absolutely from the identity matrix.
    fn place(&mut self, line: Line) {
        if self.pending_gap {
            self.pending_gap = false;
            if self.y < PAGE_H - MARGIN - 0.5 {
                self.y -= BLOCK_GAP;
            }
        }
        if self.y - line.size * LEADING < MARGIN {
            self.streams.push(Vec::new());
            self.y = PAGE_H - MARGIN;
        }
        self.y -= line.size * LEADING;
        let y = self.y;
        let ops = self.streams.last_mut().unwrap();
        ops.extend_from_slice(b"BT /");
        ops.extend_from_slice(line.font.resource().as_bytes());
        ops.extend_from_slice(format!(" {:.2} Tf ", line.size).as_bytes());
        if line.italic {
            ops.extend_from_slice(
                format!("1 0 {:.4} 1 {:.2} {:.2} Tm ", ITALIC_SHEAR, line.x, y).as_bytes(),
            );
        } else {
            ops.extend_from_slice(format!("{:.2} {:.2} Td ", line.x, y).as_bytes());
        }
        ops.push(b'(');
        ops.extend_from_slice(&escape_string(&line.text));
        ops.extend_from_slice(b") Tj ET\n");
    }
}

fn heading_size(level: usize) -> f32 {
    match level {
        1 => 16.0,
        2 => 14.0,
        _ => 12.0,
    }
}

/// Word-wrap `text` to the width left of the line's x position and place
/// every rendered line (continuation lines share the block's x).
fn place_wrapped(pages: &mut Pages, font: Font, size: f32, x: f32, italic: bool, text: &[u8]) {
    let width = PAGE_W - 2.0 * MARGIN - (x - MARGIN);
    for wrapped in wrap_line(text, font, size, width) {
        pages.place(Line {
            font,
            size,
            x,
            italic,
            text: wrapped,
        });
    }
}

/// Convert a synthesized markdown report into PDF bytes (one page minimum,
/// even for empty input).
pub fn markdown_to_pdf(md: &str) -> Vec<u8> {
    let mut pages = Pages::new();
    let mut code: Option<Vec<String>> = None;

    for line in md.lines() {
        let trimmed = line.trim();

        // Fenced code blocks: one 9pt Courier Tj per line.
        if trimmed.starts_with("```") {
            if let Some(lines) = code.take() {
                for cl in lines {
                    place_wrapped(
                        &mut pages,
                        Font::Courier,
                        CODE_SIZE,
                        MARGIN,
                        false,
                        &to_win_ansi(&cl),
                    );
                }
                pages.separate();
            } else {
                code = Some(Vec::new());
            }
            continue;
        }
        if let Some(lines) = &mut code {
            // Raw: code keeps its spacing and characters (WinAnsi-mapped
            // at emit time, like every other line).
            lines.push(line.to_string());
            continue;
        }

        if trimmed.is_empty() {
            pages.separate();
            continue;
        }
        if let Some(level) = heading_level(trimmed) {
            let text = strip_inline(trimmed[(level + 1)..].trim());
            place_wrapped(
                &mut pages,
                Font::HelveticaBold,
                heading_size(level),
                MARGIN,
                false,
                &to_win_ansi(&text),
            );
            pages.separate();
            continue;
        }
        if let Some(rest) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            let text = format!("• {}", strip_inline(rest));
            place_wrapped(
                &mut pages,
                Font::Helvetica,
                BODY_SIZE,
                MARGIN,
                false,
                &to_win_ansi(&text),
            );
            continue;
        }
        if trimmed.starts_with("> ") || trimmed == ">" {
            let rest = strip_inline(trimmed.strip_prefix("> ").unwrap_or(""));
            place_wrapped(
                &mut pages,
                Font::Helvetica,
                QUOTE_SIZE,
                MARGIN + QUOTE_INDENT,
                true,
                &to_win_ansi(&rest),
            );
            continue;
        }
        // Paragraphs (numbered items included): the text prefix keeps the
        // original numbering, exactly like the DOCX converter.
        place_wrapped(
            &mut pages,
            Font::Helvetica,
            BODY_SIZE,
            MARGIN,
            false,
            &to_win_ansi(&strip_inline(trimmed)),
        );
    }
    // Unterminated fence: flush what we have.
    if let Some(lines) = code.take() {
        for cl in lines {
            place_wrapped(
                &mut pages,
                Font::Courier,
                CODE_SIZE,
                MARGIN,
                false,
                &to_win_ansi(&cl),
            );
        }
    }
    serialize(&pages.streams)
}

/// Record an indirect object and its byte offset (exact xref entries).
fn put_object(out: &mut Vec<u8>, offsets: &mut [usize], id: usize, body: &[u8]) {
    offsets[id] = out.len();
    out.extend_from_slice(format!("{id} 0 obj\n").as_bytes());
    out.extend_from_slice(body);
    out.extend_from_slice(b"\nendobj\n");
}

/// Serialize the page content streams into PDF bytes: header, catalog,
/// pages tree, the three font objects (3..5), then one page + content
/// stream object per page, xref table, trailer with /Root, startxref, EOF.
fn serialize(streams: &[Vec<u8>]) -> Vec<u8> {
    const FIRST_PAGE_OBJ: usize = 6; // 1 catalog, 2 pages, 3..5 fonts
    let object_count = FIRST_PAGE_OBJ - 1 + 2 * streams.len();

    let mut out = Vec::new();
    // Header plus the recommended binary-comment line.
    out.extend_from_slice(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n");

    let mut offsets = vec![0usize; object_count + 1];
    let kids: Vec<String> = (0..streams.len())
        .map(|i| format!("{} 0 R", FIRST_PAGE_OBJ + 2 * i))
        .collect();
    put_object(
        &mut out,
        &mut offsets,
        1,
        b"<< /Type /Catalog /Pages 2 0 R >>",
    );
    put_object(
        &mut out,
        &mut offsets,
        2,
        format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>",
            kids.join(" "),
            streams.len()
        )
        .as_bytes(),
    );
    for (id, name) in [(3, "Helvetica"), (4, "Helvetica-Bold"), (5, "Courier")] {
        put_object(
            &mut out,
            &mut offsets,
            id,
            format!(
                "<< /Type /Font /Subtype /Type1 /BaseFont /{name} /Encoding /WinAnsiEncoding >>"
            )
            .as_bytes(),
        );
    }
    for (i, ops) in streams.iter().enumerate() {
        let page_obj = FIRST_PAGE_OBJ + 2 * i;
        let content_obj = page_obj + 1;
        put_object(
            &mut out,
            &mut offsets,
            page_obj,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {PAGE_W} {PAGE_H}] \
                 /Resources << /Font << /F1 3 0 R /F2 4 0 R /F3 5 0 R >> >> \
                 /Contents {content_obj} 0 R >>"
            )
            .as_bytes(),
        );
        // /Length covers everything between `stream\n` and the EOL that
        // precedes `endstream` (the final \n of the ops).
        let length = ops.len().saturating_sub(1);
        let mut content = format!("<< /Length {length} >>\nstream\n").into_bytes();
        content.extend_from_slice(ops);
        content.extend_from_slice(b"endstream");
        put_object(&mut out, &mut offsets, content_obj, &content);
    }

    let xref_pos = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n", object_count + 1).as_bytes());
    out.extend_from_slice(b"0000000000 65535 f \n");
    for offset in &offsets[1..] {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_pos}\n%%EOF",
            object_count + 1
        )
        .as_bytes(),
    );
    out
}

/// Export `answer_md` to a .pdf file at `path`.
pub fn export(answer_md: &str, path: &std::path::Path) -> Result<(), VygrError> {
    std::fs::write(path, markdown_to_pdf(answer_md)).map_err(VygrError::Io)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page_count(bytes: &[u8]) -> usize {
        bytes.windows(12).filter(|w| w == b"/Type /Page ").count()
    }

    #[test]
    fn file_starts_and_ends_with_pdf_markers() {
        let bytes = markdown_to_pdf("# Report\n\nBody text.\n");
        assert!(
            bytes.starts_with(b"%PDF-1.4\n"),
            "opens with the 1.4 header"
        );
        assert!(bytes.ends_with(b"%%EOF"), "closes with the EOF marker");
    }

    #[test]
    fn contains_catalog_pages_and_base14_fonts() {
        let bytes = markdown_to_pdf("# Report\n\nBody.\n");
        let s = String::from_utf8_lossy(&bytes);
        for needle in [
            "/Type /Catalog",
            "/Type /Pages",
            "/Type /Page ",
            "/BaseFont /Helvetica",
            "/BaseFont /Helvetica-Bold",
            "/BaseFont /Courier",
            "/Encoding /WinAnsiEncoding",
            "trailer\n<< /Size",
            "/Root 1 0 R",
        ] {
            assert!(s.contains(needle), "pdf missing `{needle}`");
        }
    }

    /// The correctness-critical check: parse the produced bytes and verify
    /// that every xref entry points exactly at its `N 0 obj` header. The
    /// input paginates, so the walk also covers the page/content object
    /// numbering across several pages.
    #[test]
    fn xref_entries_point_at_their_objects() {
        let filler = "Filler paragraph forces several pages so every page and content \
                      object id is covered by this walk.\n\n"
            .repeat(30);
        let md = format!(
            "# T\n\n## S\n\n{filler}- a\n- b\n\n1. n\n\n> q\n\n```sh\necho hi\n```\n\n\
             Body [1] with (parens), \\ and emoji \u{1F600}.\n"
        );
        let bytes = markdown_to_pdf(&md);
        assert!(page_count(&bytes) > 1, "input must paginate for this walk");
        let startxref = bytes
            .windows(10)
            .rposition(|w| w == b"startxref\n")
            .expect("startxref present");
        let xref_pos: usize = String::from_utf8_lossy(&bytes[startxref + 10..])
            .lines()
            .next()
            .unwrap()
            .parse()
            .expect("numeric startxref offset");
        assert_eq!(&bytes[xref_pos..xref_pos + 5], b"xref\n");

        // Header line after `xref`: `0 N`.
        let eol = bytes[xref_pos + 5..]
            .iter()
            .position(|&b| b == b'\n')
            .unwrap();
        let count_line = String::from_utf8_lossy(&bytes[xref_pos + 5..xref_pos + 5 + eol]);
        let count: usize = count_line
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .expect("object count");
        let mut cursor = xref_pos + 5 + eol + 1;

        for id in 0..count {
            let entry = &bytes[cursor..cursor + 20];
            assert_eq!(entry[19], b'\n', "xref entry {id} is not 20 bytes");
            let text = String::from_utf8_lossy(&entry[..19]).into_owned();
            cursor += 20;
            if id == 0 {
                assert_eq!(text, "0000000000 65535 f ", "entry 0 is the free head");
                continue;
            }
            let offset: usize = text[..10].parse().expect("10-digit offset");
            let header = format!("{id} 0 obj");
            assert_eq!(
                &bytes[offset..offset + header.len()],
                header.as_bytes(),
                "xref entry for object {id} points at `{header}`, not `{}`",
                String::from_utf8_lossy(&bytes[offset..offset + header.len()]),
            );
        }
        assert_eq!(
            &bytes[cursor..cursor + 8],
            b"trailer\n",
            "xref table ends where the trailer begins"
        );
    }

    /// Long input forces multiple pages; the report starts on page 1 and
    /// every page carries exactly one content stream.
    #[test]
    fn long_input_paginates() {
        assert_eq!(page_count(&markdown_to_pdf("# T\n\nBody.\n")), 1);
        let paragraph =
            "Lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor \
             incididunt ut labore et dolore magna aliqua ut enim ad minim veniam quis \
             nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo.\n\n";
        let md = format!("# Long Report\n\n{}", paragraph.repeat(60));
        let bytes = markdown_to_pdf(&md);
        let pages = page_count(&bytes);
        assert!(pages > 1, "long input must paginate (got {pages})");
        assert_eq!(
            bytes.windows(10).filter(|w| w == b"/Contents ").count(),
            pages,
            "one content stream per page"
        );
        let first_stream = String::from_utf8_lossy(&bytes[..2048.min(bytes.len())]);
        assert!(
            first_stream.contains("(Long Report) Tj"),
            "title lands on the first page"
        );
    }

    #[test]
    fn empty_input_renders_one_page() {
        let bytes = markdown_to_pdf("");
        assert_eq!(page_count(&bytes), 1);
        assert!(bytes.ends_with(b"%%EOF"));
    }

    /// Escaping is required for `\`, `(`, `)`; anything outside WinAnsi
    /// (here: an emoji, and CJK) collapses to `?`, one per character.
    #[test]
    fn strings_are_escaped_and_non_winansi_becomes_question() {
        let bytes = markdown_to_pdf("a (b) \\c) \u{1F600} 日本 d\n");
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.contains("(a \\(b\\) \\\\c\\) ? ?? d) Tj"), "stream: {s}");
    }

    #[test]
    fn bullets_quotes_headings_and_code_render() {
        let md = "# H1\n\n## H2\n\n- one\n- two\n\n1. first\n\n> quoted\n\n```sh\necho hi\n```\n";
        let bytes = markdown_to_pdf(md);
        let s = String::from_utf8_lossy(&bytes);
        // The bullet prefix survives as the WinAnsi bullet byte (0x95).
        assert!(bytes.windows(5).any(|w| w == b"\x95 one"), "bullet: {s}");
        assert!(s.contains("/F2 16.00 Tf"), "h1 is 16pt bold: {s}");
        assert!(s.contains("/F2 14.00 Tf"), "h2 is 14pt bold: {s}");
        assert!(s.contains("/F3 9.00 Tf"), "code is 9pt Courier: {s}");
        assert!(s.contains("(echo hi) Tj"), "one Tj per code line: {s}");
        assert!(
            s.contains("(1. first) Tj"),
            "numbered items keep their prefix: {s}"
        );
        // Quotes: skewed text matrix (synthetic italic) + indent.
        assert!(
            s.contains(&format!(
                "BT /F1 10.50 Tf 1 0 0.2126 1 {:.2} ",
                MARGIN + QUOTE_INDENT
            )),
            "quote is italic 10.5pt at an indent: {s}"
        );
        assert!(s.contains("(quoted) Tj"), "quote text: {s}");
    }

    /// Export end-to-end into a tempdir.
    #[test]
    fn export_writes_pdf_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("report.pdf");
        export(
            "# Report\n\nBody **bold** text with [1] citations.\n",
            &path,
        )
        .unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.starts_with(b"%PDF-1.4"));
        assert!(bytes.ends_with(b"%%EOF"));
    }

    /// Extract every literal string operand from `(text) Tj` operators
    /// (unescape-aware; the only parens in these PDFs are string literals).
    fn tj_payloads(bytes: &[u8]) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'(' => {
                    let mut j = i + 1;
                    let mut text = Vec::new();
                    while j < bytes.len() {
                        match bytes[j] {
                            b'\\' => {
                                text.push(bytes[j + 1]);
                                j += 2;
                            }
                            b')' => break,
                            b => {
                                text.push(b);
                                j += 1;
                            }
                        }
                    }
                    out.push(text);
                    i = j + 1;
                }
                _ => i += 1,
            }
        }
        out
    }

    /// Long body paragraphs word-wrap into several 11pt lines, every line
    /// fits the content width (nothing overflows the margin), and no text
    /// is dropped.
    #[test]
    fn body_text_word_wraps_to_the_margins() {
        let words = "alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo \
                     lima mike november oscar papa quebec romeo sierra tango uniform \
                     victor whiskey xray yankee zulu ";
        let bytes = markdown_to_pdf(&words.repeat(6));
        let payloads = tj_payloads(&bytes);
        assert!(
            payloads.len() > 1,
            "a long paragraph wraps into several lines"
        );
        let content_w = (PAGE_W - 2.0 * MARGIN) / BODY_SIZE * 1000.0; // font units
        for p in &payloads {
            let w: u32 = p
                .iter()
                .map(|&b| u32::from(char_width(Font::Helvetica, b)))
                .sum();
            assert!(
                w as f32 <= content_w * 1.001,
                "line overflows the margin: {:?}",
                String::from_utf8_lossy(p)
            );
        }
        let joined: String = payloads
            .iter()
            .map(|p| String::from_utf8_lossy(p).to_string())
            .collect();
        assert!(joined.contains("zulu"), "no text truncated");
    }
}
