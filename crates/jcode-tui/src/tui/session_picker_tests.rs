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

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>()
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
        first_user_prompt: messages_preview
            .iter()
            .find(|msg| msg.role == "user" && !msg.content.trim().is_empty())
            .map(|msg| msg.content.clone()),
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
#[ignore = "developer benchmark: profiles real /resume through first rendered picker frame"]
fn benchmark_real_resume_first_render_reports_timings() {
    invalidate_session_list_cache();

    let total_start = std::time::Instant::now();

    let loading_render_start = std::time::Instant::now();
    let mut loading_picker = SessionPicker::loading();
    let backend = ratatui::backend::TestBackend::new(120, 40);
    let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| loading_picker.render(frame))
        .expect("render loading picker");
    let loading_render_elapsed = loading_render_start.elapsed();

    let load_start = std::time::Instant::now();
    let (server_groups, orphan_sessions) = load_sessions_grouped().expect("load sessions grouped");
    let load_elapsed = load_start.elapsed();
    let loaded_count: usize = server_groups
        .iter()
        .map(|group| group.sessions.len())
        .sum::<usize>()
        + orphan_sessions.len();

    let construct_start = std::time::Instant::now();
    let mut picker = SessionPicker::new_grouped(server_groups, orphan_sessions);
    let selected_before_render = picker.selected_session().map(|session| {
        (
            session.id.clone(),
            session.title.clone(),
            session.external_path.clone(),
            session.messages_preview.len(),
        )
    });
    let construct_elapsed = construct_start.elapsed();

    let first_render_start = std::time::Instant::now();
    terminal
        .draw(|frame| picker.render(frame))
        .expect("render loaded picker");
    let first_render_elapsed = first_render_start.elapsed();
    let selected_after_first_render = picker
        .selected_session()
        .map(|session| (session.id.clone(), session.messages_preview.len()));

    let second_render_start = std::time::Instant::now();
    terminal
        .draw(|frame| picker.render(frame))
        .expect("render loaded picker again");
    let second_render_elapsed = second_render_start.elapsed();

    eprintln!(
        "real resume first render: total={}ms loading_render={}ms load_grouped={}ms/{} construct={}ms first_render={}ms second_render={}ms selected_before={:?} selected_after={:?}",
        total_start.elapsed().as_millis(),
        loading_render_elapsed.as_millis(),
        load_elapsed.as_millis(),
        loaded_count,
        construct_elapsed.as_millis(),
        first_render_elapsed.as_millis(),
        second_render_elapsed.as_millis(),
        selected_before_render,
        selected_after_first_render,
    );
}

#[test]
#[ignore = "developer benchmark: profiles cached /resume first render latency"]
fn benchmark_real_resume_cached_first_render_reports_timings() {
    invalidate_session_list_cache();

    let refresh_start = std::time::Instant::now();
    let (_fresh_groups, _fresh_orphans) =
        load_sessions_grouped().expect("refresh sessions grouped");
    let refresh_elapsed = refresh_start.elapsed();

    let total_start = std::time::Instant::now();
    let cache_start = std::time::Instant::now();
    let (server_groups, orphan_sessions) =
        load_cached_sessions_grouped().expect("load cached sessions grouped");
    let cache_elapsed = cache_start.elapsed();
    let cached_count: usize = server_groups
        .iter()
        .map(|group| group.sessions.len())
        .sum::<usize>()
        + orphan_sessions.len();

    let construct_start = std::time::Instant::now();
    let mut picker = SessionPicker::new_grouped(server_groups, orphan_sessions);
    let construct_elapsed = construct_start.elapsed();

    let render_start = std::time::Instant::now();
    let backend = ratatui::backend::TestBackend::new(120, 40);
    let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| picker.render(frame))
        .expect("render cached picker");
    let render_elapsed = render_start.elapsed();

    eprintln!(
        "real resume cached first render: total={}ms cache_read={}ms/{} construct={}ms first_render={}ms cache_refresh={}ms",
        total_start.elapsed().as_millis(),
        cache_elapsed.as_millis(),
        cached_count,
        construct_elapsed.as_millis(),
        render_elapsed.as_millis(),
        refresh_elapsed.as_millis(),
    );
}

#[test]
fn test_format_estimated_tokens_uses_compact_units() {
    assert_eq!(SessionPicker::format_estimated_tokens(0), "~0 tok");
    assert_eq!(SessionPicker::format_estimated_tokens(999), "~999 tok");
    assert_eq!(SessionPicker::format_estimated_tokens(1_000), "~1k tok");
    assert_eq!(SessionPicker::format_estimated_tokens(1_234), "~1.2k tok");
    assert_eq!(SessionPicker::format_estimated_tokens(12_345), "~12k tok");
    assert_eq!(SessionPicker::format_estimated_tokens(999_500), "~1M tok");
    assert_eq!(
        SessionPicker::format_estimated_tokens(1_234_567),
        "~1.2M tok"
    );
    assert_eq!(
        SessionPicker::format_estimated_tokens(1_234_567_890),
        "~1.2B tok"
    );
    assert_eq!(
        SessionPicker::format_estimated_tokens(1_234_567_890_123),
        "~1.2T tok"
    );
}

