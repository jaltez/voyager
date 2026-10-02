//! Context relevance scoring (ADR-0006, M2.1): Okapi BM25 over the run's
//! own corpus. No embeddings, no vector store; short-lived research runs
//! do not justify the machinery.

use std::collections::{HashMap, HashSet};

/// Tokenize for scoring: lowercase ASCII alphanumeric runs of length >= 2.
pub fn terms(text: &str) -> HashSet<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| t.len() >= 2)
        .map(str::to_string)
        .collect()
}

/// Okapi BM25 index over a fixed corpus (one entry per source document).
pub struct Bm25 {
    k1: f32,
    b: f32,
    /// Per-document term frequencies.
    docs: Vec<HashMap<String, u32>>,
    /// Number of documents containing each term.
    doc_freq: HashMap<String, usize>,
    doc_lens: Vec<f32>,
    avgdl: f32,
}

impl Bm25 {
    pub fn build<'a, I: IntoIterator<Item = &'a str>>(docs: I) -> Self {
        let mut index = Self {
            k1: 1.5,
            b: 0.75,
            docs: Vec::new(),
            doc_freq: HashMap::new(),
            doc_lens: Vec::new(),
            avgdl: 0.0,
        };
        let mut total_len = 0u64;
        for doc in docs {
            let mut tf: HashMap<String, u32> = HashMap::new();
            for term in terms(doc) {
                *tf.entry(term).or_insert(0) += 1;
            }
            total_len += tf.len() as u64;
            for term in tf.keys() {
                *index.doc_freq.entry(term.clone()).or_insert(0) += 1;
            }
            index.doc_lens.push(tf.len() as f32);
            index.docs.push(tf);
        }
        if !index.docs.is_empty() {
            index.avgdl = total_len as f32 / index.docs.len() as f32;
        }
        index
    }

    /// BM25 score of document `idx` against the query term set. Duplicate
    /// query terms count once (we score topics, not bag-of-words).
    pub fn score(&self, query_terms: &HashSet<String>, idx: usize) -> f32 {
        let Some(tf) = self.docs.get(idx) else {
            return 0.0;
        };
        let n = self.docs.len() as f32;
        let dl = self.doc_lens[idx];
        let mut total = 0.0;
        for term in query_terms {
            let f = match tf.get(term) {
                Some(&f) => f as f32,
                None => continue,
            };
            let df = self.doc_freq.get(term).copied().unwrap_or(0) as f32;
            let idf = (1.0 + (n - df + 0.5) / (df + 0.5)).ln();
            let norm = self.k1 * (1.0 - self.b + self.b * dl / self.avgdl.max(1.0));
            total += idf * (f * (self.k1 + 1.0)) / (f + norm);
        }
        total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relevant_doc_outscores_irrelevant_one() {
        let corpus = [
            "rust webassembly benchmark results 2026 runtime performance",
            "cooking pasta dinner recipes italian cuisine tomato sauce",
            "rust compiler error messages and borrow checking explained",
        ];
        let index = Bm25::build(corpus.iter().copied());
        let q = terms("rust webassembly benchmark");
        assert!(index.score(&q, 0) > index.score(&q, 1));
        // A same-topic doc also ranks, and the top match is the best one.
        assert!(index.score(&q, 2) > index.score(&q, 1));
        assert!(index.score(&q, 0) > index.score(&q, 2));
    }

    #[test]
    fn empty_query_and_empty_corpus_are_safe() {
        let index = Bm25::build(["some doc"].iter().copied());
        assert_eq!(index.score(&HashSet::new(), 0), 0.0);
        assert_eq!(index.score(&terms("anything"), 7), 0.0);
        let empty = Bm25::build(std::iter::empty::<&str>());
        assert_eq!(empty.score(&terms("x"), 0), 0.0);
    }

    #[test]
    fn term_frequency_and_length_normalization_matter() {
        let corpus = [
            "wasm wasm wasm wasm gc support",
            "wasm gc support and shipping in browsers",
        ];
        let index = Bm25::build(corpus.iter().copied());
        let q = terms("wasm");
        // Higher term frequency wins despite similar length.
        assert!(index.score(&q, 0) > index.score(&q, 1));
    }
}
