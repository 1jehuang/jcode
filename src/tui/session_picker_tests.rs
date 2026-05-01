use super::*;
use chrono::{Duration as ChronoDuration, Utc};
use std::io::Write;
use std::time::{Duration as StdDuration, SystemTime};

fn write_session_file_with_mtime(
    path: impl AsRef<std::path::Path>,
    content: &str,
    modified_secs: u64,
) {
    let mut file = std::fs::File::create(path.as_ref()).expect("create session file");
    file.write_all(content.as_bytes())
        .expect("write session file");
    file.set_modified(SystemTime::UNIX_EPOCH + StdDuration::from_secs(modified_secs))
        .expect("set modified time");
}

fn make_session(id: &str, short_name: &str, is_debug: bool, status: SessionStatus) -> SessionInfo {
    make_session_with_flags(id, short_name, is_debug, false, status)
}

fn make_session_with_flags(
    id: &str,
    short_name: &str,
    is_debug: bool,
    is_canary: bool,
    status: SessionStatus,
) -> SessionInfo {
    let now = Utc::now();
    let title = "Test session".to_string();
    let working_dir = Some("/tmp".to_string());
    let messages_preview = vec![
        PreviewMessage {
            role: "user".to_string(),
            content: "hello".to_string(),
            tool_calls: Vec::new(),
            tool_data: None,
            timestamp: None,
        },
        PreviewMessage {
            role: "assistant".to_string(),
            content: "world".to_string(),
            tool_calls: Vec::new(),
            tool_data: None,
            timestamp: None,
        },
    ];
    let search_index = build_search_index(
        id,
        short_name,
        &title,
        working_dir.as_deref(),
        None,
        &messages_preview,
    );

    SessionInfo {
        id: id.to_string(),
        parent_id: None,
        short_name: short_name.to_string(),
        icon: "🧪".to_string(),
        title,
        message_count: 2,
        user_message_count: 1,
        assistant_message_count: 1,
        created_at: now - ChronoDuration::minutes(5),
        last_message_time: now - ChronoDuration::minutes(1),
        last_active_at: Some(now - ChronoDuration::minutes(1)),
        working_dir,
        model: None,
        provider_key: None,
        is_canary,
        is_debug,
        saved: false,
        save_label: None,
        status,
        needs_catchup: false,
        estimated_tokens: 200,
        messages_preview,
        search_index,
        server_name: None,
        server_icon: None,
        source: SessionSource::Jcode,
        resume_target: ResumeTarget::JcodeSession {
            session_id: id.to_string(),
        },
        external_path: None,
    }
}

#[test]
fn test_status_inference() {
    // Load sessions and ensure status display works
    let sessions = load_sessions().unwrap();
    for session in &sessions {
        let _ = session.status.display();
    }
}

#[test]
fn test_collect_recent_session_stems_skips_empty_recent_sessions() {
    let dir = tempfile::TempDir::new().expect("tempdir");

    write_session_file_with_mtime(
        dir.path().join("session_alpha_1000.json"),
        r#"{"messages":[{"role":"user","content":"hi"}]}"#,
        1000,
    );
    write_session_file_with_mtime(
        dir.path().join("session_beta_2000.json"),
        r#"{"messages":[]}"#,
        2000,
    );
    write_session_file_with_mtime(
        dir.path().join("session_gamma_3000.json"),
        r#"{"messages":[{"role":"user","content":"hello"}]}"#,
        3000,
    );
    write_session_file_with_mtime(
        dir.path().join("session_delta_4000.json"),
        r#"{"messages":[]}"#,
        4000,
    );

    let stems = collect_recent_session_stems(dir.path(), 2).expect("collect stems");
    assert_eq!(stems, vec!["session_gamma_3000", "session_alpha_1000"]);
}