#[test]
fn test_session_item_uses_single_primary_title_line() {
    let mut session = make_session(
        "session_primary_title",
        "rhino",
        false,
        SessionStatus::Closed,
    );
    session.title = "Generated release planning".to_string();
    session.estimated_tokens = 1_234_567;
    let picker = SessionPicker::new(vec![session.clone()]);

    let rows = picker.render_session_item_lines(&session, false);
    let text_rows: Vec<String> = rows.iter().map(line_text).collect();

    // The title must appear on exactly one row (the primary line); other rows
    // (stats, prompt preview, created/dir) must not repeat it.
    assert_eq!(
        text_rows
            .iter()
            .filter(|row| row.contains("Generated release planning"))
            .count(),
        1,
        "title should render on exactly one row: {text_rows:?}"
    );
    assert!(text_rows[0].contains("Generated release planning"));
    assert!(
        text_rows[1..]
            .iter()
            .all(|row| !row.contains("Generated release planning")),
        "title should only be rendered on the primary row: {text_rows:?}"
    );
    assert!(
        text_rows.iter().all(|row| !row.contains("rhino")),
        "memorable short name should remain searchable but not take display space: {text_rows:?}"
    );
    assert!(text_rows[1].contains("~1.2M tok"));
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
fn test_collect_recent_session_stems_skips_system_context_only_sessions() {
    let dir = tempfile::TempDir::new().expect("tempdir");

    write_session_file_with_mtime(
        dir.path().join("session_empty_context_9000.json"),
        r##"{"messages":[{"role":"user","display_role":"system","content":[{"type":"text","text":"<system-reminder>\n# Session Context\n</system-reminder>"}]}]}"##,
        9000,
    );
    write_session_file_with_mtime(
        dir.path().join("session_real_1000.json"),
        r#"{"messages":[{"role":"user","content":"real prompt"}]}"#,
        1000,
    );

    let stems = collect_recent_session_stems(dir.path(), 1).expect("collect stems");
    assert_eq!(stems, vec!["session_real_1000"]);
}

#[test]
fn test_collect_recent_session_stems_keeps_system_context_with_visible_journal_turn() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let stem = "session_context_then_journal_9000";

    write_session_file_with_mtime(
        dir.path().join(format!("{stem}.json")),
        r##"{"messages":[{"role":"user","display_role":"system","content":[{"type":"text","text":"<system-reminder>\n# Session Context\n</system-reminder>"}]}]}"##,
        1000,
    );
    write_session_file_with_mtime(
        dir.path().join(format!("{stem}.journal.jsonl")),
        r#"{"meta":{"updated_at":"2026-05-01T00:00:00Z"},"append_messages":[{"role":"user","content":"real prompt from journal"}]}"#,
        9000,
    );

    let stems = collect_recent_session_stems(dir.path(), 1).expect("collect stems");
    assert_eq!(stems, vec![stem]);
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
            dir.path().join(format!(
                "session_newer_created_{:013}.json",
                2_000_000 + idx
            )),
            r#"{"messages":[{"role":"user","content":"short newer-created session"}]}"#,
            1000 + idx,
        );
    }

    let target = "session_long_running_0000000000500";
    write_session_file_with_mtime(
        dir.path().join(format!("{target}.json")),
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
    assert_eq!(crashed.omitted_crashed_count, 1);
    assert!(crashed.session_ids.contains(&recent_normal.id));
    assert!(crashed.session_ids.contains(&recent_debug.id));
    assert!(
        !crashed
            .session_ids
            .iter()
            .any(|id| id == "session_stale_crash")
    );

    let mut picker = picker;
    let action = picker
        .handle_overlay_key(KeyCode::Char('R'), KeyModifiers::empty())
        .expect("restore group key should be handled");
    let OverlayAction::Selected(PickerResult::RestoreCrashedGroup(ids)) = action else {
        panic!("expected restore group action");
    };
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&recent_normal.id));
    assert!(ids.contains(&recent_debug.id));
    assert!(!ids.iter().any(|id| id == "session_stale_crash"));
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

    let mut cursor = make_session("session_cursor", "cursor", false, SessionStatus::Closed);
    cursor.provider_key = Some("cursor".to_string());
    cursor.source = SessionSource::Cursor;

    let mut picker = SessionPicker::new(vec![saved, claude_code, codex, pi, opencode, cursor]);

    assert_eq!(picker.filter_mode, SessionFilterMode::All);
    assert_eq!(picker.visible_sessions.len(), 6);

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
    assert_eq!(picker.filter_mode, SessionFilterMode::Active);
    // No live processes own these synthetic sessions, so the Active view is
    // empty in tests.
    assert_eq!(picker.visible_sessions.len(), 0);

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
    assert_eq!(picker.filter_mode, SessionFilterMode::Cursor);
    assert_eq!(picker.visible_sessions.len(), 1);
    assert!(
        picker
            .visible_session_iter()
            .all(SessionPicker::session_is_cursor)
    );

    picker.cycle_filter_mode();
    assert_eq!(picker.filter_mode, SessionFilterMode::All);
    assert_eq!(picker.visible_sessions.len(), 6);
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

fn live_presence(session_id: &str, streaming: bool) -> crate::session::SessionPresence {
    crate::session::SessionPresence {
        session_id: session_id.to_string(),
        pid: std::process::id(),
        streaming,
        streaming_since: streaming
            .then(|| std::time::SystemTime::now() - std::time::Duration::from_secs(90)),
        internal: false,
    }
}

#[test]
fn test_active_filter_shows_only_live_sessions_ready_before_working() {
    let live_working = make_session("session_working", "alpha", false, SessionStatus::Active);
    let live_ready = make_session("session_ready", "beta", false, SessionStatus::Active);
    let dead = make_session("session_dead", "dead", false, SessionStatus::Closed);

    let mut picker = SessionPicker::new(vec![live_working, live_ready, dead]);
    picker.activate_active_filter();
    picker.set_live_presence_for_test(vec![
        live_presence("session_working", true),
        live_presence("session_ready", false),
    ]);

    let visible: Vec<&str> = picker
        .visible_session_iter()
        .map(|session| session.id.as_str())
        .collect();
    // Only live sessions appear; the ready one is triaged above the working one.
    assert_eq!(visible, vec!["session_ready", "session_working"]);

    let ready = picker
        .visible_session_iter()
        .find(|session| session.id == "session_ready")
        .expect("ready visible");
    let working = picker
        .visible_session_iter()
        .find(|session| session.id == "session_working")
        .expect("working visible");
    assert!(!picker.session_is_streaming(ready));
    assert!(picker.session_is_streaming(working));
    assert!(picker.session_streaming_duration(working).is_some());
}

#[test]
fn test_active_rows_render_working_and_ready_badges() {
    let live_working = make_session("session_working", "alpha", false, SessionStatus::Active);
    let live_ready = make_session("session_ready", "beta", false, SessionStatus::Closed);
    let mut picker = SessionPicker::new(vec![live_working, live_ready]);
    picker.activate_active_filter();
    picker.set_live_presence_for_test(vec![
        live_presence("session_working", true),
        live_presence("session_ready", false),
    ]);

    let working = picker
        .visible_session_iter()
        .find(|session| session.id == "session_working")
        .cloned()
        .expect("working visible");
    let ready = picker
        .visible_session_iter()
        .find(|session| session.id == "session_ready")
        .cloned()
        .expect("ready visible");

    let working_lines = picker.render_session_item_lines(&working, false);
    let working_text = working_lines
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        working_text.contains("working 1m"),
        "expected working badge with duration, got: {working_text}"
    );

    let first_frame = picker
        .render_session_item_lines_at_frame(&working, false, 0)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");
    let second_frame = picker
        .render_session_item_lines_at_frame(&working, false, 1)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(first_frame.contains('⠋'));
    assert!(second_frame.contains('⠙'));
    assert_ne!(first_frame, second_frame, "running glyph should animate");
    assert!(picker.has_visible_running_sessions());

    let ready_lines = picker.render_session_item_lines(&ready, false);
    let ready_text = ready_lines
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        ready_text.contains("ready"),
        "expected ready badge, got: {ready_text}"
    );
    // Live presence overrides the stale persisted "closed" status.
    assert!(
        !ready_text.contains("closed"),
        "live session must not render as closed: {ready_text}"
    );
}

fn make_claude_session(session_id: &str) -> SessionInfo {
    let mut session = make_session(
        &format!("claude:{session_id}"),
        "claude live",
        false,
        SessionStatus::Closed,
    );
    session.source = SessionSource::ClaudeCode;
    session.provider_key = Some("claude-code".to_string());
    session.resume_target = ResumeTarget::ClaudeCodeSession {
        session_id: session_id.to_string(),
        session_path: format!("/tmp/{session_id}.jsonl"),
    };
    session
}

#[test]
fn live_claude_takeover_requires_explicit_key_and_confirmation() {
    let session = make_claude_session("claude-live-id");
    let target = session.resume_target.clone();
    let mut picker = SessionPicker::new(vec![session]);
    picker.set_live_presence_for_test(vec![live_presence("claude:claude-live-id", false)]);

    // Ordinary Enter remains an ordinary resume and never implies takeover.
    let ordinary = picker
        .handle_overlay_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();
    assert!(matches!(
        ordinary,
        OverlayAction::Selected(PickerResult::SelectedInCurrentTerminal(_))
            | OverlayAction::Selected(PickerResult::SelectedInNewTerminal(_))
    ));
    assert!(!picker.claude_takeover_confirmation_active_for_test());

    assert!(matches!(
        picker
            .handle_overlay_key(KeyCode::Char('T'), KeyModifiers::empty())
            .unwrap(),
        OverlayAction::Continue
    ));
    assert!(picker.claude_takeover_confirmation_active_for_test());

    // Cancellation does not emit an action.
    assert!(matches!(
        picker
            .handle_overlay_key(KeyCode::Esc, KeyModifiers::empty())
            .unwrap(),
        OverlayAction::Continue
    ));
    assert!(!picker.claude_takeover_confirmation_active_for_test());

    picker
        .handle_overlay_key(KeyCode::Char('T'), KeyModifiers::empty())
        .unwrap();
    let confirmed = picker
        .handle_overlay_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();
    assert!(matches!(
        confirmed,
        OverlayAction::Selected(PickerResult::TakeOverClaude(actual)) if actual == target
    ));
}

