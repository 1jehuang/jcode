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

    let transcript_path_json = serde_json::to_string(&transcript_path.to_string_lossy())
        .expect("serialize transcript path");
    std::fs::write(
        project_dir.join("sessions-index.json"),
        format!(
            concat!(
                "{{\"version\":1,\"entries\":[",
                "{{\"sessionId\":\"claude-session-123\",",
                "\"fullPath\":{},",
                "\"firstPrompt\":\"Investigate the login bug\",",
                "\"summary\":\"Investigate the login bug\",",
                "\"messageCount\":2,",
                "\"created\":\"2026-04-04T12:00:00Z\",",
                "\"modified\":\"2026-04-04T12:05:00Z\",",
                "\"projectPath\":\"/tmp/demo-project\"",
                "}}]}}"
            ),
            transcript_path_json
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

    // The transcript path is interpolated into a JSON string literal,
    // so single backslashes (Windows path separator) must be doubled
    // to remain valid JSON. Use serde_json to handle escaping rather
    // than relying on `.display()`.
    let transcript_path_json = serde_json::to_string(&transcript_path.to_string_lossy())
        .expect("serialize transcript path");
    std::fs::write(
        project_dir.join("sessions-index.json"),
        format!(
            concat!(
                "{{\"version\":1,\"entries\":[",
                "{{\"sessionId\":\"claude-session-456\",",
                "\"fullPath\":{},",
                "\"firstPrompt\":\"Fix the flaky test\",",
                "\"messageCount\":2,",
                "\"created\":\"2026-04-04T12:00:00Z\",",
                "\"modified\":\"2026-04-04T12:05:00Z\"",
                "}}]}}"
            ),
            transcript_path_json
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

    assert!(!loaded.search_index.contains("zebra needle"));
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
