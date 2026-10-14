use super::*;

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

#[test]
fn test_truncate_title() {
    assert_eq!(truncate_title("short"), "short");
    assert_eq!(truncate_title("line1\nline2"), "line1");

    let long = "a".repeat(100);
    let truncated = truncate_title(&long);
    assert!(truncated.ends_with("..."));
    assert!(truncated.len() <= 80);
}

#[test]
fn test_convert_text_content() {
    let content = ClaudeCodeContent::Text("hello".to_string());
    let blocks = convert_content_blocks(&content);
    assert_eq!(blocks.len(), 1);
    match &blocks[0] {
        ContentBlock::Text { text, .. } => assert_eq!(text, "hello"),
        _ => panic!("Expected text block"),
    }
}

#[test]
fn test_convert_empty_content() {
    let content = ClaudeCodeContent::Empty;
    let blocks = convert_content_blocks(&content);
    assert!(blocks.is_empty());
}

#[test]
fn test_convert_blocks_content() {
    let content = ClaudeCodeContent::Blocks(vec![
        ClaudeCodeContentBlock::Text {
            text: "hello".to_string(),
        },
        ClaudeCodeContentBlock::Thinking {
            thinking: "let me think".to_string(),
            _signature: None,
        },
        ClaudeCodeContentBlock::ToolUse {
            id: "tool1".to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({"cmd": "ls"}),
        },
    ]);
    let blocks = convert_content_blocks(&content);
    assert_eq!(blocks.len(), 3);

    match &blocks[0] {
        ContentBlock::Text { text, .. } => assert_eq!(text, "hello"),
        _ => panic!("Expected text"),
    }
    match &blocks[1] {
        ContentBlock::Reasoning { text } => assert_eq!(text, "let me think"),
        _ => panic!("Expected reasoning"),
    }
    match &blocks[2] {
        ContentBlock::ToolUse { name, .. } => assert_eq!(name, "bash"),
        _ => panic!("Expected tool use"),
    }
}

#[test]
fn imported_tool_history_is_provider_neutral() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let transcript = temp.path().join("tool-history.jsonl");
    std::fs::write(
        &transcript,
        concat!(
            "{\"type\":\"user\",\"uuid\":\"u1\",\"sessionId\":\"safe-tools\",\"message\":{\"role\":\"user\",\"content\":\"inspect it\"},\"timestamp\":\"2026-07-13T10:00:00Z\"}\n",
            "{\"type\":\"assistant\",\"uuid\":\"a1\",\"sessionId\":\"safe-tools\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"thinking\",\"thinking\":\"hidden\"},{\"type\":\"tool_use\",\"id\":\"call/unsafe\",\"name\":\"mcp:read\",\"input\":{\"path\":\"a.txt\"}}]},\"timestamp\":\"2026-07-13T10:00:01Z\"}\n",
            "{\"type\":\"user\",\"uuid\":\"u2\",\"sessionId\":\"safe-tools\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"tool_result\",\"tool_use_id\":\"call/unsafe\",\"content\":\"contents\"}]},\"timestamp\":\"2026-07-13T10:00:02Z\"}\n"
        ),
    )
    .unwrap();

    let imported = import_session_from_file(&transcript, "safe-tools").unwrap();
    assert_eq!(imported.messages.len(), 3);
    assert!(imported.messages.iter().all(|message| {
        message
            .content
            .iter()
            .all(|block| matches!(block, ContentBlock::Text { .. }))
    }));
    let visible = imported
        .messages
        .iter()
        .flat_map(|message| message.content.iter())
        .filter_map(|block| match block {
            ContentBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(visible.contains("[Imported tool call: mcp:read]"));
    assert!(visible.contains("[Imported tool result: call/unsafe]"));
    assert!(!visible.contains("hidden"));
}

#[test]
fn imported_history_is_bounded_for_fast_initial_render() {
    let mut session = Session::create_with_id("imported_test_bounded".to_string(), None, None);
    for index in 0..(IMPORT_HISTORY_MAX_MESSAGES + 80) {
        session.append_stored_message(StoredMessage {
            id: format!("message-{index}"),
            role: if index % 2 == 0 {
                Role::User
            } else {
                Role::Assistant
            },
            content: vec![ContentBlock::Text {
                text: format!("message {index} {}", "x".repeat(4096)),
                cache_control: None,
            }],
            display_role: None,
            timestamp: None,
            tool_duration_ms: None,
            token_usage: None,
        });
    }

    assert!(normalize_imported_history(&mut session, true));
    assert!(session.messages.len() <= IMPORT_HISTORY_MAX_MESSAGES + 1);
    let text_bytes = session
        .messages
        .iter()
        .flat_map(|message| message.content.iter())
        .map(|block| match block {
            ContentBlock::Text { text, .. } => text.len(),
            _ => 0,
        })
        .sum::<usize>();
    assert!(text_bytes <= IMPORT_HISTORY_MAX_TEXT_BYTES + 512);
    assert!(matches!(
        session.messages[0].content.first(),
        Some(ContentBlock::Text { text, .. }) if text.contains("older messages were omitted")
    ));
}

#[test]
fn repeated_external_resume_reuses_the_imported_snapshot() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let transcript = temp.path().join("cached.jsonl");
    std::fs::write(
        &transcript,
        "{\"type\":\"user\",\"uuid\":\"u1\",\"sessionId\":\"cached\",\"message\":{\"role\":\"user\",\"content\":\"hello\"},\"timestamp\":\"2026-07-13T10:00:00Z\"}\n",
    )
    .unwrap();
    let target = jcode_session_types::ResumeTarget::ClaudeCodeSession {
        session_id: "cached".to_string(),
        session_path: transcript.to_string_lossy().to_string(),
    };

    let first = resolve_resume_target_to_jcode(&target).unwrap();
    std::fs::remove_file(&transcript).unwrap();
    let second = resolve_resume_target_to_jcode(&target).unwrap();
    assert_eq!(first, second);
    assert!(Session::load(&imported_claude_code_session_id("cached")).is_ok());
}

