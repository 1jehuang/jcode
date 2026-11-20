use super::*;

#[path = "prelowercased_tokenization_probe.rs"]
mod probe;

fn legacy(text: &str) -> Vec<String> {
    tokenize_session_search_query(&text.to_lowercase())
}

#[test]
fn prelowered_matches_historical_index_normalization() {
    let cases = [
        "",
        "A a an AND and the THE alpha ALPHA beta 0 1 12 x xx",
        "\0alpha\x01beta\tGAMMA\nDELTA\r\x7fomega",
        "ΟΣ ΟΣΑ Σ ΣΣ ΟΣ\u{301} ΑΣ'Α İ İİ I ı i Iİ",
        "e\u{301} É E\u{307} İSTANBUL Straße ẞ K ﬀ 中文 日本語",
        "alpha-alpha_alpha alpha123 123 alpha123 and and and",
    ];
    for text in cases {
        assert_eq!(
            tokenize_prelowercased_session_search_text(&text.to_lowercase()),
            legacy(text),
            "{text:?}"
        );
    }
    // Exercise every Unicode scalar, both isolated and in contextual sigma strings.
    for scalar in 0..=0x10ffff {
        let Some(c) = char::from_u32(scalar) else {
            continue;
        };
        let text = format!("AA{c}Σ {c}{c} {c}İ ΟΣ{c} ZZ");
        assert_eq!(
            tokenize_prelowercased_session_search_text(&text.to_lowercase()),
            legacy(&text),
            "U+{scalar:04X}"
        );
    }
}

#[test]
fn ordinary_query_retains_split_then_lower_semantics() {
    assert_eq!(
        tokenize_session_search_query("İİ"),
        vec!["i\u{307}i\u{307}"]
    );
    assert!(tokenize_prelowercased_session_search_text(&"İİ".to_lowercase()).is_empty());
    let text = "alpha ".repeat(1000) + &"z".repeat(100_000);
    assert_eq!(
        tokenize_prelowercased_session_search_text(&text),
        legacy(&text)
    );
}
