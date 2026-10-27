// Lifecycle regressions for the transcript-independent, session-local journal.

#[test]
fn test_prompt_history_survives_replaced_and_cleared_transcript() {
    let mut app = create_test_app();
    app.replace_display_messages(vec![
        DisplayMessage::user("before checkpoint"),
        DisplayMessage::user("removed by restore"),
    ]);
    app.replace_display_messages(vec![DisplayMessage::user("before checkpoint")]);
    assert_eq!(
        app.merged_prompt_history(),
        vec!["before checkpoint", "removed by restore"]
    );
    app.clear_display_messages();
    assert_eq!(
        app.merged_prompt_history(),
        vec!["before checkpoint", "removed by restore"]
    );
}

#[test]
fn test_prompt_history_repeat_remains_newest_after_restore() {
    let mut app = create_test_app();
    app.replace_display_messages(vec![
        DisplayMessage::user("first"),
        DisplayMessage::user("second"),
    ]);
    app.record_prompt_history("first");
    app.replace_display_messages(vec![DisplayMessage::user("first")]);
    assert_eq!(app.merged_prompt_history(), vec!["second", "first"]);
    // Local checkpoint restoration re-renders messages through push, not replace.
    app.clear_display_messages();
    app.push_display_message(DisplayMessage::user("second"));
    assert_eq!(app.merged_prompt_history(), vec!["second", "first"]);
}

#[test]
fn test_prompt_history_switch_is_immediate_before_history_arrives() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.set_remote_prompt_history_session("recall-session-a");
    app.replace_display_messages(vec![DisplayMessage::user("only session a")]);
    app.open_prompt_history_search();
    app.handle_prompt_history_search_key(KeyCode::Char('a'), KeyModifiers::empty());
    assert!(app.prompt_history_search.is_some());

    // A SessionId arrives first, while A's transcript is still visible.
    app.set_remote_prompt_history_session("recall-session-b");
    assert!(app.prompt_history_search.is_none());
    assert!(app.history_draft.is_none());
    assert!(app.merged_prompt_history().is_empty());
    app.replace_display_messages(vec![DisplayMessage::user("only session b")]);
    assert_eq!(app.merged_prompt_history(), vec!["only session b"]);

    // Returning to A restores its own journal, never B's old display.
    app.set_remote_prompt_history_session("recall-session-a");
    assert_eq!(app.merged_prompt_history(), vec!["only session a"]);
    app.replace_display_messages(Vec::new());
    assert_eq!(app.merged_prompt_history(), vec!["only session a"]);
}

#[test]
fn test_prompt_history_pending_switch_hides_source_until_authoritative_reply() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.set_remote_prompt_history_session("recall-pending-source");
    app.replace_display_messages(vec![DisplayMessage::user("source only")]);
    app.suspend_prompt_history_for_switch();
    assert!(app.merged_prompt_history().is_empty());
    app.open_prompt_history_search();
    app.handle_prompt_history_search_key(KeyCode::Char('s'), KeyModifiers::empty());
    assert!(
        app.prompt_history_search
            .as_ref()
            .unwrap()
            .matches
            .is_empty()
    );
    app.cancel_prompt_history_switch();
    assert_eq!(app.merged_prompt_history(), vec!["source only"]);
    app.suspend_prompt_history_for_switch();
    app.set_remote_prompt_history_session("recall-pending-target");
    assert!(app.merged_prompt_history().is_empty());
}

#[test]
fn test_prompt_history_same_session_reconnect_keeps_removed_prompts() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.resume_session_id = Some("recall-resumed-session".to_string());
    app.set_remote_prompt_history_session("recall-resumed-session");
    app.replace_display_messages(vec![
        DisplayMessage::user("kept"),
        DisplayMessage::user("rewound"),
    ]);
    app.replace_display_messages(vec![DisplayMessage::user("kept")]);
    app.set_remote_prompt_history_session("recall-resumed-session");
    app.replace_display_messages(vec![DisplayMessage::user("kept")]);
    assert_eq!(app.merged_prompt_history(), vec!["kept", "rewound"]);
}

#[test]
fn test_prompt_history_initial_server_id_adopts_early_startup_prompt() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.record_prompt_history("sent before subscribe history");
    app.set_remote_prompt_history_session("recall-server-assigned-session");
    assert_eq!(
        app.merged_prompt_history(),
        vec!["sent before subscribe history"]
    );
    app.set_remote_prompt_history_session("recall-another-session");
    assert!(app.merged_prompt_history().is_empty());
}

#[test]
fn test_prompt_history_local_new_session_starts_empty() {
    let mut app = create_test_app();
    app.push_display_message(DisplayMessage::user("old local session"));
    let old_session = app.session.id.clone();
    app.clear_display_messages();
    app.session.id = "recall-new-local-session".to_string();
    app.adopt_prompt_history_display();
    assert!(app.merged_prompt_history().is_empty());
    app.session.id = old_session;
    app.adopt_prompt_history_display();
    assert_eq!(app.merged_prompt_history(), vec!["old local session"]);
}

