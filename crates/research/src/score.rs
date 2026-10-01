//! Context relevance scoring (ADR-0006): term-overlap placeholder today,
//! BM25 planned. No embeddings, no vector store — short-lived research
//! runs do not justify the machinery.

use std::collections::HashSet;

/// Tokenize for scoring: lowercase ASCII alphanumeric runs of length >= 2.
pub fn terms(text: &str) -> HashSet<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| t.len() >= 2)
        .map(str::to_string)
        .collect()
}

/// Relevance of `doc` to the query term set: Jaccard-flavored overlap that
/// slightly rewards shorter documents.
pub fn relevance(query_terms: &HashSet<String>, doc: &str) -> f32 {
    if query_terms.is_empty() {
        return 0.0;
    }
    let doc_terms = terms(doc);
    let hits = query_terms
        .iter()
        .filter(|t| doc_terms.contains(*t))
        .count();
    hits as f32 / (1.0 + (doc_terms.len() as f32).sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relevant_doc_outscores_irrelevant_one() {
        let q = terms("rust webassembly benchmark");
        let relevant = relevance(&q, "rust webassembly benchmark results 2026");
        let irrelevant = relevance(&q, "cooking pasta dinner recipes italian");
        assert!(relevant > irrelevant);
        assert!(relevant > 0.0);
    }
}
