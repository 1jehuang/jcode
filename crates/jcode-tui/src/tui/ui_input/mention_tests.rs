use super::*;

#[test]
fn composer_mode_cursor_inside_multibyte_char_does_not_panic() {
    // A raw byte cursor landed inside the multibyte 'é' (2 bytes); paste and
    // remote paths can produce such offsets. Slicing input before the
    // cursor must clamp to a char boundary instead of panicking.
    let input = "café/naïve";
    let mid_char = input.find('é').unwrap() + 1; // inside 'é'
    assert_eq!(composer_mode(input, mid_char, false), ComposerMode::Chat);
    // The mention path reads the same bounded cursor and must not panic
    // either.
    let input = "@café";
    let mid_char = input.find('é').unwrap() + 1;
    assert_eq!(
        composer_mode(input, mid_char, false),
        ComposerMode::FileMention
    );
}

#[test]
fn extract_at_query_bracket_and_newline_boundaries() {
    // Opening brackets and newlines are valid left-of-@ boundaries, so
    // the picker must open for @ in any of these positions.
    for (input, at_pos) in [
        ("(@main.rs", 1),
        ("[@main.rs", 1),
        ("{@main.rs", 1),
        ("see ( @main.rs", 6),
        ("line one\n@main.rs", 9),
        ("list:\n  @main.rs", 8),
    ] {
        assert_eq!(
            extract_at_query_for_cursor(input, input.len()),
            Some((at_pos, "main.rs".to_string())),
            "bracket/newline boundary must open the picker for {input:?}"
        );
        assert_eq!(
            composer_mode(input, input.len(), false),
            ComposerMode::FileMention
        );
    }
    // A closing bracket immediately left of @ is not a boundary.
    assert_eq!(extract_at_query_for_cursor(")@main", 5), None);
    assert_eq!(composer_mode(")@main", 5, false), ComposerMode::Chat);
}

#[test]
fn composer_mode_detects_at_file_mention() {
    assert_eq!(composer_mode("@", 1, false), ComposerMode::FileMention);
    assert_eq!(
        composer_mode("@main.rs", 8, false),
        ComposerMode::FileMention
    );
    // @ after space should not trigger
    assert_eq!(composer_mode(" @ ", 3, false), ComposerMode::Chat);
    // mid-word @ should not trigger
    assert_eq!(composer_mode("test@main", 9, false), ComposerMode::Chat);
    // / takes priority over @
    assert_eq!(
        composer_mode("/help @main", 12, false),
        ComposerMode::SlashCommand
    );
    // Remote mode still allows @file
    assert_eq!(composer_mode("@main", 5, true), ComposerMode::FileMention);
}

#[test]
fn extract_at_query_basic() {
    assert_eq!(
        extract_at_query_for_cursor("help @main.rs", "help @main.rs".len()),
        Some((5, "main.rs".to_string()))
    );
    assert_eq!(
        extract_at_query_for_cursor("@", "@".len()),
        Some((0, "".to_string()))
    );
}

#[test]
fn extract_at_query_ends_after_whitespace() {
    // Once the user types past the @ token, the mention is no longer
    // active: it must not drive suggestions or intercept Enter.
    assert_eq!(
        extract_at_query_for_cursor("x @main.rs y", "x @main.rs y".len()),
        None
    );
    assert_eq!(
        extract_at_query_for_cursor("@main then continue", "@main then continue".len()),
        None
    );
    assert_eq!(
        extract_at_query_for_cursor("help @main.rs ", "help @main.rs ".len()),
        None
    ); // trailing space
    assert_eq!(
        extract_at_query_for_cursor("see @a, @b ok", "see @a, @b ok".len()),
        None
    );
}

#[test]
fn extract_at_query_no_trigger() {
    assert_eq!(extract_at_query_for_cursor("@ ", "@ ".len()), None);
    assert_eq!(
        extract_at_query_for_cursor("test@main", "test@main".len()),
        None
    );
    assert_eq!(extract_at_query_for_cursor("", "".len()), None);
}

#[test]
fn accept_completion_drops_at_sign() {
    let result = accept_completion_at("see @main.rs", "see @main.rs".len(), "src/main.rs");
    assert_eq!(
        result,
        Some(("see src/main.rs".to_string(), 4 + "src/main.rs".len()))
    );
    // Inactive mention (user typed past the token): nothing to accept.
    assert_eq!(
        accept_completion_at(
            "see @main.rs please",
            "see @main.rs please".len(),
            "src/main.rs"
        ),
        None
    );
}