#[test]
fn test_collect_recent_session_stems_uses_timestamp_as_mtime_tiebreaker() {
    let dir = tempfile::TempDir::new().expect("tempdir");

    write_session_file_with_mtime(
        dir.path().join("session_old_1111.json"),
        r#"{"messages":[{"role":"user","content":"old"}]}"#,
        1000,
    );
    write_session_file_with_mtime(
        dir.path().join("session_mid_2222.json"),
        r#"{"messages":[{"role":"user","content":"mid"}]}"#,
        1000,
    );
    write_session_file_with_mtime(
        dir.path().join("session_new_3333.json"),
        r#"{"messages":[{"role":"user","content":"new"}]}"#,
        1000,
    );

    let stems = collect_recent_session_stems(dir.path(), 3).expect("collect stems");
    assert_eq!(
        stems,
        vec!["session_new_3333", "session_mid_2222", "session_old_1111"]
    );
}

#[test]
fn test_collect_recent_session_stems_prefers_recently_modified_long_running_session() {
    let dir = tempfile::TempDir::new().expect("tempdir");

    for idx in 0..120 {
        write_session_file_with_mtime(
            &dir.path().join(format!(
                "session_newer_created_{:013}.json",
                2_000_000 + idx
            )),
            r#"{"messages":[{"role":"user","content":"short newer-created session"}]}"#,
            1000 + idx,
        );
    }

    let target = "session_long_running_0000000000500";
    write_session_file_with_mtime(
        &dir.path().join(format!("{target}.json")),
        r#"{"messages":[{"role":"user","content":"old creation time, recently active"}]}"#,
        10_000,
    );

    let stems = collect_recent_session_stems(dir.path(), 100).expect("collect stems");
    assert_eq!(stems.first().map(String::as_str), Some(target));
    assert!(stems.iter().any(|stem| stem == target));
}

#[test]
fn test_toggle_test_sessions_rebuilds_visibility() {
    let normal = make_session("session_normal", "normal", false, SessionStatus::Closed);
    let debug = make_session("session_debug", "debug", true, SessionStatus::Closed);

    let mut picker = SessionPicker::new(vec![normal.clone(), debug.clone()]);

    assert_eq!(picker.visible_sessions.len(), 1);
    assert!(!picker.show_test_sessions);
    assert_eq!(picker.hidden_test_count, 1);

    picker.toggle_test_sessions();
    assert!(picker.show_test_sessions);
    assert_eq!(picker.visible_sessions.len(), 2);
    assert_eq!(picker.hidden_test_count, 0);

    picker.toggle_test_sessions();
    assert!(!picker.show_test_sessions);
    assert_eq!(picker.visible_sessions.len(), 1);
    assert_eq!(picker.hidden_test_count, 1);
}

#[test]
fn test_new_grouped_hides_debug_by_default() {
    let normal = make_session("session_normal", "normal", false, SessionStatus::Closed);
    let debug = make_session("session_debug", "debug", true, SessionStatus::Closed);
    let canary = make_session_with_flags(
        "session_canary",
        "canary",
        false,
        true,
        SessionStatus::Closed,
    );
    let orphan_normal = make_session(
        "orphan_normal",
        "orphan-normal",
        false,
        SessionStatus::Closed,
    );
    let orphan_debug = make_session("orphan_debug", "orphan-debug", true, SessionStatus::Closed);

    let groups = vec![ServerGroup {
        name: "main".to_string(),
        icon: "🛰".to_string(),
        version: "v0.1.0".to_string(),
        git_hash: "abc1234".to_string(),
        is_running: true,
        sessions: vec![normal.clone(), debug.clone(), canary.clone()],
    }];

    let mut picker = SessionPicker::new_grouped(groups, vec![orphan_normal, orphan_debug]);

    assert!(!picker.show_test_sessions);
    // Canary sessions are now visible by default, only debug sessions are hidden
    assert_eq!(picker.visible_sessions.len(), 3); // normal + canary + orphan_normal
    assert!(picker.visible_session_iter().all(|s| !s.is_debug));
    assert_eq!(picker.hidden_test_count, 2); // debug + orphan_debug

    picker.toggle_test_sessions();
    assert!(picker.show_test_sessions);
    assert_eq!(picker.visible_sessions.len(), 5);
    assert_eq!(picker.hidden_test_count, 0);
    assert!(picker.visible_session_iter().any(|s| s.is_debug));
    assert!(picker.visible_session_iter().any(|s| s.is_canary));
}