#[test]
fn live_claude_row_is_labeled_and_closed_claude_cannot_take_over() {
    let mut live = make_claude_session("live-row");
    let mut closed = make_claude_session("closed-row");
    live.last_message_time = Utc::now();
    closed.last_message_time = Utc::now() - ChronoDuration::minutes(1);
    let mut picker = SessionPicker::new(vec![live.clone(), closed]);
    picker.set_live_presence_for_test(vec![live_presence("claude:live-row", false)]);

    let rendered = picker
        .render_session_item_lines(&live, false)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("live Claude"), "rendered row: {rendered}");

    picker
        .handle_overlay_key(KeyCode::Down, KeyModifiers::empty())
        .unwrap();
    picker
        .handle_overlay_key(KeyCode::Char('T'), KeyModifiers::empty())
        .unwrap();
    assert!(!picker.claude_takeover_confirmation_active_for_test());
}

#[test]
fn test_current_session_row_is_labeled() {
    let session = make_session("session_self", "self", false, SessionStatus::Active);
    let mut picker = SessionPicker::new(vec![session.clone()]);
    picker.set_current_session_id(Some("session_self".to_string()));

    let lines = picker.render_session_item_lines(&session, false);
    let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");
    assert!(
        text.contains("current"),
        "expected current label, got: {text}"
    );
}

