use super::*;

#[test]
fn prelowercased_index_matches_legacy_hash_and_overflow_policy() {
    let mut texts = vec![
        "ALPHA alpha and THE 0 1 x xx\0\tΟΣ ΟΣΑ İİ İSTANBUL e\u{301} É".to_owned(),
        "z".repeat(MAX_INDEX_TOKEN_LEN + 1),
        "alpha ".repeat(1000),
    ];
    texts.push(
        (0..MAX_TOKENS_PER_FILE + 100)
            .map(|i| format!("token{i} "))
            .collect(),
    );
    for text in texts {
        let tokens = jcode_session_types::tokenize_session_search_query(&text.to_lowercase());
        let mut hashes: Vec<_> = tokens
            .iter()
            .filter(|t| t.len() <= MAX_INDEX_TOKEN_LEN)
            .map(|t| hash_token(t))
            .collect();
        hashes.sort_unstable();
        hashes.dedup();
        let overflow = hashes.len() > MAX_TOKENS_PER_FILE;
        hashes.truncate(MAX_TOKENS_PER_FILE);
        assert_eq!(hash_tokens_for_text(&text), (hashes, overflow));
    }
}
