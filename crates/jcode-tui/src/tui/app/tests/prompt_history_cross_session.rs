// Cross-session prompt history + Ctrl+R reverse search tests
// ====================================================================

#[test]
fn test_prompt_history_records_only_new_prompts_and_moves_repeats_to_front() {
    let mut app = create_test_app();

    app.record_prompt_history("first");
    app.record_prompt_history("second");
    app.record_prompt_history("first"); // repeat: moves to front, no duplicate

    let history = app.persisted_prompt_history.clone().unwrap();
    assert_eq!(history, vec!["second".to_string(), "first".to_string()]);
}

#[test]
fn test_prompt_history_skips_slash_shell_and_empty_inputs() {
    let mut app = create_test_app();

    app.record_prompt_history("/help");
    app.record_prompt_history("!ls -la");
    app.record_prompt_history("   ");
    app.record_prompt_history("");

    assert_eq!(
        app.persisted_prompt_history.as_deref().unwrap_or_default(),
        &[] as &[String]
    );
}

#[test]
fn test_prompt_history_skips_pending_login_input() {
    let mut app = create_test_app();
    app.pending_login = Some(PendingLogin::Gemini {
        verifier: "v".to_string(),
        expected_state: None,
        redirect_uri: "http://localhost".to_string(),
    });

    app.record_prompt_history("sk-secret-value");

    assert_eq!(
        app.persisted_prompt_history.as_deref().unwrap_or_default(),
        &[] as &[String]
    );
}

#[test]
fn test_up_arrow_recalls_prompts_from_previous_sessions() {
    let mut app = create_test_app();
    // Persisted history from earlier sessions, oldest first.
    app.persisted_prompt_history = Some(vec![
        "old session prompt".to_string(),
        "newer old prompt".to_string(),
    ]);
    // Current session has one prompt.
    app.display_messages = vec![DisplayMessage::user("current prompt")];
    app.bump_display_messages_version();

    app.handle_key(KeyCode::Up, KeyModifiers::empty()).unwrap();
    assert_eq!(app.input, "current prompt");

    app.handle_key(KeyCode::Up, KeyModifiers::empty()).unwrap();
    assert_eq!(app.input, "newer old prompt");

    app.handle_key(KeyCode::Up, KeyModifiers::empty()).unwrap();
    assert_eq!(app.input, "old session prompt");

    app.handle_key(KeyCode::Down, KeyModifiers::empty())
        .unwrap();
    assert_eq!(app.input, "newer old prompt");
}

#[test]
fn test_merged_prompt_history_dedupes_across_sessions() {
    let mut app = create_test_app();
    app.persisted_prompt_history =
        Some(vec!["shared prompt".to_string(), "unique old".to_string()]);
    app.display_messages = vec![DisplayMessage::user("shared prompt")];
    app.bump_display_messages_version();

    let merged = app.merged_prompt_history();
    assert_eq!(
        merged,
        vec!["unique old".to_string(), "shared prompt".to_string()]
    );
}

#[test]
fn test_ctrl_r_opens_history_search_and_enter_inserts_selection() {
    let mut app = create_test_app();
    app.persisted_prompt_history = Some(vec![
        "fix the login bug".to_string(),
        "write more tests".to_string(),
    ]);

    app.handle_key(KeyCode::Char('r'), KeyModifiers::CONTROL)
        .unwrap();
    assert!(app.prompt_history_search.is_some());
    // Newest first with an empty query.
    assert_eq!(
        app.prompt_history_search.as_ref().unwrap().matches[0],
        "write more tests"
    );

    // Type a query that matches only the older prompt.
    for c in "login".chars() {
        app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
            .unwrap();
    }
    let state = app.prompt_history_search.as_ref().unwrap();
    assert_eq!(state.matches, vec!["fix the login bug".to_string()]);

    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();
    assert!(app.prompt_history_search.is_none());
    assert_eq!(app.input, "fix the login bug");
    assert_eq!(app.cursor_pos, app.input.len());
}

#[test]
fn test_history_search_esc_cancels_without_touching_input() {
    let mut app = create_test_app();
    app.persisted_prompt_history = Some(vec!["some prompt".to_string()]);
    app.input = "draft".to_string();
    app.cursor_pos = app.input.len();

    app.handle_key(KeyCode::Char('r'), KeyModifiers::CONTROL)
        .unwrap();
    assert!(app.prompt_history_search.is_some());

    app.handle_key(KeyCode::Esc, KeyModifiers::empty()).unwrap();
    assert!(app.prompt_history_search.is_none());
    assert_eq!(app.input, "draft");
}

#[test]
fn test_history_search_up_down_moves_selection() {
    let mut app = create_test_app();
    app.persisted_prompt_history = Some(vec![
        "alpha".to_string(),
        "beta".to_string(),
        "gamma".to_string(),
    ]);

    app.handle_key(KeyCode::Char('r'), KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(app.prompt_history_search.as_ref().unwrap().selected, 0);

    app.handle_key(KeyCode::Up, KeyModifiers::empty()).unwrap();
    assert_eq!(app.prompt_history_search.as_ref().unwrap().selected, 1);

    // Ctrl+R again also steps older (readline muscle memory).
    app.handle_key(KeyCode::Char('r'), KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(app.prompt_history_search.as_ref().unwrap().selected, 2);

    app.handle_key(KeyCode::Down, KeyModifiers::empty())
        .unwrap();
    assert_eq!(app.prompt_history_search.as_ref().unwrap().selected, 1);

    // Enter inserts the selected (middle) match: newest-first order is
    // [gamma, beta, alpha], selected=1 is beta.
    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();
    assert_eq!(app.input, "beta");
}

#[test]
fn test_prompt_history_file_roundtrip_dedupes_and_caps() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("prompt-history.jsonl");

    crate::tui::app::prompt_history::append_to_path(&path, "one");
    crate::tui::app::prompt_history::append_to_path(&path, "two");
    crate::tui::app::prompt_history::append_to_path(&path, "one"); // repeat

    let loaded = crate::tui::app::prompt_history::load_from_path(&path);
    // Dedupe keeps the most recent occurrence.
    assert_eq!(loaded, vec!["two".to_string(), "one".to_string()]);

    // Multiline prompts survive the JSONL roundtrip.
    crate::tui::app::prompt_history::append_to_path(&path, "multi\nline\nprompt");
    let loaded = crate::tui::app::prompt_history::load_from_path(&path);
    assert_eq!(loaded.last().unwrap(), "multi\nline\nprompt");
}

#[test]
fn test_submit_input_records_prompt_history() {
    let mut app = create_test_app();
    app.input = "hello world".to_string();
    app.cursor_pos = app.input.len();

    app.submit_input();

    assert_eq!(
        app.persisted_prompt_history.as_deref().unwrap_or_default(),
        &["hello world".to_string()]
    );
}