#[test]
fn test_maybe_refresh_live_presence_throttles_and_detects_changes() {
    let session = make_session("session_live", "live", false, SessionStatus::Active);
    let mut picker = SessionPicker::new(vec![session]);
    picker.activate_active_filter();
    // Freshly injected snapshot: refresh is throttled, nothing changes.
    picker.set_live_presence_for_test(vec![live_presence("session_live", true)]);
    assert!(!picker.maybe_refresh_live_presence());
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
        OverlayAction::Selected(PickerResult::SelectedInCurrentTerminal(ids)) => {
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

    let alternate_action = picker
        .handle_overlay_key(KeyCode::Enter, KeyModifiers::CONTROL)
        .unwrap();

    match alternate_action {
        OverlayAction::Selected(PickerResult::SelectedInNewTerminal(ids)) => {
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
        other => panic!("expected alternate selected sessions, got {other:?}"),
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
fn onboarding_external_filter_picks_latest_visible_transcript() {
    let now = Utc::now();

    let mut older = make_session("codex_older", "older", false, SessionStatus::Closed);
    older.source = SessionSource::Codex;
    older.model = Some("gpt-5-codex".to_string());
    older.last_active_at = Some(now - ChronoDuration::minutes(30));
    older.resume_target = ResumeTarget::CodexSession {
        session_id: "codex_older".to_string(),
        session_path: "/tmp/codex_older.jsonl".to_string(),
    };

    let mut newer = make_session("codex_newer", "newer", false, SessionStatus::Closed);
    newer.source = SessionSource::Codex;
    newer.model = Some("gpt-5-codex".to_string());
    newer.last_active_at = Some(now - ChronoDuration::minutes(2));
    newer.resume_target = ResumeTarget::CodexSession {
        session_id: "codex_newer".to_string(),
        session_path: "/tmp/codex_newer.jsonl".to_string(),
    };

    // A non-Codex session that must be filtered out.
    let jcode = make_session("jcode_one", "jcode", false, SessionStatus::Closed);

    let mut picker = SessionPicker::new(vec![older, jcode, newer]);
    picker.activate_external_cli_filter(SessionFilterMode::Codex);

    assert_eq!(picker.visible_session_count(), 2);

    let latest = picker
        .latest_visible_resume_target()
        .expect("latest visible target");
    assert_eq!(
        latest,
        ResumeTarget::CodexSession {
            session_id: "codex_newer".to_string(),
            session_path: "/tmp/codex_newer.jsonl".to_string(),
        }
    );
}

#[test]
fn onboarding_external_filter_with_no_matches_has_no_target() {
    let jcode = make_session("jcode_only", "jcode", false, SessionStatus::Closed);
    let mut picker = SessionPicker::new(vec![jcode]);
    picker.activate_external_cli_filter(SessionFilterMode::ClaudeCode);

    assert_eq!(picker.visible_session_count(), 0);
    assert!(picker.latest_visible_resume_target().is_none());
}

#[test]
fn onboarding_banner_defaults_to_suggested_review() {
    let mut picker = SessionPicker::new(Vec::new());
    picker.activate_onboarding_banner(vec![Line::from("welcome")]);

    assert!(picker.onboarding_banner_active());
    assert!(picker.onboarding_review_recent_project_highlighted());
    assert!(!picker.onboarding_start_new_highlighted());
}

#[test]
fn onboarding_banner_is_action_only() {
    let mut picker = SessionPicker::new(Vec::new());
    picker.activate_onboarding_banner(vec![Line::from("welcome")]);

    assert_eq!(picker.visible_session_count(), 0);
    assert!(picker.onboarding_review_recent_project_highlighted());
}

#[test]
fn onboarding_banner_offers_review_then_new_session() {
    let mut picker = SessionPicker::new(Vec::new());
    picker.activate_onboarding_banner(vec![Line::from("welcome")]);

    // The suggested review is the default top action.
    let action = picker
        .handle_overlay_key(KeyCode::Enter, KeyModifiers::empty())
        .expect("overlay key");
    assert!(matches!(
        action,
        OverlayAction::Selected(PickerResult::ReviewRecentProject)
    ));

    // Down selects the blank-session action.
    picker.next();
    assert!(picker.onboarding_start_new_highlighted());
    let action = picker
        .handle_overlay_key(KeyCode::Enter, KeyModifiers::empty())
        .expect("overlay key");
    assert!(matches!(
        action,
        OverlayAction::Selected(PickerResult::StartNewSession)
    ));

    // There is no session list below the two actions.
    picker.next();
    assert!(picker.onboarding_start_new_highlighted());
    picker.previous();
    assert!(picker.onboarding_review_recent_project_highlighted());
}

#[test]
fn onboarding_banner_renders_prompt_and_both_action_rows() {
    let mut picker = SessionPicker::new(Vec::new());
    picker.activate_onboarding_banner(vec![
        Line::from("Welcome to jcode"),
        Line::from("Choose how to begin."),
    ]);

    let backend = ratatui::backend::TestBackend::new(120, 40);
    let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| picker.render(frame))
        .expect("render onboarding picker");

    let buffer = terminal.backend().buffer().clone();
    let text: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
    let lines = (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert!(
        text.contains("Welcome to jcode"),
        "onboarding prompt should render in the banner: {text:?}"
    );
    assert!(
        text.contains("Start a new session"),
        "start-new row should render in the banner: {text:?}"
    );
    assert!(
        text.contains("Find bugs in what I've been working on"),
        "suggested-review row should render in the banner: {text:?}"
    );
    assert!(
        !text.contains("Sessions"),
        "resume chrome must be absent: {text:?}"
    );
    assert!(
        !text.contains('╭') && !text.contains('╰') && !text.contains('│'),
        "onboarding choice should not render an outer boundary: {lines:#?}"
    );

    let welcome_y = lines
        .iter()
        .position(|line| line.contains("Welcome to jcode"))
        .expect("welcome row");
    let review_y = lines
        .iter()
        .position(|line| line.contains("Find bugs in what I've been working on"))
        .expect("review row");
    let start_y = lines
        .iter()
        .position(|line| line.contains("Start a new session"))
        .expect("start-new row");
    let review_x = lines[review_y]
        .find("Find bugs in what I've been working on")
        .expect("review column");
    let start_x = lines[start_y]
        .find("Start a new session")
        .expect("start-new column");

    assert!(
        welcome_y < review_y,
        "welcome copy should introduce the centered suggested prompt: {lines:#?}"
    );
    assert!(
        review_y.abs_diff(buffer.area.height as usize / 2) <= 1,
        "suggested prompt should be vertically centered: {lines:#?}"
    );
    assert!(
        review_x < 50,
        "suggested prompt should span the visual center: {lines:#?}"
    );
    assert!(
        start_y >= buffer.area.height as usize - 3 && start_x >= 95,
        "blank-session action should stay secondary in the bottom-right: {lines:#?}"
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

/// Build a session with many short user/assistant turns so the preview overflows
/// a small viewport (used to exercise the preview scrollbar + sticky header).
fn make_session_with_many_turns(id: &str, turns: usize) -> SessionInfo {
    let mut session = make_session(id, id, false, SessionStatus::Closed);
    let mut preview = Vec::new();
    for i in 0..turns {
        preview.push(PreviewMessage {
            role: "user".to_string(),
            content: format!("user prompt number {i}"),
            tool_calls: Vec::new(),
            tool_data: None,
            timestamp: None,
        });
        preview.push(PreviewMessage {
            role: "assistant".to_string(),
            content: format!("assistant reply number {i}"),
            tool_calls: Vec::new(),
            tool_data: None,
            timestamp: None,
        });
    }
    session.first_user_prompt = preview.first().map(|m| m.content.clone());
    session.messages_preview = preview;
    session
}

fn buffer_text(picker: &mut SessionPicker, w: u16, h: u16) -> String {
    let backend = ratatui::backend::TestBackend::new(w, h);
    let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| picker.render(frame))
        .expect("render picker");
    let buffer = terminal.backend().buffer().clone();
    buffer.content().iter().map(|cell| cell.symbol()).collect()
}

// ---------------------------------------------------------------------------
// Developer benchmarks: profile the operations exercised by the `/resume`
// overlay. These are `#[ignore]`d so they never run in CI; run them with:
//
//   cargo test -p jcode-tui --lib --release -- --ignored --nocapture benchmark_resume_op
//
// They print human-readable timing lines to stderr. They use synthetic
// sessions so they are deterministic and independent of the user's session
// store.
// ---------------------------------------------------------------------------

/// Build a synthetic preview message list that mimics a realistic conversation:
/// alternating user prompts and multi-paragraph markdown assistant replies. The
/// assistant content includes markdown (headers, lists, code) so it exercises
/// the same markdown render + wrap path as the real preview.
fn bench_preview_messages(turns: usize, assistant_paragraphs: usize) -> Vec<PreviewMessage> {
    let mut preview = Vec::with_capacity(turns * 2);
    for turn in 0..turns {
        preview.push(PreviewMessage {
            role: "user".to_string(),
            content: format!(
                "Prompt {turn}: can you refactor the session picker so that the preview \
                 pane does not rebuild and re-wrap every line on every single frame?"
            ),
            tool_calls: Vec::new(),
            tool_data: None,
            timestamp: None,
        });

        let mut body = String::new();
        body.push_str(&format!("## Response {turn}\n\n"));
        for para in 0..assistant_paragraphs {
            body.push_str(&format!(
                "Here is paragraph {para} of a longer answer that wraps across several \
                 terminal columns and therefore costs real work to lay out. It mentions \
                 `render_preview`, `wrap_lines`, and the scroll offset so the markdown \
                 renderer has inline code spans to style.\n\n"
            ));
            body.push_str("- a bullet point that also needs wrapping and styling\n");
            body.push_str("- another bullet with `inline_code` to style\n\n");
        }
        body.push_str("```rust\nlet scroll = self.scroll_offset as usize; // cached?\n```\n");
        preview.push(PreviewMessage {
            role: "assistant".to_string(),
            content: body,
            tool_calls: Vec::new(),
            tool_data: None,
            timestamp: None,
        });
    }
    preview
}

/// A session whose preview is large enough to overflow the viewport and require
/// scrolling (the case the user reported as slow).
fn bench_large_session(id: &str, turns: usize, assistant_paragraphs: usize) -> SessionInfo {
    let mut session = make_session(id, id, false, SessionStatus::Closed);
    let preview = bench_preview_messages(turns, assistant_paragraphs);
    session.first_user_prompt = preview.first().map(|m| m.content.clone());
    session.estimated_tokens = 4_000 * turns;
    session.message_count = turns * 2;
    session.user_message_count = turns;
    session.assistant_message_count = turns;
    session.messages_preview = preview;
    session
}

fn bench_render_full(picker: &mut SessionPicker, w: u16, h: u16) -> std::time::Duration {
    let backend = ratatui::backend::TestBackend::new(w, h);
    let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
    let start = std::time::Instant::now();
    terminal
        .draw(|frame| picker.render(frame))
        .expect("render picker");
    start.elapsed()
}

fn bench_render_preview_only(picker: &mut SessionPicker, area: Rect) -> std::time::Duration {
    // Render into a backend sized exactly to the area, placing it at the origin
    // (the preview/list rendering only depends on width/height, not x/y).
    let area = Rect::new(0, 0, area.width, area.height);
    let backend = ratatui::backend::TestBackend::new(area.width, area.height);
    let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
    let start = std::time::Instant::now();
    terminal
        .draw(|frame| picker.render_preview(frame, area))
        .expect("render preview");
    start.elapsed()
}

fn bench_render_list_only(picker: &mut SessionPicker, area: Rect) -> std::time::Duration {
    let area = Rect::new(0, 0, area.width, area.height);
    let backend = ratatui::backend::TestBackend::new(area.width, area.height);
    let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
    let start = std::time::Instant::now();
    terminal
        .draw(|frame| picker.render_session_list(frame, area))
        .expect("render list");
    start.elapsed()
}

fn bench_median(mut samples: Vec<std::time::Duration>) -> std::time::Duration {
    samples.sort();
    samples[samples.len() / 2]
}

/// Profile the cost of a single preview-scroll frame. This is the operation the
/// user reported as slow: after scrolling, every frame rebuilds + re-wraps the
/// entire preview. We render once to warm any lazy state, then time repeated
/// scroll-and-render ticks and attribute time to the preview vs the (unchanged)
/// session list.
#[test]
#[ignore = "developer benchmark: profiles /resume preview scroll frame cost"]
fn benchmark_resume_op_preview_scroll_frame_cost() {
    const W: u16 = 120;
    const H: u16 = 40;
    let main_area = Rect::new(0, 0, W, H);
    // Mirrors render(): list = 40%, preview = 60% of the width.
    let list_area = Rect::new(0, 0, (W as f32 * 0.40) as u16, H);
    let preview_area = Rect::new(list_area.width, 0, W - list_area.width, H);

    for &(turns, paras) in &[(20usize, 2usize), (80, 3), (200, 4)] {
        let session = bench_large_session("scroll_bench", turns, paras);
        let preview_len = session.messages_preview.len();
        let mut picker = SessionPicker::new(vec![session]);
        picker.focus = PaneFocus::Preview;

        // Warm render (auto-scrolls to bottom, builds wrap state once).
        let _ = bench_render_full(&mut picker, W, H);

        const ITERS: usize = 60;
        let mut full_samples = Vec::with_capacity(ITERS);
        let mut preview_samples = Vec::with_capacity(ITERS);
        let mut list_samples = Vec::with_capacity(ITERS);
        for i in 0..ITERS {
            // Alternate scroll direction so we exercise both bounds.
            if i % 2 == 0 {
                picker.scroll_preview_up(1);
            } else {
                picker.scroll_preview_down(1);
            }
            full_samples.push(bench_render_full(&mut picker, W, H));
            preview_samples.push(bench_render_preview_only(&mut picker, preview_area));
            list_samples.push(bench_render_list_only(&mut picker, list_area));
        }

        let full = bench_median(full_samples);
        let preview = bench_median(preview_samples);
        let list = bench_median(list_samples);
        eprintln!(
            "preview scroll frame: turns={turns} paras={paras} preview_msgs={preview_len} \
             area={}x{} | full_frame={:>6.0}us preview_only={:>6.0}us list_only={:>6.0}us \
             (preview is {:.0}% of frame)",
            main_area.width,
            main_area.height,
            full.as_nanos() as f64 / 1000.0,
            preview.as_nanos() as f64 / 1000.0,
            list.as_nanos() as f64 / 1000.0,
            preview.as_nanos() as f64 / full.as_nanos().max(1) as f64 * 100.0,
        );
    }
}

/// Profile how list rendering scales with the number of sessions. Because
/// `render_session_list` rebuilds a `ListItem` for *every* session each frame
/// (not just the visible window), this should grow ~linearly with N even though
/// only ~H rows are visible. Relevant to scroll because the list is redrawn on
/// every preview-scroll frame too.
#[test]
#[ignore = "developer benchmark: profiles /resume session-list render scaling vs N"]
fn benchmark_resume_op_list_render_scaling() {
    const W: u16 = 48;
    const H: u16 = 40;
    let list_area = Rect::new(0, 0, W, H);

    for &n in &[50usize, 200, 1000, 3000] {
        let sessions: Vec<SessionInfo> = (0..n)
            .map(|i| {
                make_session(
                    &format!("scale_{i}"),
                    &format!("session {i}"),
                    false,
                    SessionStatus::Closed,
                )
            })
            .collect();
        let mut picker = SessionPicker::new(sessions);
        picker.focus = PaneFocus::Sessions;
        let _ = bench_render_list_only(&mut picker, list_area);

        const ITERS: usize = 30;
        let mut samples = Vec::with_capacity(ITERS);
        for _ in 0..ITERS {
            samples.push(bench_render_list_only(&mut picker, list_area));
        }
        let m = bench_median(samples);
        eprintln!(
            "list render scaling: N={n:>5} visible_rows~={} | list_render={:>7.0}us \
             ({:.1}us/session)",
            H.saturating_sub(2),
            m.as_nanos() as f64 / 1000.0,
            m.as_nanos() as f64 / 1000.0 / n as f64,
        );
    }
}

/// Profile a search keystroke (`rebuild_items` + the cached search narrowing)
/// as the query grows, plus the cost of clearing the query (the non-prefix /
/// backspace path that cannot reuse the narrowing cache).
#[test]
#[ignore = "developer benchmark: profiles /resume search keystroke cost"]
fn benchmark_resume_op_search_keystroke() {
    for &n in &[200usize, 1000, 3000] {
        let sessions: Vec<SessionInfo> = (0..n)
            .map(|i| {
                make_session(
                    &format!("search_{i}"),
                    &format!("session about topic {} number {i}", i % 17),
                    false,
                    SessionStatus::Closed,
                )
            })
            .collect();
        let mut picker = SessionPicker::new(sessions);

        // Progressive typing: each keystroke appends one char and rebuilds.
        let query = "session about topic 3";
        let mut typed = String::new();
        let mut keystroke_samples = Vec::new();
        for ch in query.chars() {
            typed.push(ch);
            picker.search_query = typed.clone();
            picker.search_active = true;
            let start = std::time::Instant::now();
            picker.rebuild_items();
            keystroke_samples.push(start.elapsed());
        }
        let typed_median = bench_median(keystroke_samples.clone());
        let typed_worst = keystroke_samples.iter().copied().max().unwrap();

        // Clearing the search (full re-scan, no narrowing cache reuse).
        picker.search_query.clear();
        picker.search_active = false;
        let clear_start = std::time::Instant::now();
        picker.rebuild_items();
        let clear_elapsed = clear_start.elapsed();

        eprintln!(
            "search keystroke: N={n:>5} | per_keystroke_median={:>6.0}us worst={:>6.0}us \
             clear_query={:>6.0}us",
            typed_median.as_nanos() as f64 / 1000.0,
            typed_worst.as_nanos() as f64 / 1000.0,
            clear_elapsed.as_nanos() as f64 / 1000.0,
        );
    }
}

/// Profile navigating the session list (next/previous) followed by a re-render,
/// across list sizes. Navigation resets preview scroll and triggers a full
/// re-render of both panes.
#[test]
#[ignore = "developer benchmark: profiles /resume list navigation frame cost"]
fn benchmark_resume_op_nav_frame_cost() {
    const W: u16 = 120;
    const H: u16 = 40;

    for &n in &[50usize, 500, 2000] {
        let sessions: Vec<SessionInfo> = (0..n)
            .map(|i| bench_large_session(&format!("nav_{i}"), 6, 2))
            .collect();
        let mut picker = SessionPicker::new(sessions);
        picker.focus = PaneFocus::Sessions;
        let _ = bench_render_full(&mut picker, W, H);

        const ITERS: usize = 40;
        let mut samples = Vec::with_capacity(ITERS);
        for i in 0..ITERS {
            if i % 2 == 0 {
                picker.next();
            } else {
                picker.previous();
            }
            samples.push(bench_render_full(&mut picker, W, H));
        }
        let m = bench_median(samples);
        eprintln!(
            "nav frame: N={n:>5} | nav+full_render_median={:>7.0}us",
            m.as_nanos() as f64 / 1000.0,
        );
    }
}

/// Profile constructing the picker (`new`) and the initial `rebuild_items`
/// across list sizes, isolating the non-IO construction cost that runs
/// synchronously when `/resume` opens.
#[test]
#[ignore = "developer benchmark: profiles /resume picker construction cost vs N"]
fn benchmark_resume_op_construction_cost() {
    for &n in &[200usize, 1000, 5000] {
        let sessions: Vec<SessionInfo> = (0..n)
            .map(|i| {
                make_session(
                    &format!("ctor_{i}"),
                    &format!("session {i}"),
                    false,
                    SessionStatus::Closed,
                )
            })
            .collect();

        const ITERS: usize = 20;
        let mut samples = Vec::with_capacity(ITERS);
        for _ in 0..ITERS {
            let clone = sessions.clone();
            let start = std::time::Instant::now();
            let _picker = SessionPicker::new(clone);
            samples.push(start.elapsed());
        }
        let m = bench_median(samples);
        eprintln!(
            "construction: N={n:>5} | new()+rebuild_items_median={:>7.0}us ({:.2}us/session)",
            m.as_nanos() as f64 / 1000.0,
            m.as_nanos() as f64 / 1000.0 / n as f64,
        );
    }
}

/// Any of the native scrollbar thumb glyphs (see `render_native_scrollbar`).
fn contains_scrollbar_glyph(text: &str) -> bool {
    text.contains('•') || text.contains('╷') || text.contains('╵') || text.contains('│')
}

#[test]
fn test_preview_pane_shows_scrollbar_when_overflowing() {
    let session = make_session_with_many_turns("preview_scroll", 60);
    let mut picker = SessionPicker::new(vec![session]);
    picker.focus = PaneFocus::Preview;

    // Small height so the long preview overflows and needs a scrollbar.
    let text = buffer_text(&mut picker, 100, 16);
    assert!(
        contains_scrollbar_glyph(&text),
        "preview scrollbar glyph should render when content overflows:\n{text}"
    );
}

#[test]
fn test_session_list_shows_scrollbar_when_overflowing() {
    // Many sessions so the left list overflows a short viewport.
    let sessions: Vec<SessionInfo> = (0..40)
        .map(|i| {
            make_session(
                &format!("list_scroll_{i}"),
                &format!("s{i}"),
                false,
                SessionStatus::Closed,
            )
        })
        .collect();
    let mut picker = SessionPicker::new(sessions);
    picker.focus = PaneFocus::Sessions;

    let text = buffer_text(&mut picker, 100, 16);
    assert!(
        contains_scrollbar_glyph(&text),
        "session list scrollbar glyph should render when list overflows:\n{text}"
    );
}

#[test]
fn test_preview_sticky_prompt_header_appears_after_scrolling() {
    let session = make_session_with_many_turns("sticky_header", 60);
    let mut picker = SessionPicker::new(vec![session]);
    picker.focus = PaneFocus::Preview;

    // First render auto-scrolls to the bottom; the topmost prompts are off-screen,
    // so a dimmed "N› ..." sticky header should pin a prior prompt at the top of
    // the preview's content area.
    let w = 100u16;
    let h = 16u16;
    let backend = ratatui::backend::TestBackend::new(w, h);
    let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| picker.render(frame))
        .expect("render picker");
    let buffer = terminal.backend().buffer().clone();

    // The preview pane occupies the right 60% of the width; its inner content
    // starts just inside the rounded border. Read the first inner content row and
    // confirm it carries the "N›" sticky-header marker.
    let preview_inner_x = (w as f32 * 0.40) as u16 + 1;
    let header_row: String = (preview_inner_x..w.saturating_sub(1))
        .map(|x| buffer[(x, 1)].symbol())
        .collect();
    assert!(
        header_row.contains('›'),
        "sticky prompt header should pin a numbered prompt at the top of the preview:\n\
         row={header_row:?}"
    );
    // The header marker is a prompt number followed by the chevron.
    assert!(
        header_row
            .trim_start()
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit()),
        "sticky header should begin with a prompt number:\nrow={header_row:?}"
    );
}

#[test]
fn test_preview_sticky_prompt_header_survives_async_preview_load() {
    // Regression: when the selected session's transcript is still loading on a
    // background thread, the first render only shows a "Loading…" placeholder
    // (max_scroll == 0). The auto-scroll flag must NOT be consumed on that
    // placeholder frame; otherwise the populated transcript stays pinned at the
    // top and the sticky "previous prompt" header never appears (it only renders
    // when scrolled past a prompt). This reproduces the intermittent "/resume
    // sometimes doesn't show your last prompt at the top" bug.
    let mut session = make_session_with_many_turns("async_sticky", 60);
    let full_preview = std::mem::take(&mut session.messages_preview);
    session.first_user_prompt = full_preview.first().map(|m| m.content.clone());

    let mut picker = SessionPicker::new(vec![session.clone()]);
    picker.focus = PaneFocus::Preview;

    // Simulate an in-flight background load for the selected session: empty
    // preview + a pending load whose id matches. Keep the sender alive so the
    // receiver reports `Empty` (still loading) rather than `Disconnected`.
    let (tx, rx) = std::sync::mpsc::channel::<Option<Vec<PreviewMessage>>>();
    picker.pending_preview_load = Some(PendingSessionPreviewLoad {
        session_id: "async_sticky".to_string(),
        receiver: rx,
    });

    let w = 100u16;
    let h = 16u16;
    let render = |picker: &mut SessionPicker| {
        let backend = ratatui::backend::TestBackend::new(w, h);
        let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| picker.render(frame))
            .expect("render picker");
        terminal.backend().buffer().clone()
    };

    // First frame: transcript still loading -> placeholder shown, auto-scroll
    // must remain armed.
    let _ = render(&mut picker);
    assert!(
        picker.auto_scroll_preview,
        "auto-scroll should stay armed while the preview is still loading"
    );

    // The background load completes: deliver the real transcript exactly the way
    // `poll_preview_load` would (drop the channel + populate the preview).
    drop(tx);
    picker.pending_preview_load = None;
    picker.apply_session_preview("async_sticky", full_preview);

    // Second frame: now that content is present we snap to the bottom and the
    // top prompts scroll off-screen, so the sticky header should pin a prompt.
    let buffer = render(&mut picker);
    assert!(
        picker.scroll_offset > 0,
        "preview should auto-scroll to the bottom once content loads, got {}",
        picker.scroll_offset
    );

    let preview_inner_x = (w as f32 * 0.40) as u16 + 1;
    let header_row: String = (preview_inner_x..w.saturating_sub(1))
        .map(|x| buffer[(x, 1)].symbol())
        .collect();
    assert!(
        header_row.contains('›'),
        "sticky prompt header should appear after an async preview load:\n\
         row={header_row:?}"
    );
    assert!(
        header_row
            .trim_start()
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit()),
        "sticky header should begin with a prompt number:\nrow={header_row:?}"
    );
}

#[test]
fn preview_render_cache_is_reused_across_scroll_and_rebuilt_on_selection_change() {
    // The preview pane caches its fully-wrapped content keyed by a content hash
    // and pane geometry, so scrolling reuses the cache instead of re-rendering
    // and re-wrapping every line. Navigating to another session must invalidate
    // it (different content hash).
    let a = make_session_with_many_turns("cache_a", 60);
    let b = make_session_with_many_turns("cache_b", 60);
    let mut picker = SessionPicker::new(vec![a, b]);
    picker.focus = PaneFocus::Preview;

    let w = 100u16;
    let h = 16u16;
    let render = |picker: &mut SessionPicker| {
        let backend = ratatui::backend::TestBackend::new(w, h);
        let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| picker.render(frame))
            .expect("render picker");
    };

    // First render builds the cache for the selected session.
    render(&mut picker);
    let key_after_build = picker
        .preview_cache
        .as_ref()
        .map(|c| c.key.clone())
        .expect("preview cache built on first render");
    let wrapped_len = picker
        .preview_cache
        .as_ref()
        .map(|c| c.wrapped_lines.len())
        .unwrap();
    assert!(wrapped_len > h as usize, "preview should overflow viewport");

    // Scrolling several times must not change the cache key (content unchanged):
    // the cache is reused and only the scroll offset + visible slice move.
    for _ in 0..5 {
        picker.scroll_preview_up(1);
        render(&mut picker);
        let key_now = picker
            .preview_cache
            .as_ref()
            .map(|c| c.key.clone())
            .unwrap();
        assert!(
            key_now == key_after_build,
            "scrolling must reuse the cached wrapped preview"
        );
    }

    // Navigating to a different session changes the content hash, so the cache
    // is rebuilt for the new selection.
    picker.next();
    render(&mut picker);
    let key_after_nav = picker
        .preview_cache
        .as_ref()
        .map(|c| c.key.clone())
        .unwrap();
    assert!(
        key_after_nav != key_after_build,
        "selecting a different session must invalidate the preview cache"
    );
}

