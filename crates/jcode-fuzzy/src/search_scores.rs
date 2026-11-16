//! Bounded, query-local memoization of fuzzy word scores. Repeated words in
//! metadata, tool output, and different transcripts must not rerun the DP.
//! Entries include misses, never outlive a search, and never cache file results.
use crate::PreparedTokenQuery;
use std::collections::HashMap;

const MAX_WORDS: usize = 4096;
const MAX_BYTES: usize = 512 * 1024;

struct TokenScores {
    prepared: PreparedTokenQuery,
    words: HashMap<Box<str>, Option<i32>>,
}

#[derive(Default)]
/// Reuses prepared single-token queries and their word scores during a search.
/// Positive and negative scores share a 4,096-entry / 512 KiB retention budget.
/// Exhausting the budget disables new word retention, not matching. The caller
/// owns the lifetime, so this never caches mutable file or session contents.
pub struct CachedWordScores {
    tokens: HashMap<String, TokenScores>,
    entries: usize,
    bytes: usize,
}

impl CachedWordScores {
    fn prepare(&mut self, token: &str) {
        if !self.tokens.contains_key(token) {
            self.tokens.insert(
                token.to_owned(),
                TokenScores {
                    prepared: PreparedTokenQuery::new(token),
                    words: HashMap::new(),
                },
            );
        }
    }

    /// Score one whitespace-delimited word, bounded to 128 Unicode characters.
    pub fn score(&mut self, token: &str, word: &str) -> Option<i32> {
        self.prepare(token);
        // A missing cache entry is a miss, never a reason to panic in search.
        let scores = self.tokens.get_mut(token)?;
        Self::score_word(scores, &mut self.entries, &mut self.bytes, word)
    }

    /// Best score within one text segment. Optional length-distance penalties
    /// distinguish a near typo from an incidental longer prefix/suffix match.
    pub fn best_score(&mut self, token: &str, text: &str, penalize_length: bool) -> Option<i32> {
        self.prepare(token);
        // A missing cache entry is a miss, never a reason to panic in search.
        let scores = self.tokens.get_mut(token)?;
        let token_len = if penalize_length {
            token.chars().count()
        } else {
            0
        };
        text.split_whitespace()
            .filter_map(|word| {
                Self::score_word(scores, &mut self.entries, &mut self.bytes, word).map(|score| {
                    if penalize_length {
                        score - word.chars().count().abs_diff(token_len) as i32
                    } else {
                        score
                    }
                })
            })
            .max()
    }

    fn score_word(
        scores: &mut TokenScores,
        entries: &mut usize,
        retained_bytes: &mut usize,
        word: &str,
    ) -> Option<i32> {
        // Preserve the searcher's DP bound for URLs and minified output.
        if word.len() > 128 && word.chars().count() > 128 {
            return None;
        }
        if let Some(score) = scores.words.get(word) {
            return *score;
        }
        let score = scores.prepared.score(word);
        // Account for keys, values and hash-table bookkeeping conservatively.
        // Once full, retain useful early vocabulary and score other words
        // normally. Exhausting the budget must not change search results.
        let bytes = word.len() + std::mem::size_of::<(Box<str>, Option<i32>)>() + 32;
        if *entries < MAX_WORDS && *retained_bytes + bytes <= MAX_BYTES {
            scores.words.insert(word.into(), score);
            *entries += 1;
            *retained_bytes += bytes;
        }
        score
    }

    /// Match a single normalized token against lowercased search text. Short or
    /// nonalphabetic tokens remain literal, like `matches_search_text`.
    pub fn matches_text(&mut self, token: &str, text: &str) -> bool {
        text.contains(token)
            || (token.chars().count() >= 4
                && token.chars().all(char::is_alphabetic)
                && text
                    .split_whitespace()
                    .any(|word| self.score(token, word).is_some()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_scores_and_boolean_matching_preserve_shared_matcher_semantics() {
        let mut scores = CachedWordScores::default();
        for query in [
            "conifg", "config", "api", "café", "東京", "foo-bar", "missing", "clodu",
        ] {
            let prepared = PreparedTokenQuery::new(query);
            for text in [
                "config config deployment",
                "café CAFÉ caféine",
                "cloud acceptance todo",
                "api apt",
                "foo-bar /tmp/config.json",
                "unrelated text",
                "東京 東京駅",
                "",
            ] {
                assert_eq!(
                    scores.matches_text(query, text),
                    prepared.matches_search_text(text),
                    "query={query}, text={text}"
                );
                for word in text.split_whitespace() {
                    assert_eq!(scores.score(query, word), prepared.score(word));
                    assert_eq!(scores.score(query, word), prepared.score(word));
                }
                for penalize_length in [false, true] {
                    let expected = text
                        .split_whitespace()
                        .filter_map(|word| {
                            prepared.score(word).map(|score| {
                                if penalize_length {
                                    score
                                        - word.chars().count().abs_diff(query.chars().count())
                                            as i32
                                } else {
                                    score
                                }
                            })
                        })
                        .max();
                    assert_eq!(scores.best_score(query, text, penalize_length), expected);
                }
            }
        }
    }

    #[test]
    fn repeated_scores_reuse_entries_including_negative_results() {
        let mut scores = CachedWordScores::default();
        let hit = scores.score("conifg", "config");
        assert!(hit.is_some());
        assert_eq!(scores.score("conifg", "zzzzzz"), None);
        for _ in 0..100 {
            assert_eq!(scores.score("conifg", "config"), hit);
            assert_eq!(scores.score("conifg", "zzzzzz"), None);
        }
        assert_eq!(scores.tokens.len(), 1);
        assert_eq!(scores.entries, 2);
    }

    #[test]
    fn cache_budget_and_long_words_never_change_eligible_scores() {
        let mut scores = CachedWordScores::default();
        let prepared = PreparedTokenQuery::new("conifg");
        for index in 0..MAX_WORDS + 100 {
            let word = format!("config-{index}");
            assert_eq!(scores.score("conifg", &word), prepared.score(&word));
        }
        assert!(scores.entries <= MAX_WORDS);
        assert!(scores.bytes <= MAX_BYTES);
        assert_eq!(scores.score("conifg", "config"), prepared.score("config"));
        assert_eq!(scores.score("conifg", &"x".repeat(129)), None);
        let unicode = "é".repeat(128);
        assert_eq!(
            scores.score("café", &unicode),
            PreparedTokenQuery::new("café").score(&unicode)
        );
    }
}
