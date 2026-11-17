use super::*;

#[test]
fn fork_intent_snapshot_uses_first_genuine_request_and_keeps_inherited_search() {
    let mut messages = vec![fork_intent_message("original announcement needle")];
    for _ in 0..500 {
        messages.push(serde_json::json!({"role": "assistant", "content": "inherited reply"}));
    }
    messages.extend([
        fork_intent_message(FORK_INTENT_MARKER),
        fork_intent_message("<system-reminder>injected context</system-reminder>"),
        fork_intent_message("fix penguin session search"),
        fork_intent_message("later request"),
    ]);
    let data: SessionMessageSummaryData =
        serde_json::from_str(&serde_json::Value::Array(messages).to_string()).unwrap();
    assert_eq!(
        data.first_user_prompt.as_deref(),
        Some("fix penguin session search")
    );
    assert_eq!(data.visible_message_count, 503);
    let index = build_classified_search_index("fork", "penguin", "", None, None, &data.search_text);
    assert!(index.contains("original announcement needle"));
    assert!(index.contains("inherited reply"));
}

#[test]
fn fork_intent_journal_resets_across_entries_and_repeated_forks() {
    let temp = tempfile::tempdir().unwrap();
    let snapshot = temp.path().join("fork.json");
    std::fs::write(
        &snapshot,
        serde_json::json!({
            "created_at": "2026-11-13T00:00:00Z",
            "updated_at": "2026-11-13T00:00:00Z",
            "messages": [fork_intent_message("inherited opening")]
        })
        .to_string(),
    )
    .unwrap();
    let journal = session::session_journal_path_from_snapshot(&snapshot);
    let entry = |messages: Vec<serde_json::Value>| {
        serde_json::json!({
            "meta": {"updated_at": "2026-11-13T01:00:00Z"},
            "append_messages": messages
        })
        .to_string()
    };
    let mut lines = vec![entry(vec![fork_intent_message(FORK_INTENT_MARKER)])];
    std::fs::write(&journal, lines.join("\n")).unwrap();
    assert!(
        load_session_summary(&snapshot)
            .unwrap()
            .messages
            .first_user_prompt
            .is_none()
    );
    lines.push(entry(vec![fork_intent_message("first fork request")]));
    std::fs::write(&journal, lines.join("\n")).unwrap();
    assert_eq!(
        load_session_summary(&snapshot)
            .unwrap()
            .messages
            .first_user_prompt
            .as_deref(),
        Some("first fork request")
    );
    lines.push(entry(vec![
        fork_intent_message("inherited intervening request"),
        serde_json::json!({"role": "user", "content": FORK_INTENT_MARKER}),
        fork_intent_message("nested fork request"),
    ]));
    std::fs::write(&journal, lines.join("\n")).unwrap();
    let data = load_session_summary(&snapshot).unwrap().messages;
    assert_eq!(
        data.first_user_prompt.as_deref(),
        Some("nested fork request")
    );
    assert_eq!(data.user_message_count, 4);
    let index = build_classified_search_index("fork", "penguin", "", None, None, &data.search_text);
    assert!(index.contains("inherited opening"));
    assert!(index.contains("first fork request"));
}

#[test]
fn fork_intent_ignores_non_fork_reminders_and_assistant_quotes() {
    let data: SessionMessageSummaryData = serde_json::from_str(
        &serde_json::json!([
            fork_intent_message("original request"),
            fork_intent_message("<system-reminder>ordinary context</system-reminder>"),
            {"role": "assistant", "content": FORK_INTENT_MARKER},
            fork_intent_message("a later request")
        ])
        .to_string(),
    )
    .unwrap();
    assert_eq!(data.first_user_prompt.as_deref(), Some("original request"));
    assert!(!data.saw_fork_marker);
}