#[test]
fn preview_visible_slice_matches_scroll_position() {
    // The renderer materializes only the visible window of wrapped lines. Confirm
    // that scrolling actually changes what is drawn (i.e. the slice tracks the
    // scroll offset rather than always showing the bottom).
    let session = make_session_with_many_turns("slice", 60);
    let mut picker = SessionPicker::new(vec![session]);
    picker.focus = PaneFocus::Preview;

    let w = 100u16;
    let h = 16u16;
    let render_text = |picker: &mut SessionPicker| -> String { buffer_text(picker, w, h) };

    // First render auto-scrolls to the bottom.
    let bottom = render_text(&mut picker);
    let bottom_scroll = picker.scroll_offset;
    assert!(bottom_scroll > 0, "long preview should be scrolled down");

    // Scroll to the very top; the rendered content must differ from the bottom.
    picker.scroll_preview_up(bottom_scroll);
    let top = render_text(&mut picker);
    assert_eq!(picker.scroll_offset, 0);
    assert_ne!(
        top, bottom,
        "scrolling to the top should render different content than the bottom"
    );
}

#[test]
fn test_reseed_grouped_preserves_selection_and_search() {
    // Build a picker with several sessions, then simulate the user navigating to
    // a specific session and typing a search. A background refresh that reseeds
    // the same data must keep the highlighted session and the active search.
    let sessions: Vec<SessionInfo> = (0..6)
        .map(|i| {
            make_session(
                &format!("session_reseed_{i}"),
                &format!("reseed{i}"),
                false,
                SessionStatus::Closed,
            )
        })
        .collect();
    let mut picker = SessionPicker::new(sessions.clone());

    // Move selection to the third visible session.
    picker.next();
    picker.next();
    let selected_before = picker
        .selected_session()
        .map(|s| s.id.clone())
        .expect("a session should be selected");

    // Activate a search that matches only one session id.
    picker.search_query = "reseed4".to_string();
    picker.search_active = true;
    picker.focus = PaneFocus::Preview;
    picker.rebuild_items();
    let search_selected = picker
        .selected_session()
        .map(|s| s.id.clone())
        .expect("search should leave a selection");

    // Reseed with the same data (as the async refresh would).
    picker.reseed_grouped(Vec::new(), sessions);

    // Search query, search mode, focus, and the matched selection survive.
    assert_eq!(picker.search_query, "reseed4");
    assert!(picker.search_active);
    assert_eq!(picker.focus, PaneFocus::Preview);
    assert_eq!(
        picker.selected_session().map(|s| s.id.clone()),
        Some(search_selected)
    );

    // Clearing the search restores the full list; the originally highlighted
    // session is still resolvable in the reseeded data.
    picker.search_query.clear();
    picker.search_active = false;
    picker.rebuild_items();
    assert!(
        picker
            .visible_session_iter_for_test()
            .any(|s| s.id == selected_before),
        "previously selected session should still be present after reseed"
    );
}