#[test]
fn test_prompt_history_fork_inherits_transcript_not_parent_removed_prompts() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.set_remote_prompt_history_session("recall-parent");
    app.replace_display_messages(vec![
        DisplayMessage::user("inherited by fork"),
        DisplayMessage::user("removed from parent"),
    ]);
    app.replace_display_messages(vec![DisplayMessage::user("inherited by fork")]);
    app.set_remote_prompt_history_session("recall-child");
    app.replace_display_messages(vec![DisplayMessage::user("inherited by fork")]);
    assert_eq!(app.merged_prompt_history(), vec!["inherited by fork"]);
    app.set_remote_prompt_history_session("recall-parent");
    assert_eq!(
        app.merged_prompt_history(),
        vec!["inherited by fork", "removed from parent"]
    );
}

#[test]
fn test_prompt_history_paths_are_scoped_and_not_path_traversal() {
    use crate::tui::app::prompt_history::history_file_path;
    let a = history_file_path("recall-a").unwrap();
    let b = history_file_path("recall-b").unwrap();
    let untrusted = history_file_path("../../outside").unwrap();
    assert_ne!(a, b);
    assert_eq!(a.parent(), b.parent());
    assert_eq!(a.parent(), untrusted.parent());
    assert_ne!(a.file_name().unwrap(), "prompt-history.jsonl");
}

#[test]
fn test_prompt_history_multiline_and_existing_trim_semantics_survive_restore() {
    let mut app = create_test_app();
    app.record_prompt_history("  first line\n  second line 日本語\n");
    app.clear_display_messages();
    assert_eq!(
        app.merged_prompt_history(),
        vec!["first line\n  second line 日本語"]
    );
}

fn assert_large_prompt_history_survives_restore_and_disk(prompt: String) {
    use crate::tui::app::prompt_history::{append_to_path, load_from_path};

    let mut app = create_test_app();
    app.push_display_message(DisplayMessage::user("before large prompt"));
    app.record_prompt_history(&prompt);
    app.push_display_message(DisplayMessage::user(&prompt));
    app.replace_display_messages(vec![DisplayMessage::user("before large prompt")]);
    assert_eq!(
        app.merged_prompt_history().last().map(String::as_str),
        Some(prompt.as_str())
    );

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("large-prompt-history.jsonl");
    for entry in app.merged_prompt_history() {
        append_to_path(&path, &entry);
    }
    let loaded = load_from_path(&path);
    assert_eq!(loaded.last().map(String::as_str), Some(prompt.as_str()));

    let mut reopened = create_test_app();
    reopened.session.id = app.session.id.clone();
    reopened.persisted_prompt_history = Some(loaded);
    reopened.replace_display_messages(vec![DisplayMessage::user("before large prompt")]);
    assert_eq!(
        reopened.merged_prompt_history().last().map(String::as_str),
        Some(prompt.as_str())
    );
}

fn unicode_multiline_prompt_with_bytes(bytes: usize) -> String {
    let line = "日本語🦀\n  λ = 'preserve pasted text'\n";
    let mut prompt = line.repeat(bytes / line.len());
    prompt.push_str(&"x".repeat(bytes - prompt.len()));
    assert_eq!(prompt.len(), bytes);
    prompt
}

#[test]
fn test_prompt_history_64kb_unicode_multiline_survives_restore_and_disk() {
    assert_large_prompt_history_survives_restore_and_disk(unicode_multiline_prompt_with_bytes(
        64 * 1024,
    ));
}

#[test]
fn test_prompt_history_submission_size_boundaries_survive_restore_and_disk() {
    let local_submit_limit = crate::tui::app::input::MAX_SUBMITTED_TEXT_BYTES;
    // Remote submissions can exceed the local input limit. Recall must not
    // introduce its own smaller cutoff, including one byte past that boundary.
    for bytes in [
        local_submit_limit - 1,
        local_submit_limit,
        local_submit_limit + 1,
    ] {
        assert_large_prompt_history_survives_restore_and_disk(unicode_multiline_prompt_with_bytes(
            bytes,
        ));
    }
}

#[test]
fn test_prompt_history_expanded_slash_display_keeps_submission_order_after_rewind() {
    let mut app = create_test_app();
    app.push_display_message(DisplayMessage::user("/plugin:echo ARG"));
    // Slash expansion updates the already-inserted display row, rather than
    // submitting another user prompt through push_display_message.
    app.display_messages.last_mut().unwrap().content = "ARG".to_string();
    app.record_prompt_history("FIRST");
    app.push_display_message(DisplayMessage::user("FIRST"));
    app.record_prompt_history("SECOND");
    app.push_display_message(DisplayMessage::user("SECOND"));
    app.replace_display_messages(vec![DisplayMessage::user("ARG")]);
    assert_eq!(app.merged_prompt_history(), vec!["ARG", "FIRST", "SECOND"]);

    // An intentional repeat still moves to the newest slot, even after rewind.
    app.record_prompt_history("FIRST");
    app.replace_display_messages(vec![DisplayMessage::user("ARG")]);
    assert_eq!(app.merged_prompt_history(), vec!["ARG", "SECOND", "FIRST"]);
}