#[test]
fn test_new_grouped_without_servers_shows_orphan_sessions() {
    let normal = make_session("session_normal", "normal", false, SessionStatus::Closed);
    let debug = make_session("session_debug", "debug", true, SessionStatus::Closed);

    let mut picker = SessionPicker::new_grouped(Vec::new(), vec![normal, debug]);

    assert!(!picker.show_test_sessions);
    assert_eq!(picker.visible_sessions.len(), 1);
    assert!(picker.visible_session_iter().all(|s| !s.is_debug));
    assert_eq!(picker.hidden_test_count, 1);
    assert_eq!(picker.items.len(), 1);
    assert_eq!(picker.list_state.selected(), Some(0));

    picker.toggle_test_sessions();
    assert!(picker.show_test_sessions);
    assert_eq!(picker.visible_sessions.len(), 2);
    assert_eq!(picker.hidden_test_count, 0);
    assert_eq!(picker.items.len(), 2);
    assert!(picker.visible_session_iter().any(|s| s.is_debug));
}

#[test]
fn test_crash_reason_line_for_crashed_sessions() {
    let crashed = make_session(
        "session_crash",
        "crash",
        false,
        SessionStatus::Crashed {
            message: Some("Terminal or window closed (SIGHUP)".to_string()),
        },
    );
    let line = SessionPicker::crash_reason_line(&crashed).expect("crash reason should render");
    let text: String = line
        .spans
        .into_iter()
        .map(|s| s.content.to_string())
        .collect();
    assert!(text.contains("reason:"));
    assert!(text.contains("SIGHUP"));
}

#[test]
fn test_batch_restore_detection_excludes_already_recovered_parent_sessions() {
    let crashed = make_session(
        "session_crash_source",
        "crash-source",
        false,
        SessionStatus::Crashed {
            message: Some("boom".to_string()),
        },
    );

    let mut recovered = make_session(
        "session_recovery_rec123",
        "recovered",
        false,
        SessionStatus::Closed,
    );
    recovered.parent_id = Some(crashed.id.clone());

    let picker = SessionPicker::new(vec![crashed, recovered]);

    assert!(picker.crashed_sessions.is_none());
    assert!(picker.crashed_session_ids.is_empty());
}

#[test]
fn test_grouped_batch_restore_uses_last_active_at_and_includes_debug_sessions() {
    let now = Utc::now();

    let mut recent_normal = make_session(
        "session_recent_normal",
        "recent-normal",
        false,
        SessionStatus::Crashed {
            message: Some("recent crash".to_string()),
        },
    );
    recent_normal.last_message_time = now - ChronoDuration::minutes(10);
    recent_normal.last_active_at = Some(now - ChronoDuration::seconds(10));

    let mut recent_debug = make_session(
        "session_recent_debug",
        "recent-debug",
        true,
        SessionStatus::Crashed {
            message: Some("debug crash".to_string()),
        },
    );
    recent_debug.last_message_time = now - ChronoDuration::minutes(9);
    recent_debug.last_active_at = Some(now - ChronoDuration::seconds(20));

    let mut stale_crash = make_session(
        "session_stale_crash",
        "stale-crash",
        false,
        SessionStatus::Crashed {
            message: Some("old crash".to_string()),
        },
    );
    stale_crash.last_message_time = now - ChronoDuration::seconds(30);
    stale_crash.last_active_at = Some(now - ChronoDuration::minutes(3));

    let picker = SessionPicker::new_grouped(
        vec![ServerGroup {
            name: "main".to_string(),
            icon: "🛰".to_string(),
            version: "v0.1.0".to_string(),
            git_hash: "abc1234".to_string(),
            is_running: true,
            sessions: vec![recent_normal.clone(), recent_debug.clone(), stale_crash],
        }],
        Vec::new(),
    );

    let crashed = picker
        .crashed_sessions
        .as_ref()
        .expect("expected eligible crashed sessions");

    assert_eq!(crashed.session_ids.len(), 2);
    assert!(crashed.session_ids.contains(&recent_normal.id));
    assert!(crashed.session_ids.contains(&recent_debug.id));
    assert!(
        !crashed
            .session_ids
            .iter()
            .any(|id| id == "session_stale_crash")
    );
}

