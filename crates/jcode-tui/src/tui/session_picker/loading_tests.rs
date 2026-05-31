use super::*;
use std::path::Path;

struct EnvVarGuard {
    key: &'static str,
    prev: Option<std::ffi::OsString>,
}

impl EnvVarGuard {
    fn set_path(key: &'static str, value: &std::path::Path) -> Self {
        let prev = std::env::var_os(key);
        crate::env::set_var(key, value);
        Self { key, prev }
    }

    fn set_str(key: &'static str, value: &str) -> Self {
        let prev = std::env::var_os(key);
        crate::env::set_var(key, value);
        Self { key, prev }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(prev) = self.prev.take() {
            crate::env::set_var(self.key, prev);
        } else {
            crate::env::remove_var(self.key);
        }
    }
}

fn write_picker_snapshot(path: &Path, has_messages: bool) {
    let body = if has_messages {
        "{\"messages\":[{\"role\":\"user\"}]}"
    } else {
        "{\"messages\": []}"
    };
    std::fs::write(path, body).expect("write picker snapshot");
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
fn cached_grouped_sessions_round_trip_from_disk() {
    let _env_lock = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("temp dir");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let _scan_limit = EnvVarGuard::set_str("JCODE_SESSION_PICKER_MAX_SESSIONS", "100");
    let _include_saved = EnvVarGuard::set_str("JCODE_SESSION_PICKER_INCLUDE_OLD_SAVED", "0");

    let sessions_dir = temp.path().join("sessions");
    std::fs::create_dir_all(&sessions_dir).expect("create sessions dir");
    let now = chrono::Utc::now();
    let session = SessionInfo {
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
        messages_preview: Vec::new(),
        search_index: "cache test".to_string(),
        server_name: None,
        server_icon: None,
        source: SessionSource::Jcode,
        resume_target: ResumeTarget::JcodeSession {
            session_id: "session_cache_test_1770000000000".to_string(),
        },
        external_path: None,
    };
    let cache = GroupedSessionListDiskCache {
        version: SESSION_LIST_DISK_CACHE_VERSION,
        generated_at: now,
        sessions_dir,
        scan_limit: session_scan_limit(),
        include_old_saved_sessions: include_old_saved_sessions_on_initial_load(),
        server_groups: Vec::new(),
        orphan_sessions: vec![session],
    };

    let path = session_list_disk_cache_path().expect("cache path");
    crate::storage::write_json_fast(&path, &cache).expect("write cache");

    let (_groups, orphans) = load_cached_sessions_grouped().expect("load cache");
    assert_eq!(orphans.len(), 1);
    assert_eq!(orphans[0].id, "session_cache_test_1770000000000");
    assert_eq!(orphans[0].title, "Cache test");
}

#[test]
fn load_sessions_includes_claude_code_sessions_from_external_home() {
    let _env_lock = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("temp dir");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

    let project_dir = temp.path().join("external/.claude/projects/demo-project");
    std::fs::create_dir_all(&project_dir).expect("create project dir");

    let transcript_path = project_dir.join("claude-session-123.jsonl");
    std::fs::write(
        &transcript_path,
        concat!(
            "{\"type\":\"user\",\"uuid\":\"u1\",\"message\":{\"role\":\"user\",\"content\":\"Investigate the login bug\"}}\n",
            "{\"type\":\"assistant\",\"uuid\":\"a1\",\"parentUuid\":\"u1\",\"message\":{\"role\":\"assistant\",\"content\":\"I can help with that.\"}}\n"
        ),
    )
    .expect("write transcript");

    std::fs::write(
        project_dir.join("sessions-index.json"),
        format!(
            concat!(
                "{{\"version\":1,\"entries\":[",
                "{{\"sessionId\":\"claude-session-123\",",
                "\"fullPath\":\"{}\",",
                "\"firstPrompt\":\"Investigate the login bug\",",
                "\"summary\":\"Investigate the login bug\",",
                "\"messageCount\":2,",
                "\"created\":\"2026-04-04T12:00:00Z\",",
                "\"modified\":\"2026-04-04T12:05:00Z\",",
                "\"projectPath\":\"/tmp/demo-project\"",
                "}}]}}"
            ),
            transcript_path.display()
        ),
    )
    .expect("write index");

    let sessions = load_sessions().expect("load sessions");
    let session = sessions
        .iter()
        .find(|session| {
            matches!(
                session.resume_target,
                ResumeTarget::ClaudeCodeSession { .. }
            )
        })
        .expect("claude session present");

    assert_eq!(session.source, SessionSource::ClaudeCode);
    assert_eq!(session.id, "claude:claude-session-123");
    assert_eq!(session.short_name, "demo-project");
    assert_eq!(session.title, "Investigate the login bug");
    assert_eq!(session.message_count, 2);
    assert_eq!(session.working_dir.as_deref(), Some("/tmp/demo-project"));
}

#[test]
fn load_claude_code_preview_reads_transcript_messages() {
    let _env_lock = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("temp dir");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

    let project_dir = temp.path().join("external/.claude/projects/demo-project");
    std::fs::create_dir_all(&project_dir).expect("create project dir");

    let transcript_path = project_dir.join("claude-session-456.jsonl");
    std::fs::write(
        &transcript_path,
        concat!(
            "{\"type\":\"user\",\"uuid\":\"u1\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"Fix the flaky test\"}]}}\n",
            "{\"type\":\"assistant\",\"uuid\":\"a1\",\"parentUuid\":\"u1\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"I found the race condition\"}]}}\n"
        ),
    )
    .expect("write transcript");

    std::fs::write(
        project_dir.join("sessions-index.json"),
        format!(
            concat!(
                "{{\"version\":1,\"entries\":[",
                "{{\"sessionId\":\"claude-session-456\",",
                "\"fullPath\":\"{}\",",
                "\"firstPrompt\":\"Fix the flaky test\",",
                "\"messageCount\":2,",
                "\"created\":\"2026-04-04T12:00:00Z\",",
                "\"modified\":\"2026-04-04T12:05:00Z\"",
                "}}]}}"
            ),
            transcript_path.display()
        ),
    )
    .expect("write index");

    let preview = load_claude_code_preview("claude-session-456").expect("preview");
    assert_eq!(preview.len(), 2);
    assert_eq!(preview[0].role, "user");
    assert!(preview[0].content.contains("Fix the flaky test"));
    assert_eq!(preview[1].role, "assistant");
    assert!(preview[1].content.contains("I found the race condition"));
}

#[test]
fn load_sessions_includes_modern_codex_sessions() {
    let _env_lock = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("temp dir");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

    let codex_dir = temp.path().join("external/.codex/sessions/2026/04/05");
    std::fs::create_dir_all(&codex_dir).expect("create codex dir");

    let transcript_path = codex_dir.join("rollout-2026-04-05T19-00-00-test.jsonl");
    std::fs::write(
        &transcript_path,
        concat!(
            "{\"timestamp\":\"2026-04-05T19:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"019d-codex-test\",\"timestamp\":\"2026-04-05T18:59:00Z\",\"cwd\":\"/tmp/codex-demo\",\"source\":\"cli\"}}\n",
            "{\"timestamp\":\"2026-04-05T19:00:01Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"# AGENTS.md instructions for /tmp/codex-demo\\n\\n<INSTRUCTIONS>ignored</INSTRUCTIONS>\"}]}}\n",
            "{\"timestamp\":\"2026-04-05T19:00:03Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"Fix the OpenAI usage widget\"}]}}\n",
            "{\"timestamp\":\"2026-04-05T19:00:05Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"I found the issue.\"}]}}\n"
        ),
    )
    .expect("write codex transcript");

    let sessions = load_sessions().expect("load sessions");
    let session = sessions
        .iter()
        .find(|session| matches!(session.resume_target, ResumeTarget::CodexSession { .. }))
        .expect("codex session present");

    assert_eq!(session.source, SessionSource::Codex);
    assert_eq!(session.id, "codex:019d-codex-test");
    assert_eq!(session.title, "Codex session 019d-cod");
    assert_eq!(session.message_count, 0);
    assert_eq!(session.user_message_count, 0);
    assert_eq!(session.assistant_message_count, 0);
    assert_eq!(session.working_dir.as_deref(), Some("/tmp/codex-demo"));
}

#[test]
fn load_codex_preview_preserves_blank_line_between_tool_transcript_and_followup_prose() {
    let temp = tempfile::tempdir().expect("temp dir");
    let transcript_path = temp.path().join("codex-preview.jsonl");
    std::fs::write(
        &transcript_path,
        concat!(
            "{\"timestamp\":\"2026-04-10T19:05:54.536Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"019d-preview-test\",\"timestamp\":\"2026-04-10T19:05:54.536Z\"}}\n",
            "{\"timestamp\":\"2026-04-10T19:05:55.000Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[",
            "{\"type\":\"output_text\",\"text\":\"I’m cleaning up the last leftover warning from the reverted experiment, then I’ll commit the second pass as the debounced large-swarm snapshot optimization.\\n  ✓ batch 3 calls · 174 tok\\n    ✓ apply_patch src/server/swarm.rs (30 lines) · 10 tok\\n    ✓ bash $ cargo fmt --all · 27 tok\\n    ✓ bash $ git add … status broadcasts\"},",
            "{\"type\":\"output_text\",\"text\":\"I landed the second pass as commit 158f6ac, and I’m not stopping there.\"}",
            "]}}\n"
        ),
    )
    .expect("write codex transcript");

    let preview = load_codex_preview_from_path(&transcript_path).expect("preview");
    assert_eq!(preview.len(), 1);
    assert_eq!(preview[0].role, "assistant");
    assert!(
        preview[0].content.contains(
            "✓ bash $ git add … status broadcasts\n\nI landed the second pass as commit 158f6ac"
        ),
        "preview content should preserve a blank line between tool transcript and followup prose: {:?}",
        preview[0].content
    );
}

#[test]
fn load_sessions_prefers_custom_title_over_generated_title() {
    let _env_lock = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("temp dir");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

    let mut session = Session::create_with_id(
        "session_customtitle_1770000000000".to_string(),
        None,
        Some("Generated first prompt".to_string()),
    );
    session.rename_title(Some("Custom release planning".to_string()));
    session.append_stored_message(crate::session::StoredMessage {
        id: "msg1".to_string(),
        role: crate::message::Role::User,
        content: vec![crate::message::ContentBlock::Text {
            text: "please plan the release".to_string(),
            cache_control: None,
        }],
        display_role: None,
        timestamp: None,
        tool_duration_ms: None,
        token_usage: None,
    });
    session.save().expect("save session");
    invalidate_session_list_cache();

    let sessions = load_sessions().expect("load sessions");
    let loaded = sessions
        .iter()
        .find(|session| session.id == "session_customtitle_1770000000000")
        .expect("custom title session present");
    assert_eq!(loaded.title, "Custom release planning");
    assert!(loaded.search_index.contains("custom release planning"));
    assert!(!loaded.search_index.contains("generated first prompt"));
}

#[test]
fn load_sessions_includes_saved_sessions_beyond_scan_limit() {
    let _env_lock = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("temp dir");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let _scan_limit = EnvVarGuard::set_str("JCODE_SESSION_PICKER_MAX_SESSIONS", "50");

    let mut saved_session = Session::create_with_id(
        "session_saved_beyond_scan_limit".to_string(),
        Some("/tmp/saved-beyond-scan".to_string()),
        Some("Saved Beyond Scan".to_string()),
    );
    saved_session.mark_saved(Some("Pinned Session".to_string()));
    saved_session.append_stored_message(crate::session::StoredMessage {
        id: "saved-msg".to_string(),
        role: crate::message::Role::User,
        content: vec![crate::message::ContentBlock::Text {
            text: "keep this bookmarked session visible".to_string(),
            cache_control: None,
        }],
        display_role: None,
        timestamp: None,
        tool_duration_ms: None,
        token_usage: None,
    });
    saved_session.save().expect("save saved session");

    for idx in 0..55 {
        let mut session = Session::create_with_id(
            format!("session_newer_unsaved_{idx:03}"),
            Some(format!("/tmp/newer-unsaved-{idx:03}")),
            Some(format!("Newer Unsaved {idx:03}")),
        );
        session.append_stored_message(crate::session::StoredMessage {
            id: format!("msg-{idx}"),
            role: crate::message::Role::User,
            content: vec![crate::message::ContentBlock::Text {
                text: format!("newer unsaved session {idx:03}"),
                cache_control: None,
            }],
            display_role: None,
            timestamp: None,
            tool_duration_ms: None,
            token_usage: None,
        });
        session.save().expect("save unsaved session");
    }
    invalidate_session_list_cache();

    let sessions = load_sessions().expect("load sessions");
    assert!(
        sessions
            .iter()
            .any(|session| session.id == "session_saved_beyond_scan_limit"),
        "saved sessions should remain visible even when the recency scan limit is full"
    );
}

#[test]
fn saved_metadata_detection_scans_tail_without_full_json_parse() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("session_large_saved.json");
    let large_messages = "x".repeat((super::SAVED_METADATA_TAIL_SCAN_BYTES as usize) + 16_384);
    std::fs::write(
        &path,
        format!(
            r#"{{"id":"session_large_saved","messages":[{{"role":"user","content":"{large_messages}"}}],"saved"  : true}}"#
        ),
    )
    .expect("write session");

    assert!(super::session_snapshot_or_journal_has_saved_metadata(&path));
}

