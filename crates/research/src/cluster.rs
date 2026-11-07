//! Syndication clustering (ADR-0017): group sources that are clearly the
//! same story (wire copies, syndicated posts, press-release echoes) so
//! synthesis treats them as one evidence chain instead of independent
//! confirmation. Title-token Jaccard similarity + union-find; conservative
//! threshold so only near-identical titles merge.

use std::collections::HashSet;

use crate::orchestrator::Source;

/// Titles sharing more than this Jaccard fraction of content tokens are
/// the same story. Deliberately high: a false merge hides a real source.
const MERGE_THRESHOLD: f32 = 0.7;

fn title_tokens(title: &str) -> HashSet<String> {
    title
        .to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| t.len() >= 3)
        .map(str::to_string)
        .collect()
}

fn jaccard(a: &HashSet<String>, b: &HashSet<String>) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let inter = a.intersection(b).count();
    let union = a.union(b).count();
    inter as f32 / union as f32
}

/// Assign `cluster_id`/`origin_count` to every source that belongs to a
/// multi-member cluster (singletons stay `None`). Cluster ids are
/// 1-based in descending cluster size, so `[1]`-numbered citations keep
/// pointing at the highest-ranked source.
pub fn cluster_sources(sources: &mut [Source]) {
    let tokens: Vec<HashSet<String>> = sources.iter().map(|s| title_tokens(&s.title)).collect();
    let n = sources.len();
    let mut parent: Vec<usize> = (0..n).collect();

    fn find(parent: &mut Vec<usize>, i: usize) -> usize {
        if parent[i] != i {
            let root = find(parent, parent[i]);
            parent[i] = root;
        }
        parent[i]
    }

    for i in 0..n {
        for j in (i + 1)..n {
            if jaccard(&tokens[i], &tokens[j]) > MERGE_THRESHOLD {
                let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
                if ri != rj {
                    parent[ri] = rj;
                }
            }
        }
    }

    let mut members: std::collections::HashMap<usize, Vec<usize>> =
        std::collections::HashMap::new();
    for i in 0..n {
        members.entry(find(&mut parent, i)).or_default().push(i);
    }

    // Renumber clusters by size (largest first) for stable, readable ids.
    let mut clusters: Vec<(usize, Vec<usize>)> = members.into_iter().collect();
    clusters.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(&b.0)));
    let mut next_id = 1u32;
    for (_, idxs) in clusters {
        if idxs.len() < 2 {
            continue;
        }
        let id = next_id;
        next_id += 1;
        let size = idxs.len();
        for i in idxs {
            sources[i].cluster_id = Some(id);
            sources[i].origin_count = Some(size);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(title: &str, url: &str) -> Source {
        Source {
            title: title.to_string(),
            url: url.to_string(),
            snippet: String::new(),
            provider: "ddgs".into(),
            score: 1.0,
            level: 0,
            content: None,
            cluster_id: None,
            origin_count: None,
            best_quote: None,
        }
    }

    #[test]
    fn syndicated_copies_cluster_singletons_do_not() {
        let mut sources = vec![
            src("Acme announces Widget 2.0 launch", "https://a.io/1"),
            src("Acme announces Widget 2.0 launch", "https://b.io/2"),
            src("Acme announces Widget 2.0 launch", "https://c.io/3"),
            src("Unrelated deep dive into wasm memory64", "https://d.io/4"),
            src("Widget 2.0: what the critics say", "https://e.io/5"),
        ];
        cluster_sources(&mut sources);
        assert_eq!(sources[0].cluster_id, Some(1));
        assert_eq!(sources[2].origin_count, Some(3));
        assert_eq!(sources[3].cluster_id, None);
        assert_eq!(
            sources[4].cluster_id, None,
            "only near-identical titles merge"
        );
    }

    #[test]
    fn distinct_stories_never_merge() {
        let mut sources = vec![
            src("Rust 1.90 released with AFITLs", "https://a.io/1"),
            src("Zig 0.16 released with comptime changes", "https://b.io/2"),
        ];
        cluster_sources(&mut sources);
        assert!(sources.iter().all(|s| s.cluster_id.is_none()));
    }
}
