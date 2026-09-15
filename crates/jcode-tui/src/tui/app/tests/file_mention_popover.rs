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
