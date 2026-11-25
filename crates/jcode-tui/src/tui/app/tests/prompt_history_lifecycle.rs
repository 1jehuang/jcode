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
    use crate::tui::app::prompt_history::{append_snapshot_to_path, append_to_path, load_from_path};

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
    assert!(append_snapshot_to_path(&path, &app.merged_prompt_history()).is_some());
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

#[test]
fn test_prompt_history_submission_during_switch_belongs_to_target_session() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.set_remote_prompt_history_session("pending-recall-source");
    app.record_prompt_history("source prompt");
    app.suspend_prompt_history_for_request(77);
    app.record_prompt_history("target prompt sent before session acknowledgement");
    assert!(app.merged_prompt_history().is_empty());
    app.set_remote_prompt_history_session("pending-recall-target");
    app.replace_display_messages(Vec::new());
    assert_eq!(
        app.merged_prompt_history(),
        vec!["target prompt sent before session acknowledgement"]
    );
    app.set_remote_prompt_history_session("pending-recall-source");
    assert_eq!(app.merged_prompt_history(), vec!["source prompt"]);
}

#[test]
fn test_prompt_history_pending_switch_ignores_unrelated_server_error() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    app.is_remote = true;
    app.set_remote_prompt_history_session("pending-error-source");
    app.record_prompt_history("source prompt");
    app.suspend_prompt_history_for_request(77);
    let error = |id| {
        serde_json::from_value(serde_json::json!({
            "type": "error", "id": id, "message": "control request failed"
        }))
        .unwrap()
    };
    app.handle_server_event(error(76), &mut remote);
    assert!(app.merged_prompt_history().is_empty());
    app.handle_server_event(error(77), &mut remote);
    assert_eq!(app.merged_prompt_history(), vec!["source prompt"]);
}

#[test]
fn test_prompt_history_failed_switch_keeps_submission_in_source_journal() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.set_remote_prompt_history_session("failed-recall-source");
    app.record_prompt_history("source prompt");
    app.suspend_prompt_history_for_request(77);
    app.record_prompt_history("prompt after rejected switch");
    app.push_display_message(DisplayMessage::user("prompt after rejected switch"));
    app.fail_prompt_history_switch(77);
    assert_eq!(
        app.merged_prompt_history(),
        vec!["source prompt", "prompt after rejected switch"]
    );
}

#[test]
fn test_prompt_history_serializes_switches_until_acknowledged() {
    use tokio::io::AsyncBufReadExt;
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        let mut reader = tokio::io::BufReader::new(remote.take_dummy_peer().unwrap());
        app.is_remote = true;
        app.set_remote_prompt_history_session("serialized-recall-source");
        app.record_prompt_history("source prompt");
        app.resume_session(&mut remote, "serialized-recall-target")
            .await
            .unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        let id = match serde_json::from_str::<crate::protocol::Request>(&line).unwrap() {
            crate::protocol::Request::ResumeSession { id, .. } => id,
            other => panic!("unexpected request: {other:?}"),
        };
        assert!(
            app.resume_session(&mut remote, "overlapping-recall-target")
                .await
                .is_err()
        );
        assert!(app.clear_session(&mut remote).await.is_ok());
        assert!(app.merged_prompt_history().is_empty());
        line.clear();
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(10),
                reader.read_line(&mut line)
            )
            .await
            .is_err()
        );
        app.fail_prompt_history_switch(id);
        assert_eq!(app.merged_prompt_history(), vec!["source prompt"]);
    });
}

#[test]
fn test_prompt_history_remote_escape_clears_recalled_draft_without_stopping_turn() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    app.auto_poke_incomplete_todos = true;
    app.input = "recalled prompt".to_string();
    app.cursor_pos = app.input.len();
    rt.block_on(app.handle_remote_key(KeyCode::Esc, KeyModifiers::empty(), &mut remote))
        .unwrap();
    assert!(app.is_processing);
    assert!(app.input.is_empty());
    assert!(app.auto_poke_incomplete_todos);
    assert!(app.remote_interrupt_ack_deadline.is_none());
}

#[test]
fn test_prompt_history_local_escape_clears_recalled_draft_without_stopping_turn() {
    let mut app = create_test_app();
    app.is_processing = true;
    app.auto_poke_incomplete_todos = true;
    app.input = "recalled prompt".to_string();
    app.cursor_pos = app.input.len();
    app.handle_key(KeyCode::Esc, KeyModifiers::empty()).unwrap();
    assert!(app.is_processing);
    assert!(!app.cancel_requested);
    assert!(app.input.is_empty());
    assert!(app.auto_poke_incomplete_todos);
}

