// Regression tests for the greptile defect batch on the @file mention
// feature (PR 1889):
// - A3: stash and history drafts carry file chips and cursor so a restored
//   draft keeps its attachments exactly as it was stashed.
// - A5: the Tab cycle re-derives its candidate list from the saved base
//   query, so repeated Tab after a unique completion keeps cycling the
//   original candidates instead of getting stuck.
// - A10: a chip accepted directly after an opening bracket stays attached
//   (styled, backspace-atomic, pruned correctly); the same path inside
//   prose with closing brackets never attaches.

use std::path::PathBuf;

/// Install a fixed file index into a cache bound to `cwd`, then set the
/// app's session working_dir to the same path so `check_refresh` keeps the
/// seeded index instead of discarding it for a cwd mismatch (which would
/// kick a background rebuild and race the test).
fn seed_file_mention_cache(app: &mut App, cwd: &std::path::Path, paths: &[&str]) {
    let cache =
        crate::tui::app::file_mention::FileMentionCache::with_cwd_and_test_index(cwd, paths);
    *app.file_mention_cache.borrow_mut() = cache;
    app.session.working_dir = Some(cwd.to_string_lossy().to_string());
}

#[test]
fn file_mention_tab_cycle_survives_unique_completion() {
    let mut app = create_test_app();
    app.is_remote = false;
    let dir = tempfile::tempdir().unwrap();
    seed_file_mention_cache(&mut app, dir.path(), &["note_a.txt", "note_b.txt"]);

    // Step 1: "note" shares the common prefix "note_" beyond the query, so
    // the first Tab completes the prefix and clears the cycle state.
    app.input = "@note".to_string();
    app.cursor_pos = app.input.len();
    assert!(app.autocomplete());
    assert_eq!(app.input, "@note_");

    // Step 2: query "note_" has no longer shared prefix, so Tab enters the
    // cycle and completes to the first candidate.
    assert!(app.autocomplete());
    assert_eq!(app.input, "@note_a.txt");

    // The defect: the old code re-derived candidates from the narrowed
    // input "note_a.txt" and got stuck re-offering note_a.txt forever.
    // The cycle must advance to note_b.txt from the saved base query.
    assert!(app.autocomplete());
    assert_eq!(
        app.input, "@note_b.txt",
        "repeated Tab must advance through the original candidate set"
    );
}

#[test]
fn file_mention_tab_cycle_wraps_around() {
    let mut app = create_test_app();
    app.is_remote = false;
    let dir = tempfile::tempdir().unwrap();
    seed_file_mention_cache(&mut app, dir.path(), &["note_a.txt", "note_b.txt"]);

    app.input = "@note_".to_string();
    app.cursor_pos = app.input.len();
    assert!(app.autocomplete());
    assert_eq!(app.input, "@note_a.txt");
    assert!(app.autocomplete());
    assert_eq!(app.input, "@note_b.txt");
    // Wrap back to the first candidate.
    assert!(app.autocomplete());
    assert_eq!(app.input, "@note_a.txt");
}

#[test]
fn file_mention_tab_cycle_single_candidate_is_stable() {
    let mut app = create_test_app();
    app.is_remote = false;
    let dir = tempfile::tempdir().unwrap();
    seed_file_mention_cache(&mut app, dir.path(), &["only.rs"]);

    app.input = "@only".to_string();
    app.cursor_pos = app.input.len();
    assert!(app.autocomplete());
    assert_eq!(app.input, "@only.rs");
    // With one candidate the cycle cannot advance; repeated Tab keeps the
    // same completion (and must not corrupt the input).
    assert!(app.autocomplete());
    assert_eq!(app.input, "@only.rs");
}

#[test]
fn chip_after_opening_bracket_stays_attached() {
    // A10: accept_completion can leave "(main.rs" in the composer; the chip
    // must still match so styling, pruning, and atomic deletion all work.
    let input = "(main.rs";
    let chips = [PathBuf::from("main.rs")];
    assert!(crate::tui::app::input::file_chip_matches_input(
        input, &chips[0]
    ));
    // Backspace at the end deletes the whole path in one stroke.
    let span =
        crate::tui::app::input::file_chip_backspace_span(input, input.len(), &chips);
    assert_eq!(span, Some((1, 8)));
}

#[test]
fn chip_after_closing_bracket_never_attaches() {
    // Prose like "(see main.rs)" must not treat main.rs as a chip.
    let input = "(see main.rs)";
    let chips = [PathBuf::from("main.rs")];
    assert!(!crate::tui::app::input::file_chip_matches_input(
        input, &chips[0]
    ));
    let span =
        crate::tui::app::input::file_chip_backspace_span(input, input.len(), &chips);
    assert_eq!(span, None);
}

/// A3: Ctrl+Up away from a draft with attached chips, then Ctrl+Down all
/// the way back, must restore the text, the cursor, and the chips.
#[test]
fn history_draft_restore_keeps_chips_and_cursor() {
    let mut app = create_test_app();
    app.is_remote = false;
    app.display_messages = vec![
        DisplayMessage::user("previous prompt"),
        DisplayMessage::assistant("previous response"),
    ];
    app.input = "edit src/main.rs please".to_string();
    app.cursor_pos = "edit src/".len();
    app.file_chips = vec![PathBuf::from("src/main.rs")];

    // Up walks into history; the draft (text + chips + cursor) is parked.
    app.handle_key(KeyCode::Up, KeyModifiers::CONTROL).unwrap();
    assert_eq!(app.input, "previous prompt");
    // Typing while in history would detach from the walk (edited entries no
    // longer match the history list), so exercise the prune path directly:
    // reset_tab_completion on the history entry must leave the parked draft
    // chips untouched.
    app.reset_tab_completion();

    // Down walks back: the draft returns with its chips and cursor.
    app.handle_key(KeyCode::Down, KeyModifiers::CONTROL).unwrap();
    assert_eq!(app.input, "edit src/main.rs please");
    assert_eq!(app.cursor_pos, "edit src/".len());
    assert_eq!(app.file_chips, vec![PathBuf::from("src/main.rs")]);
}

/// A3: stashing a chipped draft, typing over the empty composer, and
/// restoring the stash must bring the chips back with the text.
#[test]
fn input_stash_roundtrip_keeps_chips() {
    let mut app = create_test_app();
    app.is_remote = false;
    app.input = "review src/app.rs now".to_string();
    app.cursor_pos = "review ".len();
    app.file_chips = vec![PathBuf::from("src/app.rs")];

    app.toggle_input_stash();
    assert_eq!(app.input, "");
    assert!(app.file_chips.is_empty(), "empty composer must prune to no chips");

    // Type over the emptied composer; the stashed draft is unaffected.
    app.input = "unrelated text".to_string();
    app.cursor_pos = app.input.len();
    app.reset_tab_completion();

    app.toggle_input_stash();
    assert_eq!(app.input, "review src/app.rs now");
    assert_eq!(app.file_chips, vec![PathBuf::from("src/app.rs")]);
}

#[test]
fn backspace_inside_chip_deletes_whole_path_caller_level() {
    // A11: the caller must drain the full [start, end) span. The helper
    // returns the span; simulate the handle_basic_key drain to pin the
    // contract the three call sites now implement.
    let input = "src/main.rs";
    let cursor = "src/".len();
    let chips = [PathBuf::from("src/main.rs")];
    let span =
        crate::tui::app::input::file_chip_backspace_span(input, cursor, &chips);
    let (start, end) = span.expect("cursor inside chip must yield a span");
    let mut s = input.to_string();
    s.drain(start..end);
    assert_eq!(s, "", "whole path must be removed, no tail may survive");
}