#[test]
fn tab_complete_preserves_prefix() {
    let result = tab_complete_at("see @mai", "see @mai".len(), "main.rs");
    assert_eq!(
        result,
        Some(("see @main.rs".to_string(), 5 + "main.rs".len()))
    );
}

#[test]
fn extract_at_query_mid_input_cursor() {
    // Typing @ in the middle of existing text: the caret sits between the
    // @ token and the rest of the sentence, and the mention is active.
    assert_eq!(
        extract_at_query_for_cursor("help @ lo", 6),
        Some((5, "".to_string()))
    );
    assert_eq!(
        extract_at_query_for_cursor("help @ma lo", 8),
        Some((5, "ma".to_string()))
    );
    // Caret moved left of the @: no active mention even though the @ is
    // still the last one before end-of-input.
    assert_eq!(extract_at_query_for_cursor("help @main.rs", 4), None);
    // Caret before a later @: the later @ is the active one.
    assert_eq!(
        extract_at_query_for_cursor("@a and @b here", 8),
        Some((7, "".to_string()))
    );
    // Caret on a multibyte boundary must not panic.
    assert_eq!(extract_at_query_for_cursor("héllo @main", 1), None);
    assert_eq!(extract_at_query_for_cursor("héllo @main", 2), None);
}

#[test]
fn extract_at_query_cursor_at_end_matches_expected() {
    // The cursor at end-of-input is the canonical mention state; pin the
    // exact answers so later refactors cannot drift.
    let cases: &[(&str, Option<(usize, &str)>)] = &[
        ("", None),
        ("@", Some((0, ""))),
        ("@ ", None),
        ("help @main.rs", Some((5, "main.rs"))),
        ("help @main.rs ", None),
        ("x @main.rs y", None),
        ("test@main", None),
        ("@a and @b", Some((7, "b"))),
        ("see @a, @b ok", None),
    ];
    for (input, expected) in cases {
        assert_eq!(
            extract_at_query_for_cursor(input, input.len()),
            expected.map(|(p, q)| (p, q.to_string())),
            "mismatch for {input:?}"
        );
    }
}

#[test]
fn accept_and_tab_complete_mid_input() {
    // Completing with text after the caret replaces only the @ token.
    let result = accept_completion_at("help @ma and more", 8, "src/main.rs");
    assert_eq!(
        result,
        Some((
            "help src/main.rs and more".to_string(),
            5 + "src/main.rs".len()
        ))
    );

    let result = tab_complete_at("help @ma and more", 8, "src/main.rs");
    assert_eq!(
        result,
        Some((
            "help @src/main.rs and more".to_string(),
            6 + "src/main.rs".len()
        ))
    );
}

#[test]
fn composer_mode_mid_input_at_mention() {
    // Full composer mode agrees with the extractor at a mid-input caret.
    assert_eq!(
        composer_mode("help @ lo", 6, false),
        ComposerMode::FileMention
    );
    // Caret left of the @: plain chat, the mention is not being edited.
    assert_eq!(composer_mode("help @main.rs", 4, false), ComposerMode::Chat);
}

#[test]
fn effective_selected_row_skips_headers_in_file_mention_mode() {
    let rows: Vec<(String, &'static str)> = vec![
        (String::new(), "── Recent ──"),
        ("src/main.rs".to_string(), "recent"),
        ("src/lib.rs".to_string(), ""),
    ];
    // Selection on the header resolves to the next real file.
    assert_eq!(effective_selected_row(0, &rows, true), 1);
    // A real row stays untouched.
    assert_eq!(effective_selected_row(1, &rows, true), 1);
    assert_eq!(effective_selected_row(2, &rows, true), 2);
    // Out-of-range selection clamps to the last row (a real file), which
    // then needs no skip.
    assert_eq!(effective_selected_row(9, &rows, true), 2);

    // Header at the end wraps to the first real row.
    let tail_header = vec![("a.rs".to_string(), ""), (String::new(), "── Files ──")];
    assert_eq!(effective_selected_row(1, &tail_header, true), 0);

    // All-header list: terminates without panicking, index unchanged.
    let all_headers = vec![(String::new(), "a"), (String::new(), "b")];
    assert_eq!(effective_selected_row(1, &all_headers, true), 1);

    // Empty list is safe.
    assert_eq!(effective_selected_row(3, &[], true), 0);

    // Slash-command mode never skips (headers only exist in the
    // file-mention popover).
    assert_eq!(effective_selected_row(0, &rows, false), 0);
}
