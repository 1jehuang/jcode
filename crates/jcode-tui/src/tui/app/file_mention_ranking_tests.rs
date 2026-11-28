use super::*;

#[test]
fn heavily_used_weak_match_never_outranks_cold_strong_match() {
    // Root entries avoid directory matches. Even maximal frecency must not
    // lift a filename-substring match above a cold filename-prefix match.
    let index = build_test_index(&["notes.txt", "anotes.txt"]);
    let mut frecency = empty_frecency();
    frecency
        .data
        .insert("anotes.txt".to_string(), (9_999, Frecency::now_secs()));

    let results = search_in_index("notes", &index, &frecency, limits(10));
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].path.as_ref(), "notes.txt");
    assert_eq!(results[1].path.as_ref(), "anotes.txt");
    assert!(results[0].score > results[1].score);
}

#[test]
fn frecency_bonus_reorders_only_within_a_tier() {
    // The bounded bonus breaks ties without disturbing tier order.
    let index = build_test_index(&["a/batch.rs", "other/batch.rs"]);
    let mut frecency = empty_frecency();
    frecency
        .data
        .insert("other/batch.rs".to_string(), (3, Frecency::now_secs()));

    let results = search_in_index("batch.rs", &index, &frecency, limits(10));
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].path.as_ref(), "other/batch.rs");
    assert_eq!(results[1].path.as_ref(), "a/batch.rs");
}

#[test]
fn recent_selection_frequency_breaks_equal_match_ties() {
    let index = build_test_index(&["a/lib.rs", "z/lib.rs"]);
    let mut frecency = empty_frecency();
    let now = Frecency::now_secs();
    frecency.data.insert("a/lib.rs".into(), (1, now));
    frecency.data.insert("z/lib.rs".into(), (100, now));

    let results = search_in_index("lib.rs", &index, &frecency, limits(10));
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].path.as_ref(), "z/lib.rs");
    assert_eq!(results[1].path.as_ref(), "a/lib.rs");
    assert!(results[0].score > results[1].score);
}