#[test]
fn test_reseed_grouped_keeps_selection_when_list_changes() {
    // The highlighted session must follow its id even when the refreshed list has
    // a different order / additional sessions (the realistic refresh case).
    let initial: Vec<SessionInfo> = (0..4)
        .map(|i| {
            make_session(
                &format!("session_keep_{i}"),
                &format!("keep{i}"),
                false,
                SessionStatus::Closed,
            )
        })
        .collect();
    let mut picker = SessionPicker::new(initial.clone());
    picker.next(); // select session_keep_1
    let target = picker
        .selected_session()
        .map(|s| s.id.clone())
        .expect("selection");

    // Refreshed list: prepend a brand-new session and keep the rest, changing
    // indices so a naive index-based selection would drift.
    let mut refreshed = vec![make_session(
        "session_keep_new",
        "keepnew",
        false,
        SessionStatus::Closed,
    )];
    refreshed.extend(initial);

    picker.reseed_grouped(Vec::new(), refreshed);

    assert_eq!(
        picker.selected_session().map(|s| s.id.clone()),
        Some(target),
        "selection should follow the session id across a reordered refresh"
    );
}

#[test]
fn test_search_mode_ctrl_j_k_navigate_session_list() {
    let mut newer = make_session("session_newer", "newer", false, SessionStatus::Closed);
    let mut older = make_session("session_older", "older", false, SessionStatus::Closed);
    newer.last_message_time = Utc::now();
    older.last_message_time = Utc::now() - ChronoDuration::minutes(1);
    let mut picker = SessionPicker::new(vec![older, newer]);

    // Enter search mode (both visible sessions still match the empty query).
    picker
        .handle_overlay_key(KeyCode::Char('/'), KeyModifiers::empty())
        .unwrap();
    assert!(picker.search_active);
    let first = picker
        .selected_session()
        .map(|s| s.id.clone())
        .expect("a session is selected on entering search");

    // Ctrl+J moves down the list without typing 'j' into the query.
    picker
        .handle_overlay_key(KeyCode::Char('j'), KeyModifiers::CONTROL)
        .unwrap();
    assert!(
        picker.search_query.is_empty(),
        "Ctrl+J must not type into search"
    );
    let second = picker
        .selected_session()
        .map(|s| s.id.clone())
        .expect("a session is selected after Ctrl+J");
    assert_ne!(first, second, "Ctrl+J should move the selection down");

    // Ctrl+K moves back up to the original selection.
    picker
        .handle_overlay_key(KeyCode::Char('k'), KeyModifiers::CONTROL)
        .unwrap();
    assert!(
        picker.search_query.is_empty(),
        "Ctrl+K must not type into search"
    );
    assert_eq!(
        picker.selected_session().map(|s| s.id.clone()),
        Some(first),
        "Ctrl+K should move the selection back up"
    );
}

