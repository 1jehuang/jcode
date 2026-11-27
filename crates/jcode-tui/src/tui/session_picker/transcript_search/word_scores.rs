//! Keep the hot word-scoring loop in the already optimized fuzzy leaf crate.
pub(super) use jcode_fuzzy::CachedWordScores as WordScores;

#[cfg(test)]
mod tests {
    use super::*;
    use jcode_fuzzy::PreparedTokenQuery;

    #[test]
    fn memoized_metadata_rank_matches_uncached_reference_across_candidates() {
        let mut scores = WordScores::default();
        for query in [
            "conifg worker",
            "api missing",
            "café 東京",
            "config config",
            "foo-bar",
        ] {
            let tokens = super::super::super::loading::search_query_tokens(query);
            for text in [
                "config worker config deployment",
                "café config 東京 worker",
                "api apt /tmp/foo-bar/config.json",
                "unrelated tokens with repeated repeated words",
                "",
            ] {
                let text = text.repeat(20);
                let mut expected = super::super::literal_metadata_rank(&text, &tokens);
                expected.score = tokens
                    .iter()
                    .filter_map(|token| {
                        let prepared = PreparedTokenQuery::new(token);
                        text.split_whitespace()
                            .filter(|word| word.chars().count() <= 128)
                            .filter_map(|word| prepared.score(word))
                            .max()
                            .map(i64::from)
                    })
                    .sum();
                let actual =
                    super::super::fuzzy_metadata_rank_with_scores(&text, &tokens, &mut scores);
                assert_eq!(actual, expected, "query={query}, text={text}");
            }
        }
    }
}