#[test]
fn test_prompt_history_expanded_source_display_cannot_cross_pending_session_switch() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.set_remote_prompt_history_session("expanded-source");
    app.push_display_message(DisplayMessage::user("/plugin:echo SOURCE_ARG"));
    app.display_messages.last_mut().unwrap().content = "SOURCE_ARG".to_string();
    app.suspend_prompt_history_for_switch();
    assert!(app.merged_prompt_history().is_empty());
    app.set_remote_prompt_history_session("expanded-target");
    app.record_prompt_history("TARGET_FIRST");
    assert_eq!(app.merged_prompt_history(), vec!["TARGET_FIRST"]);
    app.replace_display_messages(vec![DisplayMessage::user("TARGET_FIRST")]);
    app.set_remote_prompt_history_session("expanded-source");
    assert_eq!(app.merged_prompt_history(), vec!["SOURCE_ARG"]);
}

#[test]
fn test_prompt_history_replay_only_slash_expansion_backfills_and_survives_reopen() {
    use crate::tui::app::prompt_history::{
        append_snapshot_to_path, append_to_path, load_from_path,
    };

    let mut app = create_test_app();
    // Real keyboard input displays the raw slash command. Only server replay
    // reconstructs its expanded ARG text, after later prompts were submitted.
    app.record_prompt_history("/plugin:echo ARG");
    app.push_display_message(DisplayMessage::user("/plugin:echo ARG"));
    for prompt in ["FIRST", "SECOND"] {
        app.record_prompt_history(prompt);
        app.push_display_message(DisplayMessage::user(prompt));
    }
    assert_eq!(app.merged_prompt_history(), vec!["FIRST", "SECOND"]);
    app.replace_display_messages(vec![DisplayMessage::user("ARG")]);
    assert_eq!(app.merged_prompt_history(), vec!["ARG", "FIRST", "SECOND"]);
    // Reapplying the same restored snapshot must not move ARG to the tail.
    app.replace_display_messages(vec![DisplayMessage::user("ARG")]);
    assert_eq!(app.merged_prompt_history(), vec!["ARG", "FIRST", "SECOND"]);

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("replay-backfill.jsonl");
    append_to_path(&path, "FIRST");
    append_to_path(&path, "SECOND");
    append_snapshot_to_path(&path, &app.merged_prompt_history());
    assert_eq!(load_from_path(&path), vec!["ARG", "FIRST", "SECOND"]);
    let mut reopened = create_test_app();
    reopened.persisted_prompt_history = Some(load_from_path(&path));
    reopened.replace_display_messages(vec![DisplayMessage::user("ARG")]);
    assert_eq!(
        reopened.merged_prompt_history(),
        vec!["ARG", "FIRST", "SECOND"]
    );
}

#[test]
fn test_prompt_history_replay_backfill_uses_before_between_and_after_anchors() {
    let mut app = create_test_app();
    for prompt in ["A", "B", "C", "NEWEST_SUBMISSION"] {
        app.record_prompt_history(prompt);
    }
    app.replace_display_messages(
        ["before A", "A", "between A and B", "B", "C", "after C"]
            .into_iter()
            .map(DisplayMessage::user)
            .collect(),
    );
    assert_eq!(
        app.merged_prompt_history(),
        vec![
            "before A",
            "A",
            "between A and B",
            "B",
            "C",
            "after C",
            "NEWEST_SUBMISSION"
        ]
    );
    // An unseen tail after the newest known anchor really is newer replay.
    app.replace_display_messages(
        ["NEWEST_SUBMISSION", "new remote tail"]
            .into_iter()
            .map(DisplayMessage::user)
            .collect(),
    );
    assert_eq!(
        app.merged_prompt_history().last().unwrap(),
        "new remote tail"
    );
}

#[test]
fn test_prompt_history_replay_backfill_preserves_known_repeat_recency_and_dedupes() {
    let mut app = create_test_app();
    for prompt in ["FIRST", "SECOND", "FIRST"] {
        app.record_prompt_history(prompt);
    }
    app.replace_display_messages(
        ["ARG", "ARG", "SECOND", "FIRST"]
            .into_iter()
            .map(DisplayMessage::user)
            .collect(),
    );
    assert_eq!(app.merged_prompt_history(), vec!["ARG", "SECOND", "FIRST"]);
    app.record_prompt_history("SECOND");
    assert_eq!(app.merged_prompt_history(), vec!["ARG", "FIRST", "SECOND"]);
}

#[test]
fn test_prompt_history_older_backfill_cannot_evict_newest_at_entry_limit() {
    let mut app = create_test_app();
    let history: Vec<String> = (0..crate::tui::app::prompt_history::MAX_PERSISTED_PROMPTS)
        .map(|index| format!("submitted-{index}"))
        .collect();
    app.persisted_prompt_history = Some(history.clone());
    app.replace_display_messages(vec![DisplayMessage::user("older reconstructed text")]);
    assert_eq!(app.merged_prompt_history(), history);
}