#[test]
fn raw_content_system_reminder_detection_handles_arrays_strings_and_unicode() {
    let raw_string: Box<serde_json::value::RawValue> =
        serde_json::from_str(r#""   <system-reminder>\nlegacy""#).expect("raw string");
    assert!(raw_content_starts_with_system_reminder(&raw_string));

    let raw_content_array: Box<serde_json::value::RawValue> =
        serde_json::from_str(r#"[{"type":"text","text":"<system-reminder>\n# Session Context"}]"#)
            .expect("raw array");
    assert!(raw_content_starts_with_system_reminder(&raw_content_array));

    let long_unicode = "─".repeat(3000);
    let raw_long_tool_result: Box<serde_json::value::RawValue> = serde_json::from_str(&format!(
        r#"[{{"type":"tool_result","content":{}}}]"#,
        serde_json::to_string(&long_unicode).expect("json string")
    ))
    .expect("raw long unicode");
    assert!(!raw_content_starts_with_system_reminder(
        &raw_long_tool_result
    ));
}

#[test]
fn session_matches_query_searches_jcode_transcript_contents() {
    let _env_lock = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("temp dir");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

    let mut session = Session::create_with_id(
        "session_transcript_search".to_string(),
        Some("/tmp/transcript-search".to_string()),
        Some("Transcript Search".to_string()),
    );
    session.append_stored_message(crate::session::StoredMessage {
        id: "msg1".to_string(),
        role: crate::message::Role::User,
        content: vec![crate::message::ContentBlock::Text {
            text: "please find the zebra needle hidden in transcript text".to_string(),
            cache_control: None,
        }],
        display_role: None,
        timestamp: None,
        tool_duration_ms: None,
        token_usage: None,
    });
    session.save().expect("save session");

    let sessions = load_sessions().expect("load sessions");
    let loaded = sessions
        .iter()
        .find(|candidate| candidate.id == "session_transcript_search")
        .expect("session present");

    assert!(loaded.search_index.contains("zebra needle"));
    assert!(loaded.messages_preview.is_empty());
    assert!(session_matches_query(loaded, "zebra needle"));
    assert!(session_matches_query(loaded, "ZEBRA NEEDLE"));
    assert!(!session_matches_query(loaded, "missing transcript phrase"));
}

#[test]
fn session_matches_query_searches_external_codex_transcript_contents() {
    let _env_lock = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("temp dir");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

    let codex_dir = temp.path().join("external/.codex/sessions/2026/04/19");
    std::fs::create_dir_all(&codex_dir).expect("create codex dir");

    let transcript_path = codex_dir.join("transcript-search.jsonl");
    std::fs::write(
        &transcript_path,
        concat!(
            "{\"timestamp\":\"2026-04-19T04:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"codex-transcript-search\",\"timestamp\":\"2026-04-19T03:59:00Z\",\"cwd\":\"/tmp/codex-search\"}}\n",
            "{\"timestamp\":\"2026-04-19T04:00:01Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"the kiwi comet bug is only mentioned in transcript content\"}]}}\n"
        ),
    )
    .expect("write codex transcript");

    let sessions = load_sessions().expect("load sessions");
    let loaded = sessions
        .iter()
        .find(|candidate| candidate.id == "codex:codex-transcript-search")
        .expect("codex session present");

    assert!(!loaded.search_index.contains("kiwi comet"));
    assert!(loaded.messages_preview.is_empty());
    assert!(session_matches_query(loaded, "kiwi comet"));
    assert!(!session_matches_query(loaded, "dragonfruit meteor"));
}

#[test]
#[ignore = "developer benchmark: times real /resume loading phases"]
fn benchmark_real_resume_loading_phases() {
    invalidate_session_list_cache();

    let sessions_dir = storage::jcode_dir().expect("jcode dir").join("sessions");
    let scan_limit = session_scan_limit();
    let candidate_limit = session_candidate_window(scan_limit);

    let phase_start = std::time::Instant::now();
    let candidates = if sessions_dir.exists() {
        collect_recent_session_candidates(&sessions_dir, candidate_limit)
            .expect("collect recent session candidates")
    } else {
        Vec::new()
    };
    let collect_candidates_elapsed = phase_start.elapsed();

    let mut sessions = Vec::new();
    let mut skipped_empty = 0usize;
    let mut skipped_imported = 0usize;
    let mut summary_errors = 0usize;
    let phase_start = std::time::Instant::now();
    for stem in &candidates {
        if sessions.len() >= scan_limit {
            let saved = sessions_dir.join(format!("{stem}.json"));
            if !session_snapshot_or_journal_has_saved_metadata(&saved) {
                continue;
            }
        }
        if stem.starts_with("imported_cc_")
            || stem.starts_with("imported_codex_")
            || stem.starts_with("imported_pi_")
            || stem.starts_with("imported_opencode_")
        {
            skipped_imported += 1;
            continue;
        }

        let path = sessions_dir.join(format!("{stem}.json"));
        match load_session_summary(&path) {
            Ok(summary) if summary.messages.visible_message_count > 0 => {
                sessions.push((stem.clone(), summary));
            }
            Ok(_) => skipped_empty += 1,
            Err(_) => summary_errors += 1,
        }
    }
    let jcode_summary_elapsed = phase_start.elapsed();

    let phase_start = std::time::Instant::now();
    let claude = load_external_claude_code_sessions(scan_limit);
    let claude_elapsed = phase_start.elapsed();

    let phase_start = std::time::Instant::now();
    let codex = load_external_codex_sessions(scan_limit);
    let codex_elapsed = phase_start.elapsed();

    let phase_start = std::time::Instant::now();
    let pi = load_external_pi_sessions(scan_limit);
    let pi_elapsed = phase_start.elapsed();

    let phase_start = std::time::Instant::now();
    let opencode = load_external_opencode_sessions(scan_limit);
    let opencode_elapsed = phase_start.elapsed();

    let phase_start = std::time::Instant::now();
    let all_sessions = load_sessions().expect("load sessions");
    let load_sessions_elapsed = phase_start.elapsed();

    invalidate_session_list_cache();
    let phase_start = std::time::Instant::now();
    let (groups, orphans) = load_sessions_grouped().expect("load grouped sessions");
    let grouped_elapsed = phase_start.elapsed();

    let snapshot_count = std::fs::read_dir(&sessions_dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| {
                    entry.file_name().to_str().is_some_and(|name| {
                        name.ends_with(".json") && !name.ends_with(".journal.json")
                    })
                })
                .count()
        })
        .unwrap_or_default();

    eprintln!(
        concat!(
            "real resume phases: scan_limit={} candidate_limit={} snapshot_count={} ",
            "candidate_count={} collect_candidates={}ms ",
            "jcode_summary={}ms jcode_loaded={} skipped_empty={} skipped_imported={} summary_errors={} ",
            "external_claude={}ms/{} external_codex={}ms/{} external_pi={}ms/{} external_opencode={}ms/{} ",
            "load_sessions={}ms/{} load_sessions_grouped={}ms groups={} orphans={}"
        ),
        scan_limit,
        candidate_limit,
        snapshot_count,
        candidates.len(),
        collect_candidates_elapsed.as_millis(),
        jcode_summary_elapsed.as_millis(),
        sessions.len(),
        skipped_empty,
        skipped_imported,
        summary_errors,
        claude_elapsed.as_millis(),
        claude.len(),
        codex_elapsed.as_millis(),
        codex.len(),
        pi_elapsed.as_millis(),
        pi.len(),
        opencode_elapsed.as_millis(),
        opencode.len(),
        load_sessions_elapsed.as_millis(),
        all_sessions.len(),
        grouped_elapsed.as_millis(),
        groups.len(),
        orphans.len(),
    );
}

