use super::*;

#[test]
fn composer_mode_detects_at_file_mention() {
    assert_eq!(composer_mode("@", false), ComposerMode::FileMention);
    assert_eq!(composer_mode("@main.rs", false), ComposerMode::FileMention);
    // @ after space should not trigger
    assert_eq!(composer_mode(" @ ", false), ComposerMode::Chat);
    // mid-word @ should not trigger
    assert_eq!(composer_mode("test@main", false), ComposerMode::Chat);
    // / takes priority over @
    assert_eq!(
        composer_mode("/help @main", false),
        ComposerMode::SlashCommand
    );
    // Remote mode still allows @file
    assert_eq!(composer_mode("@main", true), ComposerMode::FileMention);
}

#[test]
fn extract_at_query_basic() {
    assert_eq!(
        extract_at_query("help @main.rs"),
        Some((5, "main.rs".to_string()))
    );
    assert_eq!(extract_at_query("@"), Some((0, "".to_string())));
}

#[test]
fn extract_at_query_ends_after_whitespace() {
    // Once the user types past the @ token, the mention is no longer
    // active: it must not drive suggestions or intercept Enter.
    assert_eq!(extract_at_query("x @main.rs y"), None);
    assert_eq!(extract_at_query("@main then continue"), None);
    assert_eq!(extract_at_query("help @main.rs "), None); // trailing space
    assert_eq!(extract_at_query("see @a, @b ok"), None);
}

#[test]
fn extract_at_query_no_trigger() {
    assert_eq!(extract_at_query("@ "), None);
    assert_eq!(extract_at_query("test@main"), None);
    assert_eq!(extract_at_query(""), None);
}

#[test]
fn accept_completion_drops_at_sign() {
    let result = accept_completion("see @main.rs", "src/main.rs");
    assert_eq!(
        result,
        Some(("see src/main.rs".to_string(), 4 + "src/main.rs".len()))
    );
    // Inactive mention (user typed past the token): nothing to accept.
    assert_eq!(
        accept_completion("see @main.rs please", "src/main.rs"),
        None
    );
}

#[test]
fn tab_complete_preserves_prefix() {
    let result = tab_complete("see @mai", "main.rs");
    assert_eq!(
        result,
        Some(("see @main.rs".to_string(), 5 + "main.rs".len()))
    );
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
