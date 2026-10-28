// Integration tests for the `@file` mention popover sentinel rows.
//
// Section headers (empty cmd) and the "building index" hint row are sentinels:
// they render inside the suggestion popover but must never be accepted as a
// completion. Accepting either would insert junk text as a file chip and
// rewrite the composer input.

/// Seed the per-frame suggestion memo so the popover shows exactly `rows`
/// without touching the index. The cache fields are private to `app`, but the
/// tests module is a child of `app`, so it can populate them directly.
fn seed_suggestions(app: &mut App, rows: Vec<(String, &'static str)>) {
    let signature = app.command_suggestions_signature();
    let epoch = app.command_suggestions_epoch.get();
    *app.command_suggestions_cache.borrow_mut() = Some(super::CommandSuggestionsCache {
        input: app.input.clone(),
        signature,
        epoch,
        suggestions: rows,
    });
    app.command_suggestion_selected = 0;
}

/// Enter on the "building index" hint row must not modify the input or add a
/// chip. The hint row only appears while the initial index build is in flight
/// and produces no candidates, so the test seeds it directly into the popover.
#[test]
fn file_mention_enter_on_building_hint_row_is_inert() {
    let mut app = create_test_app();
    app.is_remote = false;
    app.input = "@src".to_string();
    app.cursor_pos = app.input.len();

    seed_suggestions(
        &mut app,
        vec![(
            "⏳ Building file index...".to_string(),
            "first use takes a few seconds",
        )],
    );

    let input_before = app.input.clone();
    let chips_before = app.file_chips.clone();

    let accepted = app.accept_selected_command_suggestion();
    assert!(!accepted, "the hint row must be rejected by the accept guard");
    assert_eq!(app.input, input_before, "input must stay untouched");
    assert_eq!(app.file_chips, chips_before, "no chip may be recorded");
}

/// Enter on a section-header row (empty cmd) accepts the next real file:
/// headers are unselectable virtual rows, so the accept path skips past them
/// downward. This pins the designed behavior so a future refactor cannot
/// silently start inserting header text or make Enter inert.
#[test]
fn file_mention_enter_on_section_header_row_accepts_next_file() {
    let mut app = create_test_app();
    app.is_remote = false;
    app.input = "@src".to_string();
    app.cursor_pos = app.input.len();

    seed_suggestions(
        &mut app,
        vec![
            (String::new(), "── Recent ──"),
            ("src/main.rs".to_string(), "recent"),
        ],
    );
    // Land the selection on the header row.
    app.command_suggestion_selected = 0;

    let accepted = app.accept_selected_command_suggestion();
    assert!(
        accepted,
        "Enter on a header must accept the next real file, not the header itself"
    );
    assert!(
        app.file_chips
            .iter()
            .any(|c| c.to_string_lossy() == "src/main.rs"),
        "the row after the header must be recorded as the chip"
    );
    assert!(
        !app.input.contains('@'),
        "the @ sign must be dropped after accepting a completion"
    );
}

/// A real path suggestion still flows through the accept path: Enter replaces
/// the @query with the path and records a chip.
#[test]
fn file_mention_enter_on_real_path_accepts_and_records_chip() {
    use std::time::{Duration, Instant};

    let mut app = create_test_app();
    app.is_remote = false;
    // Point at this crate so the index has real files to offer. The manifest
    // dir is stable regardless of where cargo runs the test binary from.
    app.session.working_dir = Some(env!("CARGO_MANIFEST_DIR").to_string());

    // Seed the index: type a query, let check_refresh see the cwd, and wait for
    // the async build to finish (bounded). The dedicated runtime serves the
    // tokio::spawn calls in refresh_async.
    app.input = "@input.rs".to_string();
    app.cursor_pos = app.input.len();

    let rt = tokio::runtime::Runtime::new().expect("test runtime");
    let _guard = rt.enter();

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        // command_suggestions memoizes per frame; the real TUI advances the
        // epoch on every rendered frame, so the test must do the same.
        app.advance_command_suggestions_epoch();
        let suggestions = app.command_suggestions();
        let has_real_row = suggestions
            .iter()
            .any(|(cmd, _)| !cmd.is_empty() && !cmd.starts_with('⏳'));
        if has_real_row || Instant::now() > deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    // Accept the first real (non-sentinel) row.
    let suggestions = app.command_suggestions();
    let real = suggestions
        .iter()
        .find(|(cmd, _)| !cmd.is_empty() && !cmd.starts_with('⏳'));

    let Some((path, _)) = real.cloned() else {
        panic!("expected at least one real path suggestion for @input.rs");
    };
    let index = suggestions
        .iter()
        .position(|(cmd, _)| cmd == &path)
        .unwrap_or(0);
    app.command_suggestion_selected = index;

    let accepted = app.accept_selected_command_suggestion();
    assert!(accepted, "accepting a real path should succeed");
    assert!(
        app.file_chips.iter().any(|c| c.to_string_lossy() == path),
        "chip must be recorded for the accepted path"
    );
    assert!(
        !app.input.contains('@'),
        "the @ sign must be dropped after accepting a completion"
    );
}

/// Accepting an `@~` suggestion records the literal `~/...` display string as
/// the chip: `prune_orphan_chips` matches chips against the composer text, so
/// the chip must carry the same string the user sees. Expansion to `$HOME`
/// happens at prompt-build time, not in the chip.
#[test]
fn file_mention_accept_home_path_records_display_chip() {
    let mut app = create_test_app();
    app.is_remote = false;
    app.input = "@~/no".to_string();
    app.cursor_pos = app.input.len();

    seed_suggestions(
        &mut app,
        vec![("~/notes/todo.txt".to_string(), "home")],
    );

    let accepted = app.accept_selected_command_suggestion();
    assert!(accepted, "accepting a ~/ path should succeed");
    assert!(
        app.file_chips
            .iter()
            .any(|c| c.to_string_lossy() == "~/notes/todo.txt"),
        "chip must store the literal ~/ display string"
    );
    assert!(
        !app.input.contains('@'),
        "the @ sign must be dropped after accepting a completion"
    );
}

/// Accepting an `@/` suggestion records the absolute path as the chip.
#[test]
fn file_mention_accept_absolute_path_records_chip() {
    let mut app = create_test_app();
    app.is_remote = false;
    app.input = "@/et".to_string();
    app.cursor_pos = app.input.len();

    seed_suggestions(
        &mut app,
        vec![("/etc/hosts".to_string(), "absolute")],
    );

    let accepted = app.accept_selected_command_suggestion();
    assert!(accepted, "accepting an absolute path should succeed");
    assert!(
        app.file_chips.iter().any(|c| c.to_string_lossy() == "/etc/hosts"),
        "chip must store the absolute path"
    );
}

/// Sentinel rows (section headers, building-index hint) must stay inert even
/// when they are the only rows in an `@~`/`@/` popover: accepting them would
/// insert junk text as a chip and rewrite the composer input.
#[test]
fn file_mention_home_query_sentinel_rows_stay_inert() {
    let mut app = create_test_app();
    app.is_remote = false;
    app.input = "@~".to_string();
    app.cursor_pos = app.input.len();

    seed_suggestions(
        &mut app,
        vec![
            (String::new(), "── Recent ──"),
            ("⏳ Building file index...".to_string(), "hint"),
        ],
    );

    let input_before = app.input.clone();
    let chips_before = app.file_chips.clone();

    // Selection on the header row: accept must skip to a real row; with none
    // present it must reject instead of inserting header text.
    app.command_suggestion_selected = 0;
    assert!(
        !app.accept_selected_command_suggestion(),
        "header-only popover must not accept anything"
    );
    // Selection on the hint row: explicitly inert.
    app.command_suggestion_selected = 1;
    assert!(
        !app.accept_selected_command_suggestion(),
        "the building-index hint must never be accepted"
    );
    assert_eq!(app.input, input_before, "input must stay untouched");
    assert_eq!(app.file_chips, chips_before, "no chip may be recorded");
}

/// Tab on a filesystem (`@/`) query completes to the common prefix first,
/// then cycles the candidate rows. Real rows come from a tempdir so the
/// popover reflects genuine FS entries, not seeded state.
#[test]
fn file_mention_absolute_query_tab_completes_and_cycles() {
    let rt = tokio::runtime::Runtime::new().expect("test runtime");
    let _guard = rt.enter();

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("alpha.txt"), "a").unwrap();
    std::fs::write(dir.path().join("alphabet.md"), "b").unwrap();
    let prefix = format!("@{}/al", dir.path().to_string_lossy());

    let mut app = create_test_app();
    app.is_remote = false;
    app.session.working_dir = Some(env!("CARGO_MANIFEST_DIR").to_string());
    app.input = prefix.clone();
    app.cursor_pos = app.input.len();

    // Wait for the suggestions to materialize (the FS query needs no index,
    // but the popover pipeline goes through the same memoized path).
    use std::time::{Duration, Instant};
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        app.advance_command_suggestions_epoch();
        let suggestions = app.command_suggestions();
        let real_rows = suggestions
            .iter()
            .filter(|(cmd, _)| !cmd.is_empty() && !cmd.starts_with('\u{23f3}'))
            .count();
        if real_rows >= 2 || Instant::now() > deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    crate::tui::app::input::handle_basic_key(
        &mut app,
        crossterm::event::KeyCode::Tab,
    );

    // First Tab completes the longest common prefix: "al" grows to "alpha".
    assert!(
        app.input.contains("alpha"),
        "first Tab must complete the common prefix, got: {}",
        app.input
    );
    assert!(
        app.input.starts_with('@'),
        "tab_complete keeps the @ while cycling, got: {}",
        app.input
    );

    // Cycling continues from the completed state.
    crate::tui::app::input::handle_basic_key(
        &mut app,
        crossterm::event::KeyCode::Tab,
    );
    let cycled = app.input.clone();
    assert!(
        cycled.ends_with(".txt") || cycled.ends_with(".md"),
        "cycling must replace the input with a full candidate, got: {}",
        cycled
    );
}