#[test]
fn preview_loading_preserves_live_tool_duration_metadata() {
    let mut session = Session::create_with_id("preview_metadata".into(), None, None);
    session.append_stored_message(crate::session::StoredMessage {
        id: "tool-result".into(),
        role: crate::message::Role::User,
        content: vec![crate::message::ContentBlock::ToolResult {
            tool_use_id: "tool-call".into(),
            content: "completed".into(),
            is_error: None,
        }]
        .into(),
        display_role: None,
        timestamp: None,
        tool_duration_ms: Some(1250),
        token_usage: None,
    });
    let live = crate::tui::display_messages_from_session(&session);
    let preview = build_messages_preview(&session);
    assert_eq!(preview.len(), live.len());
    let tool = preview.iter().find(|msg| msg.role == "tool").unwrap();
    assert_eq!(tool.duration_secs, Some(1.25));
    for (preview, live) in preview.iter().zip(live) {
        assert_eq!(preview.duration_secs, live.duration_secs);
        assert_eq!(preview.title, live.title);
    }
}

#[test]
fn cloud_sessions_are_loaded_outside_the_recent_window_and_grouped_by_host() {
    let _env_lock = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("temp dir");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let id = "session_cloudtest_1770000000000";
    let mut session = Session::create_with_id(id.to_string(), None, Some("Cloud work".into()));
    session.append_stored_message(crate::session::StoredMessage {
        id: "msg1".into(),
        role: crate::message::Role::User,
        content: vec![crate::message::ContentBlock::Text {
            text: "Continue my cloud work".into(),
            cache_control: None,
        }]
        .into(),
        display_role: None,
        timestamp: None,
        tool_duration_ms: None,
        token_usage: None,
    });
    session.save().expect("save snapshot");
    let mut lease = storage::SessionLease {
        session_id: id.into(),
        epoch: 1,
        away_host: Some("dev-cloud".into()),
        repo_root: Some("/workspace/project".into()),
        last_sync_snapshot: None,
        last_sync_head: None,
        updated_at: chrono::Utc::now().to_rfc3339(),
    };
    storage::write_session_lease(&lease).expect("write lease");
    let catchup = crate::catchup::CatchupSeenSnapshot::load();
    let mut sessions = Vec::new();
    include_cloud_sessions(&temp.path().join("sessions"), &catchup, &mut sessions);
    assert_eq!(
        sessions.len(),
        1,
        "cloud snapshot is added even without recent candidates"
    );
    assert_eq!(sessions[0].server_name.as_deref(), Some("cloud: dev-cloud"));
    assert!(sessions[0].search_index.contains("cloud dev-cloud"));
    include_cloud_sessions(&temp.path().join("sessions"), &catchup, &mut sessions);
    assert_eq!(sessions.len(), 1, "existing snapshots are not duplicated");

    invalidate_session_list_cache();
    let (groups, orphans) = load_sessions_grouped().expect("group sessions");
    let group = groups
        .iter()
        .find(|group| group.name == "cloud: dev-cloud")
        .expect("cloud group");
    assert_eq!(group.icon, "☁");
    assert!(
        !group.is_running,
        "ownership does not imply remote liveness"
    );
    assert_eq!(group.sessions[0].id, id);
    assert!(!orphans.iter().any(|session| session.id == id));

    lease.away_host = None;
    storage::write_session_lease(&lease).expect("return lease");
    invalidate_session_list_cache();
    let (groups, orphans) = load_sessions_grouped().expect("group returned sessions");
    assert!(!groups.iter().any(|group| group.name == "cloud: dev-cloud"));
    assert!(
        orphans
            .iter()
            .any(|session| session.id == id && session.server_icon.is_none())
    );
}

#[test]
fn cloud_session_discovery_keeps_missing_snapshots_and_ignores_corrupt_leases() {
    let _env_lock = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("temp dir");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let dir = temp.path().join("session_leases");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("broken.json"), "{").unwrap();
    storage::write_session_lease(&storage::SessionLease {
        session_id: "session_missing_1770000000000".into(),
        epoch: 1,
        away_host: Some("dev-cloud".into()),
        repo_root: None,
        last_sync_snapshot: None,
        last_sync_head: None,
        updated_at: chrono::Utc::now().to_rfc3339(),
    })
    .unwrap();
    let mut sessions = Vec::new();
    include_cloud_sessions(
        &temp.path().join("sessions"),
        &crate::catchup::CatchupSeenSnapshot::load(),
        &mut sessions,
    );
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, "session_missing_1770000000000");
    assert_eq!(sessions[0].message_count, 0);
    assert_eq!(sessions[0].server_name.as_deref(), Some("cloud: dev-cloud"));

    // An empty local migration stub must remain selectable as well.
    let session = Session::create_with_id("session_missing_1770000000000".into(), None, None);
    storage::write_json(
        &crate::session::session_path(&session.id).unwrap(),
        &session,
    )
    .expect("save empty local stub");
    sessions.clear();
    include_cloud_sessions(
        &temp.path().join("sessions"),
        &crate::catchup::CatchupSeenSnapshot::load(),
        &mut sessions,
    );
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].message_count, 0);
    assert!(sessions[0].title.contains("cloud session"));
}

