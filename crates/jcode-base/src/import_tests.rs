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
    std::fs::create_dir_all(&session_dir).unwrap();
    std::fs::create_dir_all(&message_dir).unwrap();

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
            "\"role\":\"user\",",
            "\"time\":{\"created\":1775415601000},",
            "\"summary\":{\"title\":\"Investigate provider routing\"},",
            "\"model\":{\"providerID\":\"opencode\",\"modelID\":\"big-pickle\"}",
            "}"
        ),
    )
    .unwrap();

    std::fs::write(
        message_dir.join("msg-assistant.json"),
        concat!(
            "{",
            "\"role\":\"assistant\",",
            "\"time\":{\"created\":1775415602000},",
            "\"summary\":{\"title\":\"Found the bad provider switch\"},",
            "\"providerID\":\"opencode\",",
            "\"modelID\":\"big-pickle\"",
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