#[test]
fn test_filter_matches_recent_message_content() {
    let mut picker = SessionPicker::new(vec![make_session(
        "session_content",
        "content",
        false,
        SessionStatus::Closed,
    )]);

    picker.search_query = "world".to_string();
    picker.rebuild_items();
    assert_eq!(picker.visible_sessions.len(), 1);

    picker.search_query = "not-in-preview".to_string();
    picker.rebuild_items();
    assert!(picker.visible_sessions.is_empty());
}

#[test]
fn test_loading_preview_refreshes_search_index_for_picker_filtering() {
    let _env_lock = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("temp dir");
    let previous_home = std::env::var("JCODE_HOME").ok();
    crate::env::set_var("JCODE_HOME", temp.path());

    let mut session = Session::create_with_id(
        "session_preview_search".to_string(),
        Some("/tmp/preview-search".to_string()),
        Some("Preview Search".to_string()),
    );
    session.append_stored_message(crate::session::StoredMessage {
        id: "msg1".to_string(),
        role: crate::message::Role::User,
        content: vec![crate::message::ContentBlock::Text {
            text: "needle hidden outside the initial picker summary".to_string(),
            cache_control: None,
        }],
        display_role: None,
        timestamp: None,
        tool_duration_ms: None,
        token_usage: None,
    });
    session.save().expect("save session");

    let sessions = load_sessions().expect("load sessions");
    let mut picker = SessionPicker::new(sessions);

    let selected_before = picker.selected_session().expect("selected session");
    assert!(!selected_before.search_index.contains("needle hidden"));

    picker.ensure_selected_preview_loaded();

    let selected_after = picker
        .selected_session()
        .expect("selected session after preview");
    assert!(selected_after.search_index.contains("needle hidden"));

    picker.search_query = "needle hidden".to_string();
    picker.rebuild_items();
    assert_eq!(picker.visible_sessions.len(), 1);

    if let Some(previous_home) = previous_home {
        crate::env::set_var("JCODE_HOME", previous_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn benchmark_resume_search_reports_incremental_timings() {
    let sessions = (0..500)
        .map(|idx| {
            let mut session = make_session(
                &format!("session_bench_{idx:03}"),
                &format!("bench-{idx:03}"),
                false,
                SessionStatus::Closed,
            );
            session.messages_preview = vec![PreviewMessage {
                role: "user".to_string(),
                content: format!("benchmark transcript content alpha beta zebra-token-{idx:03}"),
                tool_calls: Vec::new(),
                tool_data: None,
                timestamp: None,
            }];
            session.search_index = build_search_index(
                &session.id,
                &session.short_name,
                &session.title,
                session.working_dir.as_deref(),
                None,
                &session.messages_preview,
            );
            session
        })
        .collect::<Vec<_>>();

    let mut picker = SessionPicker::new(sessions);

    let first_start = std::time::Instant::now();
    picker.search_query = "z".to_string();
    picker.rebuild_items();
    let first_ms = first_start.elapsed().as_secs_f64() * 1000.0;

    let second_start = std::time::Instant::now();
    picker.search_query = "ze".to_string();
    picker.rebuild_items();
    let second_ms = second_start.elapsed().as_secs_f64() * 1000.0;

    let third_start = std::time::Instant::now();
    picker.search_query = "zebra-token-499".to_string();
    picker.rebuild_items();
    let third_ms = third_start.elapsed().as_secs_f64() * 1000.0;

    assert_eq!(picker.visible_sessions.len(), 1);
    eprintln!(
        "resume search bench: first_char={:.3}ms second_char={:.3}ms full_query={:.3}ms sessions=500",
        first_ms, second_ms, third_ms
    );
}

#[test]
fn test_filter_mode_cycles_through_requested_session_sources() {
    let mut saved = make_session("session_saved", "saved", false, SessionStatus::Closed);
    saved.saved = true;
    saved.needs_catchup = true;

    let mut claude_code = make_session("claude:demo", "claude-code", false, SessionStatus::Closed);
    claude_code.source = SessionSource::ClaudeCode;
    claude_code.resume_target = ResumeTarget::ClaudeCodeSession {
        session_id: "claude-session-demo".to_string(),
        session_path: "/tmp/claude-session-demo.jsonl".to_string(),
    };

    let mut codex = make_session("session_codex", "codex", false, SessionStatus::Closed);
    codex.model = Some("gpt-5.3-codex".to_string());
    codex.source = SessionSource::Codex;

    let mut pi = make_session("session_pi", "pi", false, SessionStatus::Closed);
    pi.provider_key = Some("pi".to_string());
    pi.source = SessionSource::Pi;

    let mut opencode = make_session("session_opencode", "opencode", false, SessionStatus::Closed);
    opencode.provider_key = Some("opencode".to_string());
    opencode.source = SessionSource::OpenCode;

    let mut picker = SessionPicker::new(vec![saved, claude_code, codex, pi, opencode]);

    assert_eq!(picker.filter_mode, SessionFilterMode::All);
    assert_eq!(picker.visible_sessions.len(), 5);

    picker.cycle_filter_mode();
    assert_eq!(picker.filter_mode, SessionFilterMode::CatchUp);
    assert_eq!(picker.visible_sessions.len(), 1);
    assert!(
        picker
            .visible_session_iter()
            .all(|session| session.needs_catchup)
    );

    picker.cycle_filter_mode();
    assert_eq!(picker.filter_mode, SessionFilterMode::Saved);
    assert_eq!(picker.visible_sessions.len(), 1);
    assert!(picker.visible_session_iter().all(|session| session.saved));
    assert_eq!(picker.items.len(), picker.visible_sessions.len());

    picker.cycle_filter_mode();
    assert_eq!(picker.filter_mode, SessionFilterMode::ClaudeCode);
    assert_eq!(picker.visible_sessions.len(), 1);
    assert!(
        picker
            .visible_session_iter()
            .all(SessionPicker::session_is_claude_code)
    );

    picker.cycle_filter_mode();
    assert_eq!(picker.filter_mode, SessionFilterMode::Codex);
    assert_eq!(picker.visible_sessions.len(), 1);
    assert!(
        picker
            .visible_session_iter()
            .all(SessionPicker::session_is_codex)
    );

    picker.cycle_filter_mode();
    assert_eq!(picker.filter_mode, SessionFilterMode::Pi);
    assert_eq!(picker.visible_sessions.len(), 1);
    assert!(
        picker
            .visible_session_iter()
            .all(SessionPicker::session_is_pi)
    );

    picker.cycle_filter_mode();
    assert_eq!(picker.filter_mode, SessionFilterMode::OpenCode);
    assert_eq!(picker.visible_sessions.len(), 1);
    assert!(
        picker
            .visible_session_iter()
            .all(SessionPicker::session_is_open_code)
    );

    picker.cycle_filter_mode();
    assert_eq!(picker.filter_mode, SessionFilterMode::All);
    assert_eq!(picker.visible_sessions.len(), 5);
}

#[test]
fn test_filter_mode_keyboard_shortcuts_cycle_both_directions() {
    let mut picker = SessionPicker::new(vec![make_session(
        "session_saved",
        "saved",
        false,
        SessionStatus::Closed,
    )]);
    picker
        .handle_overlay_key(KeyCode::Char('s'), KeyModifiers::empty())
        .unwrap();
    assert_eq!(picker.filter_mode, SessionFilterMode::CatchUp);

    picker
        .handle_overlay_key(KeyCode::Char('S'), KeyModifiers::empty())
        .unwrap();
    assert_eq!(picker.filter_mode, SessionFilterMode::All);
}

#[test]
fn test_space_selects_multiple_sessions_and_enter_returns_them() {
    let mut newer = make_session("session_newer", "newer", false, SessionStatus::Closed);
    let mut older = make_session("session_older", "older", false, SessionStatus::Closed);
    newer.last_message_time = Utc::now();
    older.last_message_time = Utc::now() - ChronoDuration::minutes(1);

    let mut picker = SessionPicker::new(vec![older, newer]);

    picker
        .handle_overlay_key(KeyCode::Char(' '), KeyModifiers::empty())
        .unwrap();
    picker
        .handle_overlay_key(KeyCode::Down, KeyModifiers::empty())
        .unwrap();
    picker
        .handle_overlay_key(KeyCode::Char(' '), KeyModifiers::empty())
        .unwrap();

    let action = picker
        .handle_overlay_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();

    match action {
        OverlayAction::Selected(PickerResult::Selected(ids)) => {
            assert_eq!(
                ids,
                vec![
                    ResumeTarget::JcodeSession {
                        session_id: "session_newer".to_string(),
                    },
                    ResumeTarget::JcodeSession {
                        session_id: "session_older".to_string(),
                    }
                ]
            );
        }
        other => panic!("expected selected sessions, got {other:?}"),
    }
}

#[test]
fn test_rebuild_items_prunes_selected_sessions_hidden_by_filter() {
    let mut saved = make_session("session_saved", "saved", false, SessionStatus::Closed);
    saved.saved = true;
    let normal = make_session("session_normal", "normal", false, SessionStatus::Closed);

    let mut picker = SessionPicker::new(vec![saved, normal]);
    picker
        .selected_session_ids
        .insert("session_saved".to_string());
    picker
        .selected_session_ids
        .insert("session_normal".to_string());

    picker.filter_mode = SessionFilterMode::Saved;
    picker.rebuild_items();

    assert_eq!(picker.selected_session_ids.len(), 1);
    assert!(picker.selected_session_ids.contains("session_saved"));
}

#[test]
fn test_mouse_scroll_only_affects_hovered_pane_without_changing_focus() {
    let s1 = make_session("session_1", "one", false, SessionStatus::Closed);
    let s2 = make_session("session_2", "two", false, SessionStatus::Closed);
    let s3 = make_session("session_3", "three", false, SessionStatus::Closed);
    let mut picker = SessionPicker::new(vec![s1, s2, s3]);

    picker.focus = PaneFocus::Preview;
    picker.scroll_offset = 7;
    picker.last_list_area = Some(Rect::new(0, 0, 20, 10));
    picker.last_preview_area = Some(Rect::new(20, 0, 20, 10));

    picker.handle_overlay_mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 5,
        row: 5,
        modifiers: KeyModifiers::empty(),
    });

    assert_eq!(picker.focus, PaneFocus::Preview);
    assert_eq!(picker.scroll_offset, 0);
    assert_eq!(
        picker.selected_session().map(|s| s.id.as_str()),
        Some("session_2")
    );
}

#[test]
fn test_keyboard_scroll_uses_sessions_focus_for_paging() {
    let s1 = make_session("session_1", "one", false, SessionStatus::Closed);
    let s2 = make_session("session_2", "two", false, SessionStatus::Closed);
    let s3 = make_session("session_3", "three", false, SessionStatus::Closed);
    let s4 = make_session("session_4", "four", false, SessionStatus::Closed);
    let mut picker = SessionPicker::new(vec![s1, s2, s3, s4]);

    picker.focus = PaneFocus::Sessions;
    picker.scroll_offset = 6;

    let result = picker.handle_overlay_key(KeyCode::PageDown, KeyModifiers::empty());

    assert!(matches!(result, Ok(OverlayAction::Continue)));
    assert_eq!(picker.focus, PaneFocus::Sessions);
    assert_eq!(picker.scroll_offset, 0);
    assert_eq!(
        picker.selected_session().map(|s| s.id.as_str()),
        Some("session_1")
    );
}

#[test]
fn test_keyboard_scroll_uses_preview_focus_for_paging() {
    let s1 = make_session("session_1", "one", false, SessionStatus::Closed);
    let s2 = make_session("session_2", "two", false, SessionStatus::Closed);
    let mut picker = SessionPicker::new(vec![s1, s2]);

    picker.focus = PaneFocus::Preview;

    let result = picker.handle_overlay_key(KeyCode::PageDown, KeyModifiers::empty());

    assert!(matches!(result, Ok(OverlayAction::Continue)));
    assert_eq!(picker.focus, PaneFocus::Preview);
    assert_eq!(picker.scroll_offset, PREVIEW_PAGE_SCROLL);
    assert_eq!(
        picker.selected_session().map(|s| s.id.as_str()),
        Some("session_2")
    );
}