#[test]
#[ignore = "developer benchmark: scans the real JCODE_HOME session directory"]
fn benchmark_real_resume_loading_reports_timings() {
    invalidate_session_list_cache();

    let load_start = std::time::Instant::now();
    let sessions = load_sessions().expect("load real sessions");
    let load_elapsed = load_start.elapsed();

    invalidate_session_list_cache();
    let grouped_start = std::time::Instant::now();
    let grouped = load_sessions_grouped().expect("load real grouped sessions");
    let grouped_elapsed = grouped_start.elapsed();
    let grouped_count = grouped
        .0
        .iter()
        .map(|group| group.sessions.len())
        .sum::<usize>()
        + grouped.1.len();

    eprintln!(
        "real resume bench: load_sessions={}ms count={} load_sessions_grouped={}ms grouped_count={} server_groups={} orphan_sessions={}",
        load_elapsed.as_millis(),
        sessions.len(),
        grouped_elapsed.as_millis(),
        grouped_count,
        grouped.0.len(),
        grouped.1.len()
    );
}

#[test]
fn benchmark_resume_loading_reports_timings() {
    let _env_lock = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("temp dir");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

    let sessions_dir = temp.path().join("sessions");
    std::fs::create_dir_all(&sessions_dir).expect("create sessions dir");

    for idx in 0..120 {
        let mut session = Session::create_with_id(
            format!("session_resume_bench_{idx:03}"),
            Some(format!("/tmp/resume-bench-{idx:03}")),
            Some(format!("Resume Bench {idx:03}")),
        );
        session.append_stored_message(crate::session::StoredMessage {
            id: format!("msg-{idx}-1"),
            role: crate::message::Role::User,
            content: vec![crate::message::ContentBlock::Text {
                text: format!("session {idx:03} says benchmark transcript token zebra-{idx:03}"),
                cache_control: None,
            }],
            display_role: None,
            timestamp: None,
            tool_duration_ms: None,
            token_usage: None,
        });
        session.append_stored_message(crate::session::StoredMessage {
            id: format!("msg-{idx}-2"),
            role: crate::message::Role::Assistant,
            content: vec![crate::message::ContentBlock::Text {
                text: "assistant reply for benchmark coverage".to_string(),
                cache_control: None,
            }],
            display_role: None,
            timestamp: None,
            tool_duration_ms: None,
            token_usage: None,
        });
        session.save().expect("save benchmark session");
    }

    let load_start = std::time::Instant::now();
    let sessions = load_sessions().expect("load sessions");
    let load_elapsed = load_start.elapsed();

    let group_start = std::time::Instant::now();
    let grouped = load_sessions_grouped().expect("load grouped sessions");
    let group_elapsed = group_start.elapsed();

    assert!(sessions.len() >= 100);
    assert!(!grouped.0.is_empty() || !grouped.1.is_empty());

    eprintln!(
        "resume bench: load_sessions={}ms load_sessions_grouped={}ms count={}",
        load_elapsed.as_millis(),
        group_elapsed.as_millis(),
        sessions.len()
    );
}