#[test]
fn test_search_mode_ctrl_backspace_deletes_word() {
    let mut picker = SessionPicker::new(vec![make_session(
        "session_a",
        "a",
        false,
        SessionStatus::Closed,
    )]);
    picker
        .handle_overlay_key(KeyCode::Char('/'), KeyModifiers::empty())
        .unwrap();
    for c in "hello world".chars() {
        picker
            .handle_overlay_key(KeyCode::Char(c), KeyModifiers::empty())
            .unwrap();
    }
    assert_eq!(picker.search_query, "hello world");

    // Ctrl+Backspace deletes the trailing word.
    picker
        .handle_overlay_key(KeyCode::Backspace, KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(picker.search_query, "hello ");

    // The \u{8} alias some terminals send for Ctrl+Backspace also deletes a word.
    picker
        .handle_overlay_key(KeyCode::Char('\u{8}'), KeyModifiers::empty())
        .unwrap();
    assert_eq!(picker.search_query, "");

    // Plain Backspace still deletes a single character.
    for c in "abc".chars() {
        picker
            .handle_overlay_key(KeyCode::Char(c), KeyModifiers::empty())
            .unwrap();
    }
    picker
        .handle_overlay_key(KeyCode::Backspace, KeyModifiers::empty())
        .unwrap();
    assert_eq!(picker.search_query, "ab");
}

#[test]
fn test_search_mode_ctrl_u_clears_query() {
    let mut picker = SessionPicker::new(vec![make_session(
        "session_a",
        "a",
        false,
        SessionStatus::Closed,
    )]);
    picker
        .handle_overlay_key(KeyCode::Char('/'), KeyModifiers::empty())
        .unwrap();
    for c in "needle".chars() {
        picker
            .handle_overlay_key(KeyCode::Char(c), KeyModifiers::empty())
            .unwrap();
    }
    assert_eq!(picker.search_query, "needle");
    picker
        .handle_overlay_key(KeyCode::Char('u'), KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(picker.search_query, "");
    assert!(
        picker.search_active,
        "Ctrl+U clears text but stays in search"
    );
}

#[test]
fn test_current_dir_highlight_marks_matching_sessions() {
    let mut same = make_session("same_dir", "same", false, SessionStatus::Closed);
    same.working_dir = Some("/home/jeremy/project".to_string());
    let mut other = make_session("other_dir", "other", false, SessionStatus::Closed);
    other.working_dir = Some("/home/jeremy/elsewhere".to_string());

    let mut picker = SessionPicker::new(vec![same.clone(), other.clone()]);
    // Trailing slash on the current dir should still match (normalization).
    picker.set_current_dir(Some("/home/jeremy/project/".to_string()));

    assert!(picker.session_in_current_dir(&same));
    assert!(!picker.session_in_current_dir(&other));

    let rows = picker.render_session_item_lines(&same, false);
    let text: String = rows.iter().map(line_text).collect::<Vec<_>>().join("\n");
    assert!(
        text.contains("here"),
        "matching session should show the `here` marker: {text}"
    );

    // The marker and directory line should be styled with the same-dir accent
    // green so the highlight is visually distinct, not just present as text.
    let same_dir_color = rgb(120, 200, 140);
    let marker_styled_green = rows.iter().any(|line| {
        line.spans
            .iter()
            .any(|span| span.content.contains("here") && span.style.fg == Some(same_dir_color))
    });
    assert!(
        marker_styled_green,
        "`here` marker should be rendered in the same-dir accent color"
    );

    let other_rows = picker.render_session_item_lines(&other, false);
    let other_text: String = other_rows
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !other_text.contains("▸ here"),
        "non-matching session should not show the marker: {other_text}"
    );
}

#[test]
fn test_current_dir_highlight_absent_without_current_dir() {
    let mut session = make_session("s", "s", false, SessionStatus::Closed);
    session.working_dir = Some("/home/jeremy/project".to_string());
    let picker = SessionPicker::new(vec![session.clone()]);
    // No current_dir set: nothing is highlighted.
    assert!(!picker.session_in_current_dir(&session));
}

#[test]
fn highlight_spans_marks_query_occurrences() {
    let base = Style::default().fg(Color::White);
    let tokens = vec!["resume".to_string()];
    let spans = SessionPicker::highlight_spans("Fix the Resume bug", &tokens, base);
    let combined: String = spans.iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(combined, "Fix the Resume bug");

    let highlighted: Vec<&str> = spans
        .iter()
        .filter(|s| s.style.add_modifier.contains(Modifier::BOLD))
        .map(|s| s.content.as_ref())
        .collect();
    assert_eq!(
        highlighted,
        vec!["Resume"],
        "match should be highlighted case-insensitively"
    );
}

#[test]
fn highlight_spans_marks_each_token_independently() {
    // Multi-word queries highlight every token (order independent), matching the
    // AND-token filter semantics.
    let base = Style::default().fg(Color::White);
    let tokens = vec!["resume".to_string(), "bug".to_string()];
    let spans = SessionPicker::highlight_spans("Fix the Resume bug now", &tokens, base);
    let combined: String = spans.iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(combined, "Fix the Resume bug now");
    let highlighted: Vec<&str> = spans
        .iter()
        .filter(|s| s.style.add_modifier.contains(Modifier::BOLD))
        .map(|s| s.content.as_ref())
        .collect();
    assert_eq!(
        highlighted,
        vec!["Resume", "bug"],
        "every token should be highlighted"
    );
}

#[test]
fn highlight_spans_without_query_returns_single_span() {
    let base = Style::default().fg(Color::White);
    let spans = SessionPicker::highlight_spans("hello world", &[], base);
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].content.as_ref(), "hello world");
}

