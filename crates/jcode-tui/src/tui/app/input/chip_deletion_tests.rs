// File-chip backspace/delete behavior tests (select_files feature).
// Extracted from the inline module so the `#[path]` sibling layout stays
// consistent with `terminal_control_sequence_tests`.
use super::*;
use std::path::PathBuf;

fn chip(path: &str) -> PathBuf {
    PathBuf::from(path)
}

#[test]
fn backspace_at_chip_end_deletes_whole_path() {
    let input = "hello src/main.rs";
    let cursor = input.len();
    let chips = vec![chip("src/main.rs")];
    let start = file_chip_backspace_start(input, cursor, &chips);
    assert_eq!(start, Some(6), "should find chip start at byte 6");
}

#[test]
fn backspace_inside_chip_deletes_whole_path() {
    let input = "hello src/main.rs";
    let cursor = input.len() - 3;
    let chips = vec![chip("src/main.rs")];
    let start = file_chip_backspace_start(input, cursor, &chips);
    assert_eq!(
        start,
        Some(6),
        "cursor inside chip → bulk delete entire chip"
    );
}

#[test]
fn backspace_no_chip_is_unchanged() {
    let input = "hello world";
    let cursor = input.len();
    let chips: Vec<PathBuf> = vec![];
    let start = file_chip_backspace_start(input, cursor, &chips);
    assert_eq!(start, None);
}

#[test]
fn delete_at_chip_start_deletes_whole_path() {
    let input = "src/main.rs world";
    let cursor = 0;
    let chips = vec![chip("src/main.rs")];
    let end = file_chip_delete_end(input, cursor, &chips);
    assert_eq!(end, Some(11), "should find chip end at byte 11");
}

#[test]
fn delete_inside_chip_is_single_char() {
    let input = "src/main.rs world";
    let cursor = 3;
    let chips = vec![chip("src/main.rs")];
    let end = file_chip_delete_end(input, cursor, &chips);
    assert_eq!(end, None, "cursor not at chip start → no bulk delete");
}

#[test]
fn delete_no_chip_is_unchanged() {
    let input = "hello world";
    let cursor = 0;
    let chips: Vec<PathBuf> = vec![];
    let end = file_chip_delete_end(input, cursor, &chips);
    assert_eq!(end, None);
}

#[test]
fn longest_match_wins_on_overlap() {
    // "main.rs" is a suffix of "src/main.rs"
    let input = "check src/main.rs";
    let cursor = input.len();
    let chips = vec![chip("src/main.rs"), chip("main.rs")];
    let start = file_chip_backspace_start(input, cursor, &chips);
    assert_eq!(
        start,
        Some(6),
        "longest match 'src/main.rs' wins over 'main.rs'"
    );
}

#[test]
fn backspace_at_chip_end_with_non_ascii_path() {
    let input = "查看 中文路径/文件.rs";
    let cursor = input.len();
    let chips = vec![chip("中文路径/文件.rs")];
    let start = file_chip_backspace_start(input, cursor, &chips);
    assert!(start.is_some(), "should handle non-ASCII paths");
    assert_eq!(&input[start.unwrap()..cursor], "中文路径/文件.rs");
}

/// Simulates the full flow: user selects @src/main.rs → chip stored,
/// cursor ends at path end → Backspace deletes entire path.
#[test]
fn backspace_deletes_whole_path_realistic_scenario() {
    // After picking "src/main.rs" from @ suggestions, input is:
    let input = "hello src/main.rs some more text";
    // Cursor is at end of path: "hello src/main.rs" has 17 bytes,
    // cursor on the space after main.rs
    let cursor = 17; // right after "hello src/main.rs"
    let chips = vec![chip("src/main.rs")];
    let start = file_chip_backspace_start(input, cursor, &chips);
    assert_eq!(start, Some(6), "should find chip start");
    // After drain(6..17): "hello  some more text"
    let mut s = input.to_string();
    s.drain(start.unwrap()..cursor);
    assert_eq!(s, "hello  some more text", "entire path removed at once");
}

/// Cursor at path end with suffix text: Backspace still deletes whole path.
#[test]
fn backspace_at_path_end_with_suffix() {
    let input = "hello src/main.rs";
    let cursor = input.len(); // 17
    let chips = vec![chip("src/main.rs")];
    let start = file_chip_backspace_start(input, cursor, &chips);
    assert_eq!(start, Some(6));
    let mut s = input.to_string();
    s.drain(start.unwrap()..cursor);
    assert_eq!(s, "hello ", "path fully removed, cursor at 'hello '");
}

