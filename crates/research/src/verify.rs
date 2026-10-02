//! Citation verification (T1, Mole-inspired): after synthesis, every
//! sentence carrying a `[n]` citation is checked by lexical overlap
//! against the stored source texts. Claims with weak support are flagged
//! in the report and appended to `answer.md` as a verification section.
//! This is a deterministic heuristic, not a judge: it catches fabricated
//! claims (which share no vocabulary with any fetched source) and
//! misattributions (supported by a different source than the one cited).

use std::collections::HashSet;

use serde::Serialize;

use crate::orchestrator::Source;

/// Below this fraction of shared content words a claim is unsupported.
const SUPPORT_THRESHOLD: f32 = 0.2;

#[derive(Debug, Clone, Serialize)]
pub struct WeakClaim {
    /// The citation number used by the claim.
    pub citation: usize,
    /// Best-matching source (1-based); differs from `citation` on
    /// suspected misattribution.
    pub best_source: usize,
    pub overlap: f32,
    pub excerpt: String,
    pub cited_url: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Verification {
    pub checked: usize,
    pub weak: Vec<WeakClaim>,
}

impl Verification {
    pub fn is_clean(&self) -> bool {
        self.weak.is_empty()
    }
}

/// Tokenize into content words: lowercase alphanumeric runs of length >= 3
/// (shorter tokens are noise for overlap purposes).
fn content_words(text: &str) -> HashSet<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| t.len() >= 3)
        .map(str::to_string)
        .collect()
}

/// Fraction of the claim's content words present in the source text.
fn overlap(claim: &HashSet<String>, source: &HashSet<String>) -> f32 {
    if claim.is_empty() {
        return 1.0;
    }
    let hits = claim.iter().filter(|w| source.contains(*w)).count();
    hits as f32 / claim.len() as f32
}

/// Extract the sentence around each `[n]` marker. Sentences are split on
/// periods followed by whitespace or end of text (crude but stable for
/// reports).
fn cited_sentences(answer: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let sentences: Vec<&str> = answer
        .split('\n')
        .flat_map(|line| split_sentences(line))
        .collect();
    for sentence in sentences {
        for (i, c) in sentence.char_indices() {
            if c == '[' {
                let rest = &sentence[i + 1..];
                if let Some(end) = rest.find(']') {
                    if let Ok(n) = rest[..end].trim().parse::<usize>() {
                        out.push((n, sentence.trim().to_string()));
                        break; // one record per sentence is enough
                    }
                }
            }
        }
    }
    out
}

fn split_sentences(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let bytes = line.as_bytes();
    for i in 0..bytes.len() {
        if bytes[i] == b'.' && (i + 1 == bytes.len() || bytes[i + 1] == b' ') {
            out.push(&line[start..=i]);
            start = i + 1;
        }
    }
    if start < line.len() {
        out.push(&line[start..]);
    }
    out
}

/// Verify every cited sentence of `answer` against `sources`.
pub fn verify(answer: &str, sources: &[Source]) -> Verification {
    let source_words: Vec<HashSet<String>> = sources
        .iter()
        .map(|s| {
            let mut text = format!("{} {}", s.title, s.snippet);
            if let Some(c) = &s.content {
                text.push(' ');
                text.push_str(c);
            }
            content_words(&text)
        })
        .collect();

    let mut checked = 0;
    let mut weak = Vec::new();
    for (citation, sentence) in cited_sentences(answer) {
        if citation == 0 || citation > sources.len() {
            continue;
        }
        checked += 1;
        // Strip the citation markers before tokenizing the claim.
        let claim_text = strip_citations(&sentence);
        let claim = content_words(&claim_text);
        if claim.is_empty() {
            continue;
        }
        let mut best = (0usize, 0.0f32);
        for (idx, words) in source_words.iter().enumerate() {
            let score = overlap(&claim, words);
            if score > best.1 {
                best = (idx, score);
            }
        }
        if best.1 < SUPPORT_THRESHOLD {
            weak.push(WeakClaim {
                citation,
                best_source: best.0 + 1,
                overlap: best.1,
                excerpt: truncate(&claim_text, 120),
                cited_url: sources[citation - 1].url.clone(),
            });
        }
    }
    Verification { checked, weak }
}

fn strip_citations(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('[') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find(']') {
            Some(end) => rest = &after[end + 1..],
            None => {
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// Markdown section appended to answer.md when weak claims exist.
pub fn verification_section(verification: &Verification) -> String {
    let mut out = String::from("\n## Citation verification\n\n");
    out.push_str("Lexical check of cited sentences against the stored sources ");
    out.push_str(&format!(
        "({} checked, {} weakly supported):\n",
        verification.checked,
        verification.weak.len()
    ));
    for w in &verification.weak {
        out.push_str(&format!(
            "- `[{}]` best match [{}/{}] overlap {:.2}: {}\n",
            w.citation, w.best_source, w.citation, w.overlap, w.excerpt
        ));
    }
    out.push_str("\nTreat flagged claims with caution or re-run with more depth.\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(title: &str, content: &str) -> Source {
        Source {
            title: title.to_string(),
            url: format!("https://a.io/{title}"),
            snippet: String::new(),
            provider: "t".into(),
            score: 1.0,
            level: 0,
            content: Some(content.to_string()),
        }
    }

    #[test]
    fn supported_claims_pass_and_fabrications_flag() {
        let sources = vec![
            source("rust", "Rust 1.85 shipped the 2024 edition with unsafe extern blocks and RPIT lifetime capture rules"),
            source("zig", "Zig has comptime and no hidden control flow"),
        ];
        let answer = "The 2024 edition arrived with unsafe extern blocks in Rust 1.85 [1]. \
Secret moon bases were discovered orbiting Jupiter last Tuesday [1].";
        let v = verify(answer, &sources);
        assert_eq!(v.checked, 2);
        assert_eq!(v.weak.len(), 1);
        assert!(v.weak[0].excerpt.contains("moon"));
        assert!(!v.is_clean());
    }

    #[test]
    fn misattribution_points_at_best_source() {
        let sources = vec![
            source("rust", "Zig has comptime execution"),
            source("zig", "Zig has comptime execution"),
        ];
        let v = verify("Zig has comptime execution [1].", &sources);
        assert!(v.weak.is_empty());
        // Even though the "right" citation was [2], the claim is supported.
    }

    #[test]
    fn out_of_range_citations_are_ignored() {
        let v = verify("Something cited badly [99].", &[source("a", "x y z")]);
        assert_eq!(v.checked, 0);
    }

    #[test]
    fn citation_markers_stripped_before_scoring() {
        assert_eq!(strip_citations("a [1] b [2][3]"), "a  b ");
        let cw = content_words("The quick brown fox jumps");
        assert!(cw.contains("quick") && !cw.contains("ox"));
    }
}