#[test]
fn search_highlights_matching_title_in_rendered_rows() {
    let session = make_session("abc", "deploy pipeline", false, SessionStatus::Closed);
    let mut picker = SessionPicker::new(vec![session]);
    // make_session sets title = "Test session"; search a substring of the title.
    picker.search_query = "sess".to_string();
    let rows = picker.render_session_item_lines(picker.all_sessions.first().unwrap(), false);
    let has_highlight = rows[0]
        .spans
        .iter()
        .any(|s| s.content.as_ref() == "sess" && s.style.add_modifier.contains(Modifier::BOLD));
    assert!(
        has_highlight,
        "query substring in title should be highlighted"
    );
}

#[test]
fn search_highlights_match_in_preview_and_scrolls_to_it() {
    // A long transcript where a distinctive term ("flibbertigibbet") appears only
    // in an early message. Searching for it should both highlight the match in the
    // preview pane and scroll the preview to the match rather than to the bottom.
    let mut session = make_session_with_many_turns("long", 60);
    // Inject the unique term near the top of the transcript.
    session.messages_preview[4].content = "the magic flibbertigibbet token".to_string();
    let mut picker = SessionPicker::new(vec![session]);
    picker.focus = PaneFocus::Preview;

    let w = 100u16;
    let h = 16u16;

    // Baseline: no search -> auto-scrolls to bottom.
    let _ = buffer_text(&mut picker, w, h);
    let bottom_scroll = picker.scroll_offset;
    assert!(
        bottom_scroll > 0,
        "long preview should scroll to bottom by default"
    );

    // Now search for the unique early term. Reset auto-scroll like a keystroke would.
    picker.search_query = "flibbertigibbet".to_string();
    picker.auto_scroll_preview = true;
    let text = buffer_text(&mut picker, w, h);

    // The preview should have scrolled to the match (near the top), not the bottom.
    assert!(
        picker.scroll_offset < bottom_scroll,
        "preview should scroll up to the match (got {}, bottom was {})",
        picker.scroll_offset,
        bottom_scroll
    );
    assert!(
        text.contains("flibbertigibbet"),
        "matched term should be visible in the preview after scrolling"
    );

    // The match should be highlighted (bold) in the cached wrapped lines.
    let highlighted = picker
        .preview_cache
        .as_ref()
        .expect("preview cache built")
        .wrapped_lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .any(|s| {
            s.content.to_lowercase().contains("flibbertigibbet")
                && s.style.add_modifier.contains(Modifier::BOLD)
        });
    assert!(
        highlighted,
        "matched term in preview body should be highlighted"
    );
}

#[test]
fn preview_without_search_has_no_highlight_and_scrolls_to_bottom() {
    let session = make_session_with_many_turns("nosrch", 60);
    let mut picker = SessionPicker::new(vec![session]);
    picker.focus = PaneFocus::Preview;
    let _ = buffer_text(&mut picker, 100, 16);
    assert!(
        picker.scroll_offset > 0,
        "should scroll to bottom without search"
    );
    let any_highlight = picker
        .preview_cache
        .as_ref()
        .expect("preview cache built")
        .wrapped_lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .any(|s| {
            s.style.add_modifier.contains(Modifier::BOLD) && s.style.fg == Some(rgb(255, 214, 90))
        });
    assert!(
        !any_highlight,
        "no search means no highlight color in preview"
    );
}
