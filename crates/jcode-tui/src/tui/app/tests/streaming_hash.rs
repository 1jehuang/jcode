fn assert_streaming_hash_matches_content(app: &App) {
    assert_eq!(
        crate::tui::TuiState::streaming_text_hash(app),
        crate::tui::ui::hash_text_for_cache(app.streaming_text())
    );
}

#[test]
fn streaming_hash_unchanged_frames_hash_once() {
    let mut app = create_test_app();
    let text = "streamed text 漢字\n".repeat(16_384);
    app.replace_streaming_text(text.clone());
    let before = app.streaming.streaming_hash_work.get();
    for _ in 0..120 {
        assert_streaming_hash_matches_content(&app);
    }
    let after = app.streaming.streaming_hash_work.get();
    assert_eq!(after.0 - before.0, 1, "119 repeated hashes eliminated");
    assert_eq!(after.1 - before.1, text.len(), "only one buffer scanned");
    // Baseline was 120 scans, so 119 * text.len() bytes are avoided.
    assert_eq!(120 * text.len() - (after.1 - before.1), 119 * text.len());
    eprintln!(
        "unchanged streaming hash over120reads: scans120→{}, bytes{}→{}",
        after.0 - before.0,
        120 * text.len(),
        after.1 - before.1,
    );
}

#[test]
fn streaming_hash_invalidates_all_string_mutations_and_clone_reset() {
    let mut progress = StreamingProgress::default();
    let check = |progress: &StreamingProgress| {
        assert_eq!(
            progress.text_hash(),
            crate::tui::ui::hash_text_for_cache(&progress.streaming_text)
        );
    };
    check(&progress);
    progress.streaming_text_mut().push_str("abcd漢字");
    check(&progress);
    progress.streaming_text_mut().truncate(4);
    check(&progress);
    *progress.streaming_text_mut() = "wxyz".into();
    check(&progress);
    let mut cloned = progress.clone();
    *cloned.streaming_text_mut() = "same".into();
    check(&cloned);
    check(&progress);
    assert_ne!(progress.text_hash(), cloned.text_hash());
    assert_eq!(progress.streaming_text_mut().split_off(2), "yz");
    check(&progress);
    progress.streaming_text_mut().pop();
    check(&progress);
    progress.streaming_text_mut().clear();
    check(&progress);
    progress = StreamingProgress::default();
    check(&progress);
}

#[test]
fn streaming_hash_app_lifecycle_invalidates_same_length_replace_take_clear() {
    let mut app = create_test_app();
    assert_streaming_hash_matches_content(&app);
    app.append_streaming_text("abcd");
    assert_streaming_hash_matches_content(&app);
    let previous = crate::tui::TuiState::streaming_text_hash(&app);
    app.replace_streaming_text("wxyz".into());
    assert_streaming_hash_matches_content(&app);
    assert_ne!(previous, crate::tui::TuiState::streaming_text_hash(&app));
    assert_eq!(app.take_streaming_text(), "wxyz");
    assert_streaming_hash_matches_content(&app);
    app.append_streaming_text("reset");
    assert_streaming_hash_matches_content(&app);
    app.clear_streaming_render_state();
    assert_streaming_hash_matches_content(&app);
}

#[test]
fn streaming_hash_reasoning_tail_truncate_commit_and_split_off() {
    with_reasoning_current_home(|| {
        let mut app = create_test_app();
        app.append_streaming_text("intro");
        assert_streaming_hash_matches_content(&app);
        app.open_reasoning_region();
        assert_streaming_hash_matches_content(&app);
        app.append_reasoning_text("漢字 partial");
        assert_streaming_hash_matches_content(&app);
        app.strip_reasoning_partial_tail();
        assert_streaming_hash_matches_content(&app);
        app.append_reasoning_text("more\nnext");
        assert_streaming_hash_matches_content(&app);
        app.close_reasoning_region(None);
        assert_streaming_hash_matches_content(&app);
        assert!(
            app.streaming_text().is_empty(),
            "current reasoning split and preceding answer committed"
        );
    });
}
