use super::Registry;

impl Registry {
    /// Suggest up to 3 available tool names that look similar to `name`.
    /// Uses cheap, dependency-free heuristics: case-insensitive equality,
    /// prefix/substring containment, then bounded edit distance. Helps the
    /// model recover from hallucinated tool names (#104).
    pub(super) fn closest_tool_names(name: &str, available: &[&str]) -> Vec<String> {
        let needle = name.trim().to_ascii_lowercase();
        if needle.is_empty() {
            return Vec::new();
        }
        let mut scored: Vec<(usize, &str)> = available
            .iter()
            .filter_map(|candidate| {
                let hay = candidate.to_ascii_lowercase();
                let score = if hay == needle {
                    0
                } else if hay.starts_with(&needle) || needle.starts_with(&hay) {
                    1
                } else if hay.contains(&needle) || needle.contains(&hay) {
                    2
                } else {
                    let dist = levenshtein(&needle, &hay);
                    // Only suggest near-misses, scaled to the longer name.
                    let threshold = (hay.len().max(needle.len()) / 3).max(2);
                    if dist <= threshold {
                        3 + dist
                    } else {
                        return None;
                    }
                };
                Some((score, *candidate))
            })
            .collect();
        scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)));
        scored
            .into_iter()
            .take(3)
            .map(|(_, name)| name.to_string())
            .collect()
    }
}

/// Classic Levenshtein edit distance over Unicode scalar values.
/// Used only for tool-name "did you mean" suggestions, so the simple
/// O(n*m) two-row implementation is more than sufficient.
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr: Vec<usize> = vec![0; b.len() + 1];
    for (i, &ca) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            curr[j + 1] = (prev[j + 1] + 1).min(curr[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}