/// Delete at path start with more text before: Delete removes whole path.
#[test]
fn delete_at_path_start_realistic() {
    let input = "hello src/main.rs more";
    let cursor = 6; // at start of "src/main.rs"
    let chips = vec![chip("src/main.rs")];
    let end = file_chip_delete_end(input, cursor, &chips);
    assert_eq!(end, Some(17)); // "src/main.rs".len() == 11, 6+11=17
    let mut s = input.to_string();
    s.drain(cursor..end.unwrap());
    assert_eq!(s, "hello  more", "Delete removes whole path at once");
}

#[test]
fn file_chip_backspace_after_non_ascii_suffix() {
    let chips = [PathBuf::from("src/main.rs")];
    // "help " (5) + "src/main.rs" (11) + "测试" (6) = 22 bytes
    let input = "help src/main.rs\u{6d4b}\u{8bd5}";
    let cursor = input.len(); // 22
    let start = file_chip_backspace_start(input, cursor, &chips);
    // Cursor past chip end → None (fallback to single-char deletion).
    assert_eq!(
        start, None,
        "cursor past chip → normal single-char backspace"
    );

    // Simulate single-char backspaces one at a time.
    let mut s = String::from(input);
    // 1st BS: prev_char_boundary from 22 → 19 (first byte of 试)
    let prev = crate::tui::core::prev_char_boundary(&s, 22);
    s.drain(prev..s.len());
    assert_eq!(s, "help src/main.rs\u{6d4b}");
    // 2nd BS: prev_char_boundary from 19 → 16 (end of chip)
    let prev = crate::tui::core::prev_char_boundary(&s, s.len());
    s.drain(prev..s.len());
    assert_eq!(s, "help src/main.rs");

    // Now cursor at chip end → delete entire chip.
    let start2 = file_chip_backspace_start(&s, s.len(), &chips);
    assert_eq!(start2, Some(5), "cursor at chip end → delete entire chip");
    s.drain(start2.unwrap()..s.len());
    assert_eq!(s, "help ");
}

/// A chip path must not match as a substring of a longer path when the
/// adjacent characters are not path/whitespace boundaries. Here `main.rs`
/// is a suffix of `src/main.rs.bak`; the `.` after `main.rs` is not a path
/// boundary, so the short chip must not match at that position.
#[test]
fn chip_does_not_match_inside_longer_path_with_dot_suffix() {
    let input = "edit src/main.rs.bak";
    let cursor = input.len();
    // `main.rs` should not match inside `src/main.rs.bak`.
    let chips = [PathBuf::from("main.rs")];
    let start = file_chip_backspace_start(input, cursor, &chips);
    assert_eq!(
        start, None,
        "main.rs must not match the substring of src/main.rs.bak"
    );

    // `src/main.rs.bak` matches as a whole chip.
    let chips = [PathBuf::from("src/main.rs.bak")];
    let start = file_chip_backspace_start(input, cursor, &chips);
    assert_eq!(start, Some(5), "full path chip matches on its own");
}

/// The shared chip-presence matcher must agree with the span finder: a chip
/// survives pruning (and highlights) only when it lands on a path-boundary,
/// never mid-token. A `bag` chip must not stay attached to `char_bag.rs`.
#[test]
fn file_chip_matches_input_respects_path_boundaries() {
    use crate::tui::app::input::file_chip_matches_input;

    assert!(file_chip_matches_input("see src/main.rs", &PathBuf::from("src/main.rs")));
    assert!(file_chip_matches_input("src/main.rs now", &PathBuf::from("src/main.rs")));
    assert!(file_chip_matches_input("look at main.rs", &PathBuf::from("main.rs")));
    // Mid-token inside a longer path: not a match.
    assert!(!file_chip_matches_input("edit char_bag.rs", &PathBuf::from("bag")));
    assert!(!file_chip_matches_input("edit src/main.rs.bak", &PathBuf::from("main.rs")));
    // Word-adjacent text without a boundary is not a match either.
    assert!(!file_chip_matches_input("xxmain.rs", &PathBuf::from("main.rs")));
    // Slash-separated parents are fine: the boundary is a path separator.
    assert!(file_chip_matches_input("src/main.rs", &PathBuf::from("main.rs")));
}