#[cfg(target_os = "linux")]
fn linux_process_start_token(pid: u32) -> String {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let close = stat.rfind(')').unwrap();
    stat[close + 2..]
        .split_whitespace()
        .nth(19)
        .unwrap()
        .to_string()
}

#[cfg(target_os = "linux")]
fn write_live_claude_record(
    root: &std::path::Path,
    child: &std::process::Child,
    session_id: &str,
) -> std::path::PathBuf {
    std::fs::create_dir_all(root).unwrap();
    let path = root.join(format!("{}.json", child.id()));
    std::fs::write(
        &path,
        serde_json::json!({
            "pid": child.id(),
            "sessionId": session_id,
            "cwd": "/tmp/project",
            "procStart": linux_process_start_token(child.id()),
            "kind": "interactive",
            "entrypoint": "cli",
            "startedAt": 123,
            "name": "takeover-test",
            "version": "2.1.212"
        })
        .to_string(),
    )
    .unwrap();
    path
}

#[cfg(target_os = "linux")]
fn write_claude_transcript(path: &std::path::Path, session_id: &str) {
    std::fs::write(
        path,
        format!(
            "{{\"type\":\"user\",\"uuid\":\"u1\",\"sessionId\":\"{session_id}\",\"cwd\":\"/tmp/project\",\"message\":{{\"role\":\"user\",\"content\":\"preserve this question\"}},\"timestamp\":\"2026-07-17T00:00:00Z\"}}\n{{\"type\":\"assistant\",\"uuid\":\"a1\",\"sessionId\":\"{session_id}\",\"cwd\":\"/tmp/project\",\"message\":{{\"role\":\"assistant\",\"model\":\"claude-opus-4-8\",\"content\":\"preserve this answer\"}},\"timestamp\":\"2026-07-17T00:00:01Z\"}}\n"
        ),
    )
    .unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn ordinary_resume_never_stops_a_live_claude_process() {
    use std::process::{Command, Stdio};

    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let external = temp.path().join("external/.claude");
    let transcript = external.join("projects/demo/ordinary-live.jsonl");
    std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    write_claude_transcript(&transcript, "ordinary-live");
    let mut claude = Command::new("sleep")
        .arg("60")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    write_live_claude_record(&external.join("sessions"), &claude, "ordinary-live");
    let target = jcode_session_types::ResumeTarget::ClaudeCodeSession {
        session_id: "ordinary-live".to_string(),
        session_path: transcript.to_string_lossy().to_string(),
    };

    let resolved = resolve_resume_target_to_jcode(&target).unwrap();
    assert!(matches!(
        resolved,
        jcode_session_types::ResumeTarget::JcodeSession { .. }
    ));
    assert!(claude.try_wait().unwrap().is_none());

    claude.kill().unwrap();
    claude.wait().unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn explicit_takeover_preserves_history_and_stops_only_matching_process() {
    use std::process::{Command, Stdio};

    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let external = temp.path().join("external/.claude");
    let transcript = external.join("projects/demo/takeover-live.jsonl");
    std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    write_claude_transcript(&transcript, "takeover-live");
    let mut claude = Command::new("sleep")
        .arg("60")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut unrelated = Command::new("sleep")
        .arg("60")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let registry = write_live_claude_record(&external.join("sessions"), &claude, "takeover-live");
    let target = jcode_session_types::ResumeTarget::ClaudeCodeSession {
        session_id: "takeover-live".to_string(),
        session_path: transcript.to_string_lossy().to_string(),
    };

    let resolved = take_over_live_claude_session(&target).unwrap();
    let jcode_session_types::ResumeTarget::JcodeSession { session_id } = resolved else {
        panic!("expected Jcode session");
    };
    assert!(session_id.starts_with("session_"));
    claude.wait().unwrap();
    assert!(unrelated.try_wait().unwrap().is_none());
    assert!(!registry.exists());

    let imported = Session::load(&session_id).unwrap();
    assert_eq!(
        imported.provider_session_id.as_deref(),
        Some("takeover-live")
    );
    let visible = imported
        .messages
        .iter()
        .flat_map(|message| message.content.iter())
        .filter_map(|block| match block {
            ContentBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(visible.contains("preserve this question"));
    assert!(visible.contains("preserve this answer"));

    unrelated.kill().unwrap();
    unrelated.wait().unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn takeover_with_no_complete_messages_leaves_claude_running() {
    use std::process::{Command, Stdio};

    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let external = temp.path().join("external/.claude");
    let transcript = external.join("projects/demo/incomplete-live.jsonl");
    std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    std::fs::write(&transcript, "{\"type\":\"progress\",\"data\":{}}\n").unwrap();
    let mut claude = Command::new("sleep")
        .arg("60")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    write_live_claude_record(&external.join("sessions"), &claude, "incomplete-live");
    let target = jcode_session_types::ResumeTarget::ClaudeCodeSession {
        session_id: "incomplete-live".to_string(),
        session_path: transcript.to_string_lossy().to_string(),
    };

    assert!(take_over_live_claude_session(&target).is_err());
    assert!(claude.try_wait().unwrap().is_none());
    let sessions_dir = temp.path().join("sessions");
    let staged = std::fs::read_dir(&sessions_dir)
        .map(|entries| entries.filter_map(|entry| entry.ok()).count())
        .unwrap_or(0);
    assert_eq!(staged, 0, "failed takeover must roll back staged sessions");

    claude.kill().unwrap();
    claude.wait().unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn takeover_rejects_a_transcript_from_a_different_live_session() {
    use std::process::{Command, Stdio};

    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let external = temp.path().join("external/.claude");
    let transcript = external.join("projects/demo/wrong-session.jsonl");
    std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    write_claude_transcript(&transcript, "different-session");
    let mut claude = Command::new("sleep")
        .arg("60")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    write_live_claude_record(&external.join("sessions"), &claude, "live-session");
    let target = jcode_session_types::ResumeTarget::ClaudeCodeSession {
        session_id: "live-session".to_string(),
        session_path: transcript.to_string_lossy().to_string(),
    };

    let error = take_over_live_claude_session(&target).unwrap_err();
    assert!(format!("{error:#}").contains("different-session"));
    assert!(claude.try_wait().unwrap().is_none());
    assert!(!temp.path().join("sessions").exists());

    claude.kill().unwrap();
    claude.wait().unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn takeover_timeout_preserves_the_staged_jcode_session_after_sigterm() {
    use std::process::{Command, Stdio};

    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let external = temp.path().join("external/.claude");
    let transcript = external.join("projects/demo/slow-exit.jsonl");
    std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    write_claude_transcript(&transcript, "slow-exit");
    let mut claude = Command::new("sh")
        .args(["-c", "trap '' TERM; exec sleep 60"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(50));
    write_live_claude_record(&external.join("sessions"), &claude, "slow-exit");
    let target = jcode_session_types::ResumeTarget::ClaudeCodeSession {
        session_id: "slow-exit".to_string(),
        session_path: transcript.to_string_lossy().to_string(),
    };

    let error =
        take_over_live_claude_session_with_timeout(&target, std::time::Duration::from_millis(50))
            .unwrap_err();
    assert!(error.to_string().contains("was preserved"));
    assert!(claude.try_wait().unwrap().is_none());

    let snapshots = std::fs::read_dir(temp.path().join("sessions"))
        .unwrap()
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json"))
        .collect::<Vec<_>>();
    assert_eq!(snapshots.len(), 1);
    let staged = Session::load(
        snapshots[0]
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(staged.provider_session_id.as_deref(), Some("slow-exit"));
    assert_eq!(staged.messages.len(), 2);

    claude.kill().unwrap();
    claude.wait().unwrap();
}

#[test]
fn cached_imported_session_preserves_existing_history_verbatim() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let imported_id = imported_codex_session_id("legacy-tools");
    let mut legacy = Session::create_with_id(imported_id.clone(), None, None);
    legacy.append_stored_message(StoredMessage {
        id: "assistant-tool".to_string(),
        role: Role::Assistant,
        content: vec![ContentBlock::ToolUse {
            id: "legacy/call".to_string(),
            name: "legacy:tool".to_string(),
            input: serde_json::json!({"value": 1}),
            thought_signature: None,
        }],
        display_role: None,
        timestamp: None,
        tool_duration_ms: None,
        token_usage: None,
    });
    legacy.append_stored_message(StoredMessage {
        id: "user-result".to_string(),
        role: Role::User,
        content: vec![ContentBlock::ToolResult {
            tool_use_id: "legacy/call".to_string(),
            content: "done".to_string(),
            is_error: Some(false),
        }],
        display_role: None,
        timestamp: None,
        tool_duration_ms: None,
        token_usage: None,
    });
    legacy.append_stored_message(StoredMessage {
        id: "jcode-continuation".to_string(),
        role: Role::Assistant,
        content: vec![ContentBlock::Text {
            text: "continued inside jcode".to_string(),
            cache_control: None,
        }],
        display_role: None,
        timestamp: None,
        tool_duration_ms: None,
        token_usage: None,
    });
    legacy.save().unwrap();

    let resolved =
        resolve_resume_target_to_jcode(&jcode_session_types::ResumeTarget::CodexSession {
            session_id: "legacy-tools".to_string(),
            session_path: temp
                .path()
                .join("source-no-longer-present.jsonl")
                .to_string_lossy()
                .to_string(),
        })
        .unwrap();
    assert_eq!(
        resolved,
        jcode_session_types::ResumeTarget::JcodeSession {
            session_id: imported_id.clone(),
        }
    );
    let preserved = Session::load(&imported_id).unwrap();
    assert_eq!(
        serde_json::to_value(&preserved.messages).unwrap(),
        serde_json::to_value(&legacy.messages).unwrap()
    );
}

#[test]
fn message_role_prefilter_accepts_json_whitespace() {
    assert!(json_line_has_message_role(
        r#"{"payload":{"role" : "assistant"}}"#
    ));
    assert!(json_line_has_message_role(r#"{"role":"user"}"#));
    assert!(!json_line_has_message_role(
        r#"{"type":"reasoning","content":"role text only"}"#
    ));
}

#[test]
fn test_discover_projects_uses_sandboxed_external_home() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

    let project_dir = temp.path().join("external/.claude/projects/demo");
    std::fs::create_dir_all(&project_dir).unwrap();
    std::fs::write(
        project_dir.join("sessions-index.json"),
        r#"{"version":1,"entries":[]}"#,
    )
    .unwrap();

    let projects = discover_projects().unwrap();
    assert_eq!(projects, vec![project_dir.join("sessions-index.json")]);
}

#[test]
fn test_list_claude_code_sessions_uses_live_transcripts_when_index_is_stale() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

    let project_dir = temp.path().join("external/.claude/projects/demo-project");
    std::fs::create_dir_all(&project_dir).unwrap();

    let indexed_session_path = project_dir.join("live-session-1.jsonl");
    std::fs::write(
            &indexed_session_path,
            concat!(
                "{\"type\":\"user\",\"uuid\":\"u1\",\"sessionId\":\"live-session-1\",\"cwd\":\"/tmp/demo-project\",\"message\":{\"role\":\"user\",\"content\":\"Investigate the login bug\"},\"timestamp\":\"2026-04-04T12:00:00Z\"}\n",
                "{\"type\":\"assistant\",\"uuid\":\"a1\",\"parentUuid\":\"u1\",\"sessionId\":\"live-session-1\",\"cwd\":\"/tmp/demo-project\",\"message\":{\"role\":\"assistant\",\"model\":\"claude-sonnet-4-6\",\"content\":\"I can help with that.\"},\"timestamp\":\"2026-04-04T12:05:00Z\"}\n"
            ),
        )
        .unwrap();

    let orphan_session_path = project_dir.join("orphan-session-2.jsonl");
    std::fs::write(
            &orphan_session_path,
            concat!(
                "{\"type\":\"user\",\"uuid\":\"u2\",\"sessionId\":\"orphan-session-2\",\"cwd\":\"/tmp/demo-project\",\"message\":{\"role\":\"user\",\"content\":\"Summarize the deployment issue\"},\"timestamp\":\"2026-04-05T09:00:00Z\"}\n",
                "{\"type\":\"assistant\",\"uuid\":\"a2\",\"parentUuid\":\"u2\",\"sessionId\":\"orphan-session-2\",\"cwd\":\"/tmp/demo-project\",\"message\":{\"role\":\"assistant\",\"model\":\"claude-sonnet-4-6\",\"content\":\"Here is the deployment summary.\"},\"timestamp\":\"2026-04-05T09:01:00Z\"}\n"
            ),
        )
        .unwrap();

    std::fs::write(
        project_dir.join("sessions-index.json"),
        concat!(
            "{\"version\":1,\"entries\":[",
            "{\"sessionId\":\"live-session-1\",",
            "\"fullPath\":\"/missing/live-session-1.jsonl\",",
            "\"firstPrompt\":\"Investigate the login bug\",",
            "\"summary\":\"Investigate the login bug\",",
            "\"messageCount\":2,",
            "\"created\":\"2026-04-04T12:00:00Z\",",
            "\"modified\":\"2026-04-04T12:05:00Z\",",
            "\"projectPath\":\"/tmp/demo-project\"",
            "}] }"
        ),
    )
    .unwrap();

    let sessions = list_claude_code_sessions().unwrap();

    let indexed = sessions
        .iter()
        .find(|session| session.session_id == "live-session-1")
        .expect("indexed live transcript should be discovered");
    assert_eq!(indexed.full_path, indexed_session_path.to_string_lossy());
    assert_eq!(
        indexed.summary.as_deref(),
        Some("Investigate the login bug")
    );
    assert_eq!(indexed.project_path.as_deref(), Some("/tmp/demo-project"));

    let orphan = sessions
        .iter()
        .find(|session| session.session_id == "orphan-session-2")
        .expect("orphan live transcript should be discovered");
    assert_eq!(orphan.full_path, orphan_session_path.to_string_lossy());
    assert_eq!(orphan.first_prompt, "Summarize the deployment issue");
    assert_eq!(orphan.message_count, 2);
}

#[test]
fn test_list_claude_code_sessions_uses_index_metadata_without_parsing_transcript() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

    let project_dir = temp.path().join("external/.claude/projects/demo-project");
    std::fs::create_dir_all(&project_dir).unwrap();

    let transcript_path = project_dir.join("indexed-session.jsonl");
    std::fs::write(&transcript_path, "{this is not valid jsonl}\n").unwrap();

    std::fs::write(
        project_dir.join("sessions-index.json"),
        format!(
            concat!(
                "{{\"version\":1,\"entries\":[",
                "{{\"sessionId\":\"indexed-session\",",
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
    .unwrap();

    let sessions = list_claude_code_sessions().unwrap();
    let session = sessions
        .iter()
        .find(|session| session.session_id == "indexed-session")
        .expect("indexed session should be listed from index metadata");

    assert_eq!(session.message_count, 2);
    assert_eq!(
        session.summary.as_deref(),
        Some("Investigate the login bug")
    );
    assert_eq!(session.first_prompt, "Investigate the login bug");
}

#[test]
fn test_list_claude_code_sessions_skips_empty_index_entries_without_messages() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

    let project_dir = temp.path().join("external/.claude/projects/demo-project");
    std::fs::create_dir_all(&project_dir).unwrap();

    let transcript_path = project_dir.join("empty-session.jsonl");
    std::fs::write(
        &transcript_path,
        "{\"type\":\"system\",\"sessionId\":\"empty-session\"}\n",
    )
    .unwrap();

    std::fs::write(
        project_dir.join("sessions-index.json"),
        format!(
            concat!(
                "{{\"version\":1,\"entries\":[",
                "{{\"sessionId\":\"empty-session\",",
                "\"fullPath\":\"{}\",",
                "\"firstPrompt\":\"\",",
                "\"summary\":\"\",",
                "\"messageCount\":0",
                "}}]}}"
            ),
            transcript_path.display()
        ),
    )
    .unwrap();

    let sessions = list_claude_code_sessions().unwrap();
    assert!(
        sessions.is_empty(),
        "empty placeholder sessions should be hidden"
    );
}

#[test]
fn test_import_claude_session_uses_recovered_live_transcript() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

    let project_dir = temp.path().join("external/.claude/projects/demo-project");
    std::fs::create_dir_all(&project_dir).unwrap();

    let transcript_path = project_dir.join("live-session-1.jsonl");
    std::fs::write(
            &transcript_path,
            concat!(
                "{\"type\":\"user\",\"uuid\":\"u1\",\"sessionId\":\"live-session-1\",\"cwd\":\"/tmp/demo-project\",\"message\":{\"role\":\"user\",\"content\":\"Investigate the login bug\"},\"timestamp\":\"2026-04-04T12:00:00Z\"}\n",
                "{\"type\":\"assistant\",\"uuid\":\"a1\",\"parentUuid\":\"u1\",\"sessionId\":\"live-session-1\",\"cwd\":\"/tmp/demo-project\",\"message\":{\"role\":\"assistant\",\"model\":\"claude-sonnet-4-6\",\"content\":\"I can help with that.\"},\"timestamp\":\"2026-04-04T12:05:00Z\"}\n"
            ),
        )
        .unwrap();

    std::fs::write(
        project_dir.join("sessions-index.json"),
        concat!(
            "{\"version\":1,\"entries\":[",
            "{\"sessionId\":\"live-session-1\",",
            "\"fullPath\":\"/missing/live-session-1.jsonl\",",
            "\"firstPrompt\":\"Investigate the login bug\",",
            "\"summary\":\"Investigate the login bug\",",
            "\"messageCount\":2,",
            "\"created\":\"2026-04-04T12:00:00Z\",",
            "\"modified\":\"2026-04-04T12:05:00Z\",",
            "\"projectPath\":\"/tmp/demo-project\"",
            "}] }"
        ),
    )
    .unwrap();

    let imported = import_session("live-session-1").unwrap();
    assert_eq!(
        imported.id,
        imported_claude_code_session_id("live-session-1")
    );
    assert_eq!(imported.provider_key.as_deref(), Some("claude-code"));
    assert_eq!(imported.working_dir.as_deref(), Some("/tmp/demo-project"));
    assert_eq!(imported.model.as_deref(), Some("claude-sonnet-4-6"));
    assert_eq!(imported.messages.len(), 2);
}

#[test]
fn test_import_pi_session_creates_jcode_snapshot() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

    let pi_dir = temp.path().join("external/.pi/agent/sessions/project");
    std::fs::create_dir_all(&pi_dir).unwrap();
    let session_path = pi_dir.join("session.jsonl");
    std::fs::write(
            &session_path,
            concat!(
                "{\"type\":\"session\",\"id\":\"pi-session-1\",\"timestamp\":\"2026-04-05T19:00:00Z\",\"cwd\":\"/tmp/pi-demo\"}\n",
                "{\"type\":\"model_change\",\"timestamp\":\"2026-04-05T19:00:01Z\",\"provider\":\"pi\",\"modelId\":\"pi-model\"}\n",
                "{\"type\":\"message\",\"timestamp\":\"2026-04-05T19:00:02Z\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"hello pi\"}]}}\n",
                "{\"type\":\"message\",\"timestamp\":\"2026-04-05T19:00:03Z\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"hi back\"}]}}\n"
            ),
        )
        .unwrap();

    let imported = import_pi_session(&session_path.to_string_lossy()).unwrap();
    assert_eq!(
        imported.id,
        imported_pi_session_id(&session_path.to_string_lossy())
    );
    assert_eq!(imported.provider_key.as_deref(), Some("pi"));
    assert_eq!(imported.model.as_deref(), Some("pi-model"));
    assert_eq!(imported.working_dir.as_deref(), Some("/tmp/pi-demo"));
    assert_eq!(imported.messages.len(), 2);
}

#[test]
fn test_import_opencode_session_creates_jcode_snapshot() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

    let session_dir = temp
        .path()
        .join("external/.local/share/opencode/storage/session/global");
    let message_dir = temp
        .path()
        .join("external/.local/share/opencode/storage/message/ses_test_opencode");
    let user_part_dir = temp
        .path()
        .join("external/.local/share/opencode/storage/part/msg-user");
    let assistant_part_dir = temp
        .path()
        .join("external/.local/share/opencode/storage/part/msg-assistant");
    std::fs::create_dir_all(&session_dir).unwrap();
    std::fs::create_dir_all(&message_dir).unwrap();
    std::fs::create_dir_all(&user_part_dir).unwrap();
    std::fs::create_dir_all(&assistant_part_dir).unwrap();

    std::fs::write(
        session_dir.join("ses_test_opencode.json"),
        concat!(
            "{",
            "\"id\":\"ses_test_opencode\",",
            "\"directory\":\"/tmp/opencode-demo\",",
            "\"title\":\"OpenCode imported\",",
            "\"time\":{\"created\":1775415600000,\"updated\":1775415605000}",
            "}"
        ),
    )
    .unwrap();

    std::fs::write(
        message_dir.join("msg-user.json"),
        concat!(
            "{",
            "\"id\":\"msg-user\",",
            "\"role\":\"user\",",
            "\"time\":{\"created\":1775415601000},",
            "\"model\":{\"providerID\":\"opencode\",\"modelID\":\"big-pickle\"}",
            "}"
        ),
    )
    .unwrap();

    std::fs::write(
        message_dir.join("msg-assistant.json"),
        concat!(
            "{",
            "\"id\":\"msg-assistant\",",
            "\"role\":\"assistant\",",
            "\"time\":{\"created\":1775415602000},",
            "\"providerID\":\"opencode\",",
            "\"modelID\":\"big-pickle\"",
            "}"
        ),
    )
    .unwrap();

    // Modern OpenCode (Go storage) keeps message body text in part files.
    std::fs::write(
        user_part_dir.join("prt-user.json"),
        concat!(
            "{",
            "\"id\":\"prt-user\",",
            "\"messageID\":\"msg-user\",",
            "\"type\":\"text\",",
            "\"text\":\"Investigate provider routing\"",
            "}"
        ),
    )
    .unwrap();

    std::fs::write(
        assistant_part_dir.join("prt-assistant.json"),
        concat!(
            "{",
            "\"id\":\"prt-assistant\",",
            "\"messageID\":\"msg-assistant\",",
            "\"type\":\"text\",",
            "\"text\":\"Found the bad provider switch\"",
            "}"
        ),
    )
    .unwrap();

    let imported = import_opencode_session("ses_test_opencode").unwrap();
    assert_eq!(
        imported.id,
        imported_opencode_session_id("ses_test_opencode")
    );
    assert_eq!(imported.provider_key.as_deref(), Some("opencode"));
    assert_eq!(imported.model.as_deref(), Some("big-pickle"));
    assert_eq!(imported.working_dir.as_deref(), Some("/tmp/opencode-demo"));
    assert_eq!(imported.messages.len(), 2);
    let all_text: String = imported
        .messages
        .iter()
        .flat_map(|m| m.content.iter())
        .filter_map(|block| match block {
            ContentBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        all_text.contains("Investigate provider routing"),
        "expected user part text to be imported: {all_text:?}"
    );
    assert!(
        all_text.contains("Found the bad provider switch"),
        "expected assistant part text to be imported: {all_text:?}"
    );
}

#[test]
fn test_resolve_resume_target_to_jcode_imports_codex_session() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

    let codex_dir = temp.path().join("external/.codex/sessions/2026/04/05");
    std::fs::create_dir_all(&codex_dir).unwrap();
    std::fs::write(
            codex_dir.join("rollout.jsonl"),
            concat!(
                "{\"timestamp\":\"2026-04-05T19:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"codex-resolve-test\",\"timestamp\":\"2026-04-05T18:59:00Z\",\"cwd\":\"/tmp/codex-resolve\"}}\n",
                "{\"timestamp\":\"2026-04-05T19:00:01Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"Fix codex resume\"}]}}\n",
                "{\"timestamp\":\"2026-04-05T19:00:02Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"Done\"}]}}\n"
            ),
        )
        .unwrap();

    let resolved =
        resolve_resume_target_to_jcode(&jcode_session_types::ResumeTarget::CodexSession {
            session_id: "codex-resolve-test".to_string(),
            session_path: codex_dir
                .join("rollout.jsonl")
                .to_string_lossy()
                .to_string(),
        })
        .unwrap();

    assert_eq!(
        resolved,
        jcode_session_types::ResumeTarget::JcodeSession {
            session_id: imported_codex_session_id("codex-resolve-test"),
        }
    );
    let loaded = Session::load(&imported_codex_session_id("codex-resolve-test")).unwrap();
    assert_eq!(loaded.messages.len(), 2);
}

/// The resume picker builds a `ClaudeCodeSession` target with id `claude:<id>`
/// and a transcript path; selecting it routes through
/// `resolve_resume_target_to_jcode`, which must import the transcript and hand
/// back a resumable `imported_cc_<id>` jcode session. This guards the full
/// detect -> import -> resume round-trip for Claude Code (previously only Codex
/// had coverage here).
#[test]
fn test_resolve_resume_target_to_jcode_imports_claude_code_session() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

    let project_dir = temp.path().join("external/.claude/projects/demo-project");
    std::fs::create_dir_all(&project_dir).unwrap();
    let transcript_path = project_dir.join("claude-resolve-test.jsonl");
    std::fs::write(
            &transcript_path,
            concat!(
                "{\"type\":\"user\",\"uuid\":\"u1\",\"sessionId\":\"claude-resolve-test\",\"cwd\":\"/tmp/demo-project\",\"message\":{\"role\":\"user\",\"content\":\"Fix the resume round-trip\"},\"timestamp\":\"2026-04-04T12:00:00Z\"}\n",
                "{\"type\":\"assistant\",\"uuid\":\"a1\",\"parentUuid\":\"u1\",\"sessionId\":\"claude-resolve-test\",\"cwd\":\"/tmp/demo-project\",\"message\":{\"role\":\"assistant\",\"model\":\"claude-sonnet-4-6\",\"content\":\"On it.\"},\"timestamp\":\"2026-04-04T12:01:00Z\"}\n"
            ),
        )
        .unwrap();

    let resolved =
        resolve_resume_target_to_jcode(&jcode_session_types::ResumeTarget::ClaudeCodeSession {
            session_id: "claude-resolve-test".to_string(),
            session_path: transcript_path.to_string_lossy().to_string(),
        })
        .unwrap();

    let imported_id = imported_claude_code_session_id("claude-resolve-test");
    assert_eq!(
        resolved,
        jcode_session_types::ResumeTarget::JcodeSession {
            session_id: imported_id.clone(),
        }
    );

    // The id the picker would also derive via `imported_session_id_for_target`
    // must match the snapshot actually written to disk.
    assert_eq!(
        imported_session_id_for_target(&jcode_session_types::ResumeTarget::ClaudeCodeSession {
            session_id: "claude-resolve-test".to_string(),
            session_path: transcript_path.to_string_lossy().to_string(),
        }),
        Some(imported_id.clone())
    );

    let loaded = Session::load(&imported_id).unwrap();
    assert_eq!(loaded.messages.len(), 2);
    assert_eq!(
        loaded.provider_session_id.as_deref(),
        Some("claude-resolve-test")
    );
    assert_eq!(loaded.provider_key.as_deref(), Some("claude-code"));
}

/// Regression for silent data loss: the picker hides the imported jcode session
/// (any `imported_*` stem) and only shows the external `claude:<id>` entry, so
/// re-selecting a Claude session re-enters `import_session_from_file`. If the
/// user already resumed and continued that imported session inside jcode, a
/// blind re-import previously overwrote the snapshot and dropped the jcode-side
/// messages. The continuation must be preserved instead.
#[test]
fn test_reimporting_claude_session_preserves_jcode_continuation() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

    let project_dir = temp.path().join("external/.claude/projects/demo-project");
    std::fs::create_dir_all(&project_dir).unwrap();
    let transcript_path = project_dir.join("claude-continued.jsonl");
    std::fs::write(
            &transcript_path,
            concat!(
                "{\"type\":\"user\",\"uuid\":\"u1\",\"sessionId\":\"claude-continued\",\"cwd\":\"/tmp/demo-project\",\"message\":{\"role\":\"user\",\"content\":\"original prompt\"},\"timestamp\":\"2026-04-04T12:00:00Z\"}\n",
                "{\"type\":\"assistant\",\"uuid\":\"a1\",\"parentUuid\":\"u1\",\"sessionId\":\"claude-continued\",\"cwd\":\"/tmp/demo-project\",\"message\":{\"role\":\"assistant\",\"content\":\"original reply\"},\"timestamp\":\"2026-04-04T12:01:00Z\"}\n"
            ),
        )
        .unwrap();

    // First selection imports the transcript.
    let imported = import_session_from_file(&transcript_path, "claude-continued").unwrap();
    assert_eq!(imported.messages.len(), 2);
    let imported_id = imported_claude_code_session_id("claude-continued");

    // User resumes inside jcode and appends a jcode-only follow-up message.
    let mut session = Session::load(&imported_id).unwrap();
    session.append_stored_message(StoredMessage {
        id: "jcode-continuation".to_string(),
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "jcode-only follow up".to_string(),
            cache_control: None,
        }],
        display_role: None,
        timestamp: None,
        tool_duration_ms: None,
        token_usage: None,
    });
    session.save().unwrap();
    assert_eq!(Session::load(&imported_id).unwrap().messages.len(), 3);

    // Re-selecting the external entry re-enters import; the continuation must survive.
    let resumed =
        resolve_resume_target_to_jcode(&jcode_session_types::ResumeTarget::ClaudeCodeSession {
            session_id: "claude-continued".to_string(),
            session_path: transcript_path.to_string_lossy().to_string(),
        })
        .unwrap();
    assert_eq!(
        resumed,
        jcode_session_types::ResumeTarget::JcodeSession {
            session_id: imported_id.clone(),
        }
    );

    let after = Session::load(&imported_id).unwrap();
    assert_eq!(
        after.messages.len(),
        3,
        "jcode-side continuation must not be clobbered by re-import"
    );
    let preserved = after.messages.iter().flat_map(|m| m.content.iter()).any(
        |block| matches!(block, ContentBlock::Text { text, .. } if text == "jcode-only follow up"),
    );
    assert!(
        preserved,
        "the jcode-only follow up message must be preserved"
    );
}

#[test]
fn test_import_cursor_session_creates_jcode_snapshot() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

    // Cursor stores transcripts at
    // ~/.cursor/projects/<project>/agent-transcripts/<uuid>/<uuid>.jsonl where the
    // project dir encodes the cwd with `-` separators.
    let session_id = "11111111-2222-3333-4444-555555555555";
    let transcript_dir = temp.path().join(format!(
        "external/.cursor/projects/tmp-cursor-demo/agent-transcripts/{session_id}"
    ));
    std::fs::create_dir_all(&transcript_dir).unwrap();
    let transcript_path = transcript_dir.join(format!("{session_id}.jsonl"));
    std::fs::write(
        &transcript_path,
        concat!(
            "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"CURSOR_USER_MARKER refactor router\"}]}}\n",
            "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"CURSOR_ASSISTANT_MARKER done\"}]}}\n",
        ),
    )
    .unwrap();

    let imported = import_cursor_session(session_id).unwrap();
    assert_eq!(imported.id, imported_cursor_session_id(session_id));
    assert_eq!(imported.provider_key.as_deref(), Some("cursor"));
    assert_eq!(imported.messages.len(), 2);
    let all_text: String = imported
        .messages
        .iter()
        .flat_map(|m| m.content.iter())
        .filter_map(|block| match block {
            ContentBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        all_text.contains("CURSOR_USER_MARKER refactor router"),
        "expected user text to import: {all_text:?}"
    );
    assert!(
        all_text.contains("CURSOR_ASSISTANT_MARKER done"),
        "expected assistant text to import: {all_text:?}"
    );

    // Resolving the resume target should import and remap to the jcode snapshot.
    let resumed = crate::import::resolve_resume_target_to_jcode(
        &jcode_session_types::ResumeTarget::CursorSession {
            session_id: session_id.to_string(),
            session_path: transcript_path.to_string_lossy().to_string(),
        },
    )
    .unwrap();
    assert_eq!(
        resumed,
        jcode_session_types::ResumeTarget::JcodeSession {
            session_id: imported_cursor_session_id(session_id),
        }
    );
}