/// Tab on a popover containing only sentinel rows must do nothing: the
/// cycling math would divide by zero otherwise. Guards the empty-paths
/// early return in the tab-completion handler.
#[test]
fn file_mention_sentinel_only_popover_tab_is_inert() {
    let rt = tokio::runtime::Runtime::new().expect("test runtime");
    let _guard = rt.enter();

    let mut app = create_test_app();
    app.is_remote = false;
    app.session.working_dir = Some(env!("CARGO_MANIFEST_DIR").to_string());
    app.input = "@~/.jcode-nonexistent-dir-xyz/".to_string();
    app.cursor_pos = app.input.len();
    app.advance_command_suggestions_epoch();

    let before = app.input.clone();
    crate::tui::app::input::handle_basic_key(
        &mut app,
        crossterm::event::KeyCode::Tab,
    );

    // No real candidate rows exist: input must be untouched and no panic.
    assert_eq!(app.input, before);
    assert_eq!(app.command_suggestion_selected, 0);
}

/// Backspacing over a file chip removes it from `file_chips` (via
/// `prune_orphan_chips`), but Ctrl+Z must restore both the text and the chip:
/// the send-time expansion reads `file_chips`, so losing the chip on undo
/// silently drops the file attachment from the prompt.
#[test]
fn undo_restores_file_chip_after_backspace() {
    let mut app = create_test_app();
    app.is_remote = false;
    app.input = "see src/main.rs".to_string();
    app.cursor_pos = app.input.len();
    app.file_chips.push(std::path::PathBuf::from("src/main.rs"));

    // Backspace once: the chip path vanishes from the input, so the chip is
    // pruned along with the removed character.
    crate::tui::app::input::handle_basic_key(&mut app, crossterm::event::KeyCode::Backspace);
    assert!(
        !app.input.contains("src/main.rs"),
        "backspace should remove the chip text"
    );
    assert!(
        app.file_chips.is_empty(),
        "chip should be pruned once its text is gone"
    );

    // Undo must bring back the chip together with the text.
    app.undo_input_change();
    assert_eq!(app.input, "see src/main.rs", "undo must restore the text");
    assert!(
        app.file_chips.iter().any(|c| c.to_string_lossy() == "src/main.rs"),
        "undo must restore the file chip so the attachment survives"
    );
}