#[test]
fn collect_recent_session_stems_keeps_empty_snapshot_with_journal_history() {
    let temp = tempfile::tempdir().expect("temp dir");
    let stem = "session_alpha_1770000000000";
    write_picker_snapshot(&temp.path().join(format!("{stem}.json")), false);
    std::fs::write(
        temp.path().join(format!("{stem}.journal.jsonl")),
        "{\"append_messages\":[{\"role\":\"user\"}]}",
    )
    .expect("write journal");

    let stems = collect_recent_session_stems(temp.path(), 1).expect("collect stems");
    assert_eq!(stems, vec![stem.to_string()]);
}

#[test]
fn collect_recent_session_stems_expands_candidate_window_past_recent_empty_stubs() {
    let temp = tempfile::tempdir().expect("temp dir");

    for idx in 0..30 {
        let stem = format!("session_empty_{}", 1770000000030u64 - idx as u64);
        write_picker_snapshot(&temp.path().join(format!("{stem}.json")), false);
    }

    let older_stem = "session_full_1770000000000";
    write_picker_snapshot(&temp.path().join(format!("{older_stem}.json")), true);

    let stems = collect_recent_session_stems(temp.path(), 1).expect("collect stems");
    assert_eq!(stems, vec![older_stem.to_string()]);
}

#[test]
fn trivial_hidden_only_snapshot_detector_skips_system_stub() {
    let bytes = br#"{"messages":[{"role":"user","content":[{"type":"text","text":"<system-reminder>boot</system-reminder>"}],"display_role":"system"}]}"#;
    assert!(snapshot_bytes_look_trivial_hidden_only(bytes));
}

#[test]
fn trivial_hidden_only_snapshot_detector_keeps_visible_message() {
    let bytes = br#"{"messages":[{"role":"user","content":[{"type":"text","text":"hello"}]}]}"#;
    assert!(!snapshot_bytes_look_trivial_hidden_only(bytes));
}

#[test]
fn trivial_hidden_only_snapshot_detector_keeps_system_plus_visible_message() {
    let bytes = br#"{"messages":[{"role":"user","content":[{"type":"text","text":"<system-reminder>boot</system-reminder>"}],"display_role":"system"},{"role":"assistant","content":[{"type":"text","text":"visible"}]}]}"#;
    assert!(!snapshot_bytes_look_trivial_hidden_only(bytes));
}