#[test]
fn test_prompt_history_switch_buffer_excludes_login_responses() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.set_remote_prompt_history_session("login-buffer-source");
    app.record_prompt_history("source prompt");
    app.suspend_prompt_history_for_request(77);
    app.pending_login = Some(PendingLogin::Gemini {
        verifier: "test".into(),
        expected_state: None,
        redirect_uri: "http://localhost".into(),
    });
    app.record_prompt_history("login response");
    app.pending_login = None;
    app.fail_prompt_history_switch(77);
    assert_eq!(app.merged_prompt_history(), vec!["source prompt"]);
}

#[test]
fn test_prompt_history_repeated_workspace_key_waits_without_exiting() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        app.is_remote = true;
        app.set_remote_prompt_history_session("navigation-source");
        app.workspace_client.enable(
            Some("navigation-source"),
            &[
                "navigation-source".into(),
                "navigation-target".into(),
                "navigation-third".into(),
            ],
        );
        app.workspace_navigation_keys.right = vec![crate::tui::keybind::KeyBinding {
            code: KeyCode::Right,
            modifiers: KeyModifiers::ALT,
        }];
        app.handle_remote_key(KeyCode::Right, KeyModifiers::ALT, &mut remote)
            .await
            .unwrap();
        assert!(
            app.handle_remote_key(KeyCode::Right, KeyModifiers::ALT, &mut remote)
                .await
                .is_ok()
        );
        assert!(
            app.status_notice()
                .is_some_and(|notice| notice.contains("session switch"))
        );
        let rows = app
            .workspace_client
            .visible_rows(1, Some("navigation-source"), false);
        assert_eq!(rows[0].focused_index, Some(1));
    });
}

#[test]
fn test_prompt_history_clear_during_switch_waits_without_discarding_history() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        app.is_remote = true;
        app.set_remote_prompt_history_session("clear-source");
        app.replace_display_messages(vec![DisplayMessage::user("keep this transcript")]);
        app.resume_session(&mut remote, "clear-target")
            .await
            .unwrap();
        app.input = "/clear".into();
        app.cursor_pos = app.input.len();
        assert!(
            app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), &mut remote)
                .await
                .is_ok()
        );
        assert_eq!(app.input, "/clear");
        assert!(
            app.display_messages
                .iter()
                .any(|message| message.content == "keep this transcript")
        );
        assert!(
            app.status_notice()
                .is_some_and(|notice| notice.contains("session switch"))
        );
    });
}

#[test]
fn test_prompt_history_backfill_preserves_another_clients_newer_submission() {
    use crate::tui::app::prompt_history::{append_snapshot_to_path, append_to_path, load_from_path};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shared.jsonl");
    append_to_path(&path, "FIRST");
    append_to_path(&path, "SECOND");
    let mut stale = load_from_path(&path);
    append_to_path(&path, "NEW");
    stale.insert(0, "ARG".into());
    assert!(append_snapshot_to_path(&path, &stale).is_some());
    assert_eq!(load_from_path(&path), vec!["ARG", "FIRST", "SECOND", "NEW"]);
}

#[test]
fn test_prompt_history_concurrent_backfills_and_submissions_preserve_order_through_compaction() {
    use crate::tui::app::prompt_history::{
        MAX_PERSISTED_PROMPTS, append_snapshot_to_path, append_to_path, load_from_path,
    };
    let dir = tempfile::tempdir().unwrap();
    let path = std::sync::Arc::new(dir.path().join("concurrent.jsonl"));
    std::fs::write(
        path.as_ref(),
        "\"FIRST\"\n\"SECOND\"\n".repeat(MAX_PERSISTED_PROMPTS + 1),
    )
    .unwrap();
    let start = std::sync::Arc::new(std::sync::Barrier::new(2));
    let submissions = {
        let path = path.clone();
        let start = start.clone();
        std::thread::spawn(move || {
            start.wait();
            for i in 0..12 {
                append_to_path(&path, &format!("NEW-{i}"));
            }
        })
    };
    let backfills = {
        let path = path.clone();
        let start = start.clone();
        std::thread::spawn(move || {
            let stale = vec!["ARG".into(), "FIRST".into(), "SECOND".into()];
            start.wait();
            for _ in 0..12 {
                assert!(append_snapshot_to_path(&path, &stale).is_some());
            }
        })
    };
    submissions.join().unwrap();
    backfills.join().unwrap();
    let mut expected = vec!["ARG".to_string(), "FIRST".into(), "SECOND".into()];
    expected.extend((0..12).map(|i| format!("NEW-{i}")));
    assert_eq!(load_from_path(&path), expected);
}

#[test]
fn test_prompt_history_backfill_lock_failure_preserves_existing_journal() {
    use crate::tui::app::prompt_history::{append_snapshot_to_path, append_to_path, load_from_path};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("locked.jsonl");
    append_to_path(&path, "NEW");
    std::fs::remove_file(path.with_extension("lock")).unwrap();
    std::fs::create_dir(path.with_extension("lock")).unwrap();
    assert!(append_snapshot_to_path(&path, &["OLD".into()]).is_none());
    assert_eq!(load_from_path(&path), vec!["NEW"]);
}
