use super::*;

/// Keep the most lexically relevant memories for Jev judgement. Small stores
/// pass through unchanged. Memories with no term overlap are dropped only when
/// the store is larger than the candidate budget.
pub(super) fn prefilter_for_jev(entries: Vec<MemoryEntry>, query: &str) -> Vec<MemoryEntry> {
    if entries.len() <= MAX_JEV_RECALL_CANDIDATES {
        return entries;
    }
    let ranked = bm25_rank(&entries, query, MAX_JEV_RECALL_CANDIDATES);
    let mut entries: Vec<Option<MemoryEntry>> = entries.into_iter().map(Some).collect();
    ranked
        .into_iter()
        .filter_map(|(idx, _)| entries[idx].take())
        .collect()
}

/// Rank memories by BM25 over their normalized search text.
///
/// Returns `(entry_index, score)` pairs sorted by score desc, truncated to
/// `limit`. Memories with zero query-term overlap are dropped.
pub(super) fn bm25_rank(
    entries: &[MemoryEntry],
    query_text: &str,
    limit: usize,
) -> Vec<(usize, f32)> {
    const K1: f32 = 1.2;
    const B: f32 = 0.75;

    let q_terms: Vec<String> = normalize_search_text(query_text)
        .split_whitespace()
        .map(|s| s.to_string())
        .collect();
    if q_terms.is_empty() {
        return Vec::new();
    }
    let q_set: std::collections::HashSet<&String> = q_terms.iter().collect();

    // Tokenize each doc once; compute df and doc lengths.
    let docs: Vec<Vec<String>> = entries
        .iter()
        .map(|e| {
            e.searchable_text()
                .split_whitespace()
                .map(|s| s.to_string())
                .collect()
        })
        .collect();

    let n = docs.len().max(1) as f32;
    let avgdl = docs.iter().map(|d| d.len()).sum::<usize>() as f32 / n;
    let mut df: std::collections::HashMap<&str, f32> = std::collections::HashMap::new();
    for doc in &docs {
        let unique: std::collections::HashSet<&str> = doc.iter().map(|s| s.as_str()).collect();
        for t in unique {
            *df.entry(t).or_insert(0.0) += 1.0;
        }
    }

    let mut scored: Vec<(usize, f32)> = Vec::new();
    for (idx, doc) in docs.iter().enumerate() {
        if doc.is_empty() {
            continue;
        }
        let dl = doc.len() as f32;
        let mut tf: std::collections::HashMap<&str, f32> = std::collections::HashMap::new();
        for t in doc {
            *tf.entry(t.as_str()).or_insert(0.0) += 1.0;
        }
        let mut score = 0.0f32;
        for term in &q_set {
            let Some(&f) = tf.get(term.as_str()) else {
                continue;
            };
            let n_q = *df.get(term.as_str()).unwrap_or(&0.0);
            if n_q == 0.0 {
                continue;
            }
            let idf = (((n - n_q + 0.5) / (n_q + 0.5)) + 1.0).ln();
            let denom = f + K1 * (1.0 - B + B * dl / avgdl);
            score += idf * (f * (K1 + 1.0)) / denom;
        }
        if score > 0.0 {
            scored.push((idx, score));
        }
    }
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    scored.truncate(limit);
    scored
}