#[test]
fn cached_grouped_sessions_round_trip_from_disk_borrows_inventories() {
    let _env_lock = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("temp dir");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let _scan_limit = EnvVarGuard::set_str("JCODE_SESSION_PICKER_MAX_SESSIONS", "100");
    let _include_saved = EnvVarGuard::set_str("JCODE_SESSION_PICKER_INCLUDE_OLD_SAVED", "0");

    let sessions_dir = temp.path().join("sessions");
    std::fs::create_dir_all(&sessions_dir).expect("create sessions dir");
    let now = chrono::Utc::now();
    let mut session = SessionInfo {
        id: "session_cache_test_1770000000000".to_string(),
        parent_id: None,
        short_name: "cache-test".to_string(),
        icon: "🧪".to_string(),
        title: "Cache test".to_string(),
        message_count: 1,
        user_message_count: 1,
        assistant_message_count: 0,
        created_at: now,
        last_message_time: now,
        last_active_at: Some(now),
        working_dir: Some("/tmp/cache-test".to_string()),
        model: None,
        provider_key: None,
        is_canary: false,
        is_debug: false,
        saved: false,
        save_label: None,
        status: SessionStatus::Closed,
        needs_catchup: false,
        estimated_tokens: 0,
        first_user_prompt: None,
        messages_preview: Vec::new(),
        search_index: "cache test".to_string(),
        server_name: None,
        server_icon: None,
        source: SessionSource::Jcode,
        resume_target: ResumeTarget::JcodeSession {
            session_id: "session_cache_test_1770000000000".to_string(),
        },
        external_path: None,
        todo_statuses: Vec::new(),
    };
    // Large payloads must be serialized in place, not cloned for a cache write.
    session.search_index = "search λ\n".repeat(32 * 1024);
    session.messages_preview.push(PreviewMessage {
        role: "assistant".into(),
        content: "preview 🧪\n".repeat(32 * 1024),
        tool_calls: vec!["tool".into()],
        tool_data: None,
        duration_secs: Some(1.25),
        title: Some("Preview title".into()),
        timestamp: Some(now),
    });
    let cache: GroupedSessionListDiskCache = GroupedSessionListDiskCache {
        version: SESSION_LIST_DISK_CACHE_VERSION,
        generated_at: now,
        sessions_dir,
        scan_limit: session_scan_limit(),
        include_old_saved_sessions: include_old_saved_sessions_on_initial_load(),
        external_sessions: include_external_sessions(),
        server_groups: vec![ServerGroup {
            name: "test-server".into(),
            icon: "🧪".into(),
            version: "test-version".into(),
            git_hash: "test-hash".into(),
            is_running: true,
            sessions: vec![session.clone()],
        }],
        orphan_sessions: vec![session],
    };

    let mut borrowed = grouped_session_list_disk_cache(
        &cache.sessions_dir,
        cache.scan_limit,
        &cache.server_groups,
        &cache.orphan_sessions,
    );
    borrowed.generated_at = cache.generated_at;
    assert!(std::ptr::eq(
        borrowed.server_groups,
        cache.server_groups.as_slice()
    ));
    assert!(std::ptr::eq(
        borrowed.orphan_sessions,
        cache.orphan_sessions.as_slice()
    ));
    for (borrowed_session, original) in borrowed.server_groups[0]
        .sessions
        .iter()
        .chain(borrowed.orphan_sessions.iter())
        .zip(
            cache.server_groups[0]
                .sessions
                .iter()
                .chain(cache.orphan_sessions.iter()),
        )
    {
        assert_eq!(
            borrowed_session.search_index.as_ptr(),
            original.search_index.as_ptr()
        );
        assert_eq!(
            borrowed_session.messages_preview[0].content.as_ptr(),
            original.messages_preview[0].content.as_ptr()
        );
    }
    let expected = serde_json::to_vec(&cache).expect("legacy owned encoding");
    assert_eq!(
        serde_json::to_vec(&borrowed).expect("borrowed encoding"),
        expected
    );
    let decoded: GroupedSessionListDiskCache = serde_json::from_slice(&expected).unwrap();
    assert_eq!(
        decoded.server_groups[0].sessions[0].search_index,
        cache.server_groups[0].sessions[0].search_index
    );
    assert_eq!(
        decoded.orphan_sessions[0].messages_preview[0].content,
        cache.orphan_sessions[0].messages_preview[0].content
    );

    write_grouped_session_list_disk_cache(
        &cache.sessions_dir,
        cache.scan_limit,
        &cache.server_groups,
        &cache.orphan_sessions,
    );
    let (groups, orphans) = load_cached_sessions_grouped().expect("load cache");
    assert_eq!(groups.len(), 1);
    assert_eq!(
        groups[0].sessions[0].search_index,
        cache.server_groups[0].sessions[0].search_index
    );
    assert_eq!(
        orphans[0].messages_preview[0].content,
        cache.orphan_sessions[0].messages_preview[0].content
    );
    assert_eq!(orphans.len(), 1);
    assert_eq!(orphans[0].id, "session_cache_test_1770000000000");
    assert_eq!(orphans[0].title, "Cache test");
}
