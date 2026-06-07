use super::*;
use anyhow::{Result, anyhow};

#[test]
fn test_session_exists_roundtrip() -> Result<()> {
    let tmp_dir = std::env::temp_dir().join(format!(
        "jcode-session-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| anyhow!(e))?
            .as_nanos()
    ));
    std::fs::create_dir_all(tmp_dir.join("sessions"))?;

    assert!(!session_path_in_dir(&tmp_dir, "missing-session").exists());

    let session_path = session_path_in_dir(&tmp_dir, "exists-session");
    std::fs::write(&session_path, "{}")?;
    assert!(session_path.exists());

    let random_id = format!(
        "missing-session-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| anyhow!(e))?
            .as_nanos()
    );
    assert!(!session_exists(&random_id));
    Ok(())
}

#[test]
fn derive_session_provider_key_prefers_runtime_identity_over_transport() {
    let _lock = lock_env();
    let _runtime = EnvVarGuard::set("JCODE_RUNTIME_PROVIDER", "azure-openai");
    let _namespace = EnvVarGuard::set("JCODE_OPENROUTER_CACHE_NAMESPACE", "azure-cache");
    let _active = EnvVarGuard::set("JCODE_ACTIVE_PROVIDER", "openrouter");

    assert_eq!(
        derive_session_provider_key("openrouter").as_deref(),
        Some("azure-openai")
    );
}

#[test]
fn derive_session_provider_key_falls_back_to_openrouter_namespace() {
    let _lock = lock_env();
    let _runtime = EnvVarGuard::remove("JCODE_RUNTIME_PROVIDER");
    let _namespace = EnvVarGuard::set("JCODE_OPENROUTER_CACHE_NAMESPACE", "azure-openai");
    let _active = EnvVarGuard::set("JCODE_ACTIVE_PROVIDER", "openrouter");

    assert_eq!(
        derive_session_provider_key("openrouter").as_deref(),
        Some("azure-openai")
    );
}

#[test]
fn derive_session_provider_key_keeps_openai_compatible_profile_namespace() {
    let _lock = lock_env();
    let _runtime = EnvVarGuard::set("JCODE_RUNTIME_PROVIDER", "openai-compatible");
    let _namespace = EnvVarGuard::set("JCODE_OPENROUTER_CACHE_NAMESPACE", "zai");
    let _active = EnvVarGuard::set("JCODE_ACTIVE_PROVIDER", "openrouter");

    assert_eq!(
        derive_session_provider_key("openrouter").as_deref(),
        Some("zai")
    );
}

#[test]
fn rename_title_preserves_generated_title_for_clear() {
    let mut session = Session::create_with_id(
        "session_rename_clear_123".to_string(),
        None,
        Some("Generated first prompt title".to_string()),
    );

    assert_eq!(
        session.display_title(),
        Some("Generated first prompt title")
    );
    session.rename_title(Some("Custom planning name".to_string()));
    assert_eq!(
        session.title.as_deref(),
        Some("Generated first prompt title")
    );
    assert_eq!(
        session.custom_title.as_deref(),
        Some("Custom planning name")
    );
    assert_eq!(session.display_title(), Some("Custom planning name"));

    session.rename_title(None);
    assert_eq!(
        session.title.as_deref(),
        Some("Generated first prompt title")
    );
    assert!(session.custom_title.is_none());
    assert_eq!(
        session.display_title(),
        Some("Generated first prompt title")
    );

    session.custom_title = Some("   ".to_string());
    assert_eq!(
        session.display_title(),
        Some("Generated first prompt title")
    );
}

#[test]
fn test_debug_memory_profile_reports_messages_and_provider_cache() {
    let mut session = Session::create_with_id(
        "session_memory_profile_test".to_string(),
        None,
        Some("Memory profile".to_string()),
    );
    session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "hello world".to_string(),
            cache_control: None,
        }],
    );
    session.add_message(
        Role::Assistant,
        vec![
            ContentBlock::ToolUse {
                id: "tool_1".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({"command": "echo hi"}),
                thought_signature: None,
            },
            ContentBlock::ToolResult {
                tool_use_id: "tool_1".to_string(),
                content: "hi".to_string(),
                is_error: None,
            },
        ],
    );

    session.compaction = Some(StoredCompactionState {
        summary_text: "summary".to_string(),
        openai_encrypted_content: None,
        covers_up_to_turn: 7,
        original_turn_count: 9,
        compacted_count: 7,
    });

    let _ = session.provider_messages();
    let profile = session.debug_memory_profile();

    assert_eq!(profile["messages"]["count"], 2);
    assert_eq!(profile["messages"]["memory"]["text_blocks"], 1);
    assert_eq!(profile["messages"]["memory"]["tool_use_blocks"], 1);
    assert_eq!(profile["messages"]["memory"]["tool_result_blocks"], 1);
    assert!(profile["messages"]["json_bytes"].as_u64().unwrap_or(0) > 0);
    assert_eq!(profile["provider_messages_cache"]["count"], 2);
    assert_eq!(profile["compaction"]["present"], true);
    assert_eq!(profile["compaction"]["covers_up_to_turn"], 7);
    assert_eq!(profile["compaction"]["original_turn_count"], 9);
    assert_eq!(profile["compaction"]["compacted_count"], 7);
    assert!(
        profile["provider_messages_cache"]["json_bytes"]
            .as_u64()
            .unwrap_or(0)
            > 0
    );
}

#[test]
fn token_usage_totals_counts_cache_reported_inputs_only_when_cache_fields_exist() {
    let mut session = Session::create_with_id(
        "session_token_usage_totals_test".to_string(),
        None,
        Some("Token totals".to_string()),
    );
    session.add_message_ext(
        Role::Assistant,
        vec![ContentBlock::Text {
            text: "first".to_string(),
            cache_control: None,
        }],
        None,
        Some(StoredTokenUsage {
            input_tokens: 100,
            output_tokens: 10,
            cache_read_input_tokens: None,
            cache_creation_input_tokens: None,
        }),
    );
    session.add_message_ext(
        Role::Assistant,
        vec![ContentBlock::Text {
            text: "second".to_string(),
            cache_control: None,
        }],
        None,
        Some(StoredTokenUsage {
            input_tokens: 200,
            output_tokens: 20,
            cache_read_input_tokens: Some(150),
            cache_creation_input_tokens: Some(25),
        }),
    );

    let totals = session.token_usage_totals();
    assert_eq!(totals.messages_with_token_usage, 2);
    assert_eq!(totals.input_tokens, 300);
    assert_eq!(totals.output_tokens, 30);
    assert_eq!(totals.cache_reported_input_tokens, 200);
    assert_eq!(totals.cache_read_input_tokens, 150);
    assert_eq!(totals.cache_creation_input_tokens, 25);
}

#[test]
fn initial_session_context_is_persisted_once_and_not_overwritten() {
    let mut session = Session::create_with_id(
        "session_context_test".to_string(),
        None,
        Some("Session context".to_string()),
    );

    assert!(session.ensure_initial_session_context_message());
    assert_eq!(session.messages.len(), 1);
    let first = session.messages[0].content_preview();
    assert!(first.contains("# Session Context"));
    assert!(first.contains("OS:"));
    assert_eq!(
        session.messages[0].display_role,
        Some(StoredDisplayRole::System)
    );

    assert!(!session.ensure_initial_session_context_message());
    assert_eq!(session.messages.len(), 1);

    session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "hello".to_string(),
            cache_control: None,
        }],
    );
    assert!(!session.ensure_initial_session_context_message());
    assert_eq!(session.messages.len(), 2);
}

#[test]
#[allow(clippy::redundant_closure_call)]
fn initial_session_context_uses_current_cwd_when_inserted() -> Result<()> {
    let _env_lock = lock_env();
    let original_cwd = std::env::current_dir().map_err(|e| anyhow!(e))?;
    let first_dir = tempfile::Builder::new()
        .prefix("jcode-session-context-first-")
        .tempdir()
        .map_err(|e| anyhow!(e))?;
    let second_dir = tempfile::Builder::new()
        .prefix("jcode-session-context-second-")
        .tempdir()
        .map_err(|e| anyhow!(e))?;

    std::env::set_current_dir(first_dir.path()).map_err(|e| anyhow!(e))?;
    let mut session = Session::create_with_id(
        "session_context_cwd_refresh_test".to_string(),
        None,
        Some("Session context cwd refresh".to_string()),
    );
    assert_eq!(
        session.working_dir.as_deref(),
        Some(first_dir.path().to_str().unwrap())
    );

    std::env::set_current_dir(second_dir.path()).map_err(|e| anyhow!(e))?;
    let result: std::result::Result<(), anyhow::Error> = (|| {
        assert!(session.ensure_initial_session_context_message());
        let first = session.messages[0].content_preview();
        assert!(
            first.contains(&format!(
                "Working directory: {}",
                second_dir.path().display()
            )),
            "session context should use cwd at insertion time, got: {first}"
        );
        assert_eq!(
            session.working_dir.as_deref(),
            Some(second_dir.path().to_str().unwrap())
        );
        Ok(())
    })();
    std::env::set_current_dir(original_cwd).map_err(|e| anyhow!(e))?;
    result?;

    Ok(())
}

#[test]
#[allow(clippy::redundant_closure_call)]
fn initial_session_context_can_refresh_before_real_conversation() -> Result<()> {
    let _env_lock = lock_env();
    let original_cwd = std::env::current_dir().map_err(|e| anyhow!(e))?;
    let first_dir = tempfile::Builder::new()
        .prefix("jcode-session-context-stale-")
        .tempdir()
        .map_err(|e| anyhow!(e))?;
    let second_dir = tempfile::Builder::new()
        .prefix("jcode-session-context-real-")
        .tempdir()
        .map_err(|e| anyhow!(e))?;

    std::env::set_current_dir(first_dir.path()).map_err(|e| anyhow!(e))?;
    let result: std::result::Result<(), anyhow::Error> = (|| {
        let mut session = Session::create_with_id(
            "session_context_remote_cwd_refresh_test".to_string(),
            None,
            Some("Remote cwd refresh".to_string()),
        );
        assert!(session.ensure_initial_session_context_message());
        assert!(session.messages[0].content_preview().contains(&format!(
            "Working directory: {}",
            first_dir.path().display()
        )));

        session.working_dir = Some(second_dir.path().display().to_string());
        assert!(session.refresh_initial_session_context_message());
        let refreshed = session.messages[0].content_preview();
        assert!(
            refreshed.contains(&format!(
                "Working directory: {}",
                second_dir.path().display()
            )),
            "session context should refresh to subscribed cwd, got: {refreshed}"
        );
        assert!(!refreshed.contains(&format!(
            "Working directory: {}",
            first_dir.path().display()
        )));
        Ok(())
    })();
    std::env::set_current_dir(original_cwd).map_err(|e| anyhow!(e))?;
    result?;

    Ok(())
}

#[test]
#[allow(clippy::redundant_closure_call)]
fn initial_session_context_does_not_refresh_after_real_conversation() -> Result<()> {
    let _env_lock = lock_env();
    let original_cwd = std::env::current_dir().map_err(|e| anyhow!(e))?;
    let first_dir = tempfile::Builder::new()
        .prefix("jcode-session-context-original-")
        .tempdir()
        .map_err(|e| anyhow!(e))?;
    let second_dir = tempfile::Builder::new()
        .prefix("jcode-session-context-late-")
        .tempdir()
        .map_err(|e| anyhow!(e))?;

    std::env::set_current_dir(first_dir.path()).map_err(|e| anyhow!(e))?;
    let result: std::result::Result<(), anyhow::Error> = (|| {
        let mut session = Session::create_with_id(
            "session_context_late_cwd_refresh_test".to_string(),
            None,
            Some("Late cwd refresh".to_string()),
        );
        assert!(session.ensure_initial_session_context_message());
        session.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: "hello".to_string(),
                cache_control: None,
            }],
        );

        session.working_dir = Some(second_dir.path().display().to_string());
        assert!(!session.refresh_initial_session_context_message());
        let original = session.messages[0].content_preview();
        assert!(original.contains(&format!(
            "Working directory: {}",
            first_dir.path().display()
        )));
        assert!(!original.contains(&format!(
            "Working directory: {}",
            second_dir.path().display()
        )));
        Ok(())
    })();
    std::env::set_current_dir(original_cwd).map_err(|e| anyhow!(e))?;
    result?;

    Ok(())
}

#[test]
fn existing_non_empty_session_does_not_get_retroactive_session_context() {
    let mut session = Session::create_with_id(
        "session_context_existing_test".to_string(),
        None,
        Some("Existing".to_string()),
    );
    session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "already started".to_string(),
            cache_control: None,
        }],
    );

    assert!(!session.ensure_initial_session_context_message());
    assert_eq!(session.messages.len(), 1);
    assert!(
        !session.messages[0]
            .content_preview()
            .contains("# Session Context")
    );
}

#[test]
fn load_startup_stub_preserves_metadata_but_skips_heavy_vectors() -> Result<()> {
    let _env_lock = lock_env();
    let temp_home = tempfile::Builder::new()
        .prefix("jcode-startup-stub-test-")
        .tempdir()
        .map_err(|e| anyhow!(e))?;
    let _home = EnvVarGuard::set("JCODE_HOME", temp_home.path().as_os_str());

    let session_id = "session_startup_stub_roundtrip";
    let mut session = Session::create_with_id(
        session_id.to_string(),
        Some("parent_123".to_string()),
        Some("startup stub".to_string()),
    );
    session.model = Some("gpt-5.4".to_string());
    session.reasoning_effort = Some("high".to_string());
    session.provider_key = Some("openai".to_string());
    session.route_api_method = Some("openai-api".to_string());
    session.set_canary("self-dev");
    session.append_stored_message(StoredMessage {
        id: "msg_1".to_string(),
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "hello world".to_string(),
            cache_control: None,
        }],
        display_role: None,
        timestamp: Some(Utc::now()),
        tool_duration_ms: None,
        token_usage: None,
    });
    session.record_env_snapshot(EnvSnapshot {
        captured_at: Utc::now(),
        reason: "resume".to_string(),
        session_id: session_id.to_string(),
        working_dir: Some(temp_home.path().to_string_lossy().to_string()),
        provider: "openai".to_string(),
        model: "gpt-5.4".to_string(),
        jcode_version: "test".to_string(),
        jcode_git_hash: Some("abc123".to_string()),
        jcode_git_dirty: Some(false),
        os: "linux".to_string(),
        arch: "x86_64".to_string(),
        pid: 123,
        is_selfdev: true,
        is_debug: false,
        is_canary: true,
        testing_build: Some("self-dev".to_string()),
        working_git: None,
    });
    session.record_memory_injection(
        "summary".to_string(),
        "content".to_string(),
        1,
        5,
        Vec::new(),
    );
    session.record_replay_display_message("system", Some("Launch".to_string()), "boot");
    session.save()?;

    let stub = Session::load_startup_stub(session_id)?;
    assert_eq!(stub.id, session_id);
    assert_eq!(stub.parent_id.as_deref(), Some("parent_123"));
    assert_eq!(stub.title.as_deref(), Some("startup stub"));
    assert_eq!(stub.model.as_deref(), Some("gpt-5.4"));
    assert_eq!(stub.reasoning_effort.as_deref(), Some("high"));
    assert_eq!(stub.provider_key.as_deref(), Some("openai"));
    assert_eq!(stub.route_api_method.as_deref(), Some("openai-api"));
    assert!(stub.is_canary);
    assert!(stub.messages.is_empty());
    assert!(stub.env_snapshots.is_empty());
    assert!(stub.memory_injections.is_empty());
    assert!(stub.replay_events.is_empty());
    Ok(())
}

#[test]
fn load_for_remote_startup_preserves_messages_and_replay_but_skips_heavy_vectors() -> Result<()> {
    let _env_lock = lock_env();
    let temp_home = tempfile::Builder::new()
        .prefix("jcode-remote-startup-test-")
        .tempdir()
        .map_err(|e| anyhow!(e))?;
    let _home = EnvVarGuard::set("JCODE_HOME", temp_home.path().as_os_str());

    let session_id = "session_remote_startup_roundtrip";
    let mut session = Session::create_with_id(
        session_id.to_string(),
        Some("parent_remote".to_string()),
        Some("remote startup".to_string()),
    );
    session.model = Some("gpt-5.4".to_string());
    session.reasoning_effort = Some("medium".to_string());
    session.append_stored_message(StoredMessage {
        id: "msg_remote_1".to_string(),
        role: Role::Assistant,
        content: vec![ContentBlock::Text {
            text: "hello remote startup".to_string(),
            cache_control: None,
        }],
        display_role: None,
        timestamp: Some(Utc::now()),
        tool_duration_ms: None,
        token_usage: None,
    });
    session.record_env_snapshot(EnvSnapshot {
        captured_at: Utc::now(),
        reason: "resume".to_string(),
        session_id: session_id.to_string(),
        working_dir: Some(temp_home.path().to_string_lossy().to_string()),
        provider: "openai".to_string(),
        model: "gpt-5.4".to_string(),
        jcode_version: "test".to_string(),
        jcode_git_hash: Some("abc123".to_string()),
        jcode_git_dirty: Some(false),
        os: "linux".to_string(),
        arch: "x86_64".to_string(),
        pid: 123,
        is_selfdev: false,
        is_debug: false,
        is_canary: false,
        testing_build: None,
        working_git: None,
    });
    session.record_memory_injection(
        "summary".to_string(),
        "content".to_string(),
        1,
        5,
        Vec::new(),
    );
    session.record_replay_display_message("system", Some("Launch".to_string()), "boot");
    session.save()?;

    let loaded = Session::load_for_remote_startup(session_id)?;
    assert_eq!(loaded.id, session_id);
    assert_eq!(loaded.parent_id.as_deref(), Some("parent_remote"));
    assert_eq!(loaded.model.as_deref(), Some("gpt-5.4"));
    assert_eq!(loaded.reasoning_effort.as_deref(), Some("medium"));
    assert_eq!(loaded.messages.len(), 1);
    assert!(loaded.replay_events.is_empty());
    assert!(loaded.env_snapshots.is_empty());
    assert!(loaded.memory_injections.is_empty());
    Ok(())
}

#[test]
fn test_create_marks_debug_when_test_session_env_enabled() {
    let _env_lock = lock_env();
    let _test_flag = EnvVarGuard::set("JCODE_TEST_SESSION", "1");

    let s1 = Session::create(None, None);
    assert!(s1.is_debug);

    let s2 = Session::create_with_id("session_test_1".to_string(), None, None);
    assert!(s2.is_debug);
}

#[test]
fn test_create_not_debug_when_test_session_env_disabled() {
    let _env_lock = lock_env();
    let _test_flag = EnvVarGuard::set("JCODE_TEST_SESSION", "0");

    let s = Session::create(None, None);
    assert!(!s.is_debug);
}

#[test]
fn test_recover_crashed_sessions_preserves_debug_flag() -> Result<()> {
    let _env_lock = lock_env();
    let temp_home = tempfile::Builder::new()
        .prefix("jcode-recover-debug-test-")
        .tempdir()
        .map_err(|e| anyhow!(e))?;
    let _home = EnvVarGuard::set("JCODE_HOME", temp_home.path().as_os_str());
    let _test_flag = EnvVarGuard::set("JCODE_TEST_SESSION", "0");

    let mut crashed = Session::create_with_id(
        "session_recover_debug_source".to_string(),
        None,
        Some("debug source".to_string()),
    );
    crashed.is_debug = true;
    crashed.mark_crashed(Some("test crash".to_string()));
    crashed.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "hello".to_string(),
            cache_control: None,
        }],
    );
    crashed.save()?;

    let recovered_ids = recover_crashed_sessions()?;
    assert_eq!(recovered_ids.len(), 1);

    let recovered = Session::load(&recovered_ids[0])?;
    assert!(recovered.is_debug);
    Ok(())
}

#[test]
fn test_recover_crashed_sessions_by_ids_restores_only_selected_group() -> Result<()> {
    let _env_lock = lock_env();
    let temp_home = tempfile::Builder::new()
        .prefix("jcode-recover-selected-crash-test-")
        .tempdir()
        .map_err(|e| anyhow!(e))?;
    let _home = EnvVarGuard::set("JCODE_HOME", temp_home.path().as_os_str());
    let _test_flag = EnvVarGuard::set("JCODE_TEST_SESSION", "0");

    let now = Utc::now();
    for (id, active_at) in [
        ("session_selected_crash", now),
        (
            "session_stale_unselected_crash",
            now - chrono::Duration::minutes(5),
        ),
    ] {
        let mut crashed = Session::create_with_id(id.to_string(), None, Some(id.to_string()));
        crashed.mark_crashed(Some("test crash".to_string()));
        crashed.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: format!("message from {id}"),
                cache_control: None,
            }],
        );
        crashed.last_active_at = Some(active_at);
        crashed.save()?;
    }

    let recovered_ids = recover_crashed_sessions_by_ids(&["session_selected_crash".to_string()])?;
    assert_eq!(recovered_ids.len(), 1);

    let recovered = Session::load(&recovered_ids[0])?;
    assert_eq!(
        recovered.parent_id.as_deref(),
        Some("session_selected_crash")
    );
    let stale = Session::load("session_stale_unselected_crash")?;
    assert!(matches!(stale.status, SessionStatus::Crashed { .. }));
    Ok(())
}

#[test]
fn test_save_persists_full_session_content() -> Result<()> {
    let _env_lock = lock_env();
    let temp_home = tempfile::Builder::new()
        .prefix("jcode-session-save-test-")
        .tempdir()
        .map_err(|e| anyhow!(e))?;
    let _home = EnvVarGuard::set("JCODE_HOME", temp_home.path().as_os_str());

    let mut session = Session::create_with_id(
        "session_save_persist_test".to_string(),
        None,
        Some("save fidelity test".to_string()),
    );

    session.add_message(
        Role::User,
        vec![ContentBlock::ToolResult {
            tool_use_id: "tool_1".to_string(),
            content: "OPENROUTER_API_KEY=sk-or-v1-abcdefghijklmnopqrstuvwxyz0123456789".to_string(),
            is_error: None,
        }],
    );

    session.add_message(
        Role::Assistant,
        vec![ContentBlock::ToolUse {
            id: "tool_2".to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({
                "command": "echo ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123"
            }),
            thought_signature: None,
        }],
    );

    session.save()?;

    let loaded = Session::load("session_save_persist_test")?;

    let ContentBlock::ToolResult { content, .. } = &loaded.messages[0].content[0] else {
        return Err(anyhow!("expected tool result block"));
    };
    assert!(content.contains("sk-or-v1-abcdefghijklmnopqrstuvwxyz0123456789"));
    assert!(!content.contains("[REDACTED_SECRET]"));

    let ContentBlock::ToolUse { input, .. } = &loaded.messages[1].content[0] else {
        return Err(anyhow!("expected tool use block"));
    };
    let input_str = input.to_string();
    assert!(input_str.contains("ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123"));
    assert!(!input_str.contains("[REDACTED_SECRET]"));
    Ok(())
}

#[test]
fn test_save_persists_compaction_state() -> Result<()> {
    let _env_lock = lock_env();
    let temp_home = tempfile::Builder::new()
        .prefix("jcode-session-compaction-save-test-")
        .tempdir()
        .map_err(|e| anyhow!(e))?;
    let _home = EnvVarGuard::set("JCODE_HOME", temp_home.path().as_os_str());

    let mut session = Session::create_with_id(
        "session_compaction_persist_test".to_string(),
        None,
        Some("compaction persistence test".to_string()),
    );
    session.compaction = Some(StoredCompactionState {
        summary_text: "saved summary".to_string(),
        openai_encrypted_content: None,
        covers_up_to_turn: 8,
        original_turn_count: 8,
        compacted_count: 8,
    });

    session.save()?;

    let loaded = Session::load("session_compaction_persist_test")?;
    assert_eq!(loaded.compaction, session.compaction);
    Ok(())
}

#[test]
fn test_save_persists_provider_key() -> Result<()> {
    let _env_lock = lock_env();
    let temp_home = tempfile::Builder::new()
        .prefix("jcode-session-provider-key-save-test-")
        .tempdir()
        .map_err(|e| anyhow!(e))?;
    let _home = EnvVarGuard::set("JCODE_HOME", temp_home.path().as_os_str());

    let mut session = Session::create_with_id(
        "session_provider_key_persist_test".to_string(),
        None,
        Some("provider key persistence test".to_string()),
    );
    session.provider_key = Some("opencode".to_string());
    session.model = Some("anthropic/claude-sonnet-4".to_string());

    session.save()?;

    let loaded = Session::load("session_provider_key_persist_test")?;
    assert_eq!(loaded.provider_key.as_deref(), Some("opencode"));
    assert_eq!(loaded.model.as_deref(), Some("anthropic/claude-sonnet-4"));
    Ok(())
}

#[test]
fn test_save_persists_reasoning_effort() -> Result<()> {
    let _env_lock = lock_env();
    let temp_home = tempfile::Builder::new()
        .prefix("jcode-session-reasoning-effort-save-test-")
        .tempdir()
        .map_err(|e| anyhow!(e))?;
    let _home = EnvVarGuard::set("JCODE_HOME", temp_home.path().as_os_str());

    let mut session = Session::create_with_id(
        "session_reasoning_effort_persist_test".to_string(),
        None,
        Some("reasoning effort persistence test".to_string()),
    );
    session.model = Some("gpt-5.4".to_string());
    session.reasoning_effort = Some("xhigh".to_string());

    session.save()?;

    let loaded = Session::load("session_reasoning_effort_persist_test")?;
    assert_eq!(loaded.model.as_deref(), Some("gpt-5.4"));
    assert_eq!(loaded.reasoning_effort.as_deref(), Some("xhigh"));
    Ok(())
}

#[test]
fn test_save_appends_journal_and_load_replays_it() -> Result<()> {
    let _env_lock = lock_env();
    let temp_home = tempfile::Builder::new()
        .prefix("jcode-session-journal-test-")
        .tempdir()
        .map_err(|e| anyhow!(e))?;
    let _home = EnvVarGuard::set("JCODE_HOME", temp_home.path().as_os_str());

    let mut session = Session::create_with_id(
        "session_journal_append_test".to_string(),
        None,
        Some("journal append test".to_string()),
    );
    session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "first".to_string(),
            cache_control: None,
        }],
    );
    session.save()?;

    let snapshot_path = session_path("session_journal_append_test")?;
    let journal_path = session_journal_path("session_journal_append_test")?;
    assert!(snapshot_path.exists());
    assert!(!journal_path.exists());

    session.add_message(
        Role::Assistant,
        vec![ContentBlock::Text {
            text: "second".to_string(),
            cache_control: None,
        }],
    );
    session.save()?;

    assert!(journal_path.exists());
    let journal = std::fs::read_to_string(&journal_path)?;
    assert!(journal.contains("second"));

    let loaded = Session::load("session_journal_append_test")?;
    assert_eq!(loaded.messages.len(), 2);
    assert_eq!(loaded.messages[1].content_preview(), "second");
    Ok(())
}

#[test]
fn test_save_checkpoints_after_full_mutation_and_clears_journal() -> Result<()> {
    let _env_lock = lock_env();
    let temp_home = tempfile::Builder::new()
        .prefix("jcode-session-checkpoint-test-")
        .tempdir()
        .map_err(|e| anyhow!(e))?;
    let _home = EnvVarGuard::set("JCODE_HOME", temp_home.path().as_os_str());

    let mut session = Session::create_with_id(
        "session_journal_checkpoint_test".to_string(),
        None,
        Some("checkpoint test".to_string()),
    );
    session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "one".to_string(),
            cache_control: None,
        }],
    );
    session.save()?;

    session.add_message(
        Role::Assistant,
        vec![ContentBlock::Text {
            text: "two".to_string(),
            cache_control: None,
        }],
    );
    session.save()?;

    let journal_path = session_journal_path("session_journal_checkpoint_test")?;
    assert!(journal_path.exists());

    session.truncate_messages(1);
    session.title = Some("checkpointed title".to_string());
    session.save()?;

    assert!(!journal_path.exists());

    let loaded = Session::load("session_journal_checkpoint_test")?;
    assert_eq!(loaded.title.as_deref(), Some("checkpointed title"));
    assert_eq!(loaded.messages.len(), 1);
    assert_eq!(loaded.messages[0].content_preview(), "one");
    Ok(())
}

#[test]
fn test_redacted_for_export_redacts_tool_result_and_tool_input() -> Result<()> {
    let mut session = Session::create_with_id(
        "session_redact_persist_test".to_string(),
        None,
        Some("redaction test".to_string()),
    );

    session.add_message(
        Role::User,
        vec![ContentBlock::ToolResult {
            tool_use_id: "tool_1".to_string(),
            content: "OPENROUTER_API_KEY=sk-or-v1-abcdefghijklmnopqrstuvwxyz0123456789".to_string(),
            is_error: None,
        }],
    );

    session.add_message(
        Role::Assistant,
        vec![ContentBlock::ToolUse {
            id: "tool_2".to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({
                "command": "echo ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123"
            }),
            thought_signature: None,
        }],
    );

    let persisted = session.redacted_for_export();

    let first_content = &persisted.messages[0].content[0];
    let ContentBlock::ToolResult { content, .. } = first_content else {
        return Err(anyhow!("expected tool result block"));
    };
    assert!(content.contains("OPENROUTER_API_KEY=[REDACTED_SECRET]"));
    assert!(!content.contains("sk-or-v1-abcdefghijklmnopqrstuvwxyz0123456789"));

    let second_content = &persisted.messages[1].content[0];
    let ContentBlock::ToolUse { input, .. } = second_content else {
        return Err(anyhow!("expected tool use block"));
    };
    let input_str = input.to_string();
    assert!(input_str.contains("[REDACTED_SECRET]"));
    assert!(!input_str.contains("ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123"));
    Ok(())
}

#[test]
fn test_redacted_for_export_redacts_replay_events() -> Result<()> {
    let mut session = Session::create_with_id(
        "session_redacted_replay_events_test".to_string(),
        None,
        Some("redacted replay events".to_string()),
    );

    session.record_replay_display_message(
        "swarm",
        Some("DM from fox".to_string()),
        "OPENROUTER_API_KEY=sk-or-v1-secret-value",
    );
    session.record_swarm_status_event(vec![crate::protocol::SwarmMemberStatus {
        session_id: "session_fox".to_string(),
        friendly_name: Some("fox".to_string()),
        status: "running".to_string(),
        detail: Some("ANTHROPIC_API_KEY=sk-ant-secret-value".to_string()),
        role: Some("agent".to_string()),
        is_headless: None,
        live_attachments: None,
        status_age_secs: None,
    }]);
    session.record_swarm_plan_event(
        "swarm_test".to_string(),
        1,
        vec![crate::plan::PlanItem {
            content: "OPENROUTER_API_KEY=sk-or-v1-abcdefghijklmnopqrstuvwxyz0123456789".to_string(),
            status: "pending".to_string(),
            priority: "high".to_string(),
            id: "task-1".to_string(),
            subsystem: None,
            file_scope: Vec::new(),
            blocked_by: vec![],
            assigned_to: None,
        }],
        vec![],
        Some("ANTHROPIC_API_KEY=sk-ant-secret-value".to_string()),
    );

    let redacted = session.redacted_for_export();
    assert_eq!(redacted.replay_events.len(), 3);

    let StoredReplayEventKind::DisplayMessage { content, .. } = &redacted.replay_events[0].kind
    else {
        return Err(anyhow!("expected display message replay event"));
    };
    assert!(content.contains("OPENROUTER_API_KEY=[REDACTED_SECRET]"));
    assert!(!content.contains("sk-or-v1-secret-value"));

    let StoredReplayEventKind::SwarmStatus { members } = &redacted.replay_events[1].kind else {
        return Err(anyhow!("expected swarm status replay event"));
    };
    let detail = members[0].detail.as_deref().unwrap_or_default();
    assert!(detail.contains("ANTHROPIC_API_KEY=[REDACTED_SECRET]"));
    assert!(!detail.contains("sk-ant-secret-value"));

    let StoredReplayEventKind::SwarmPlan { items, reason, .. } = &redacted.replay_events[2].kind
    else {
        return Err(anyhow!("expected swarm plan replay event"));
    };
    assert!(
        items[0]
            .content
            .contains("OPENROUTER_API_KEY=[REDACTED_SECRET]")
    );
    assert!(
        !items[0]
            .content
            .contains("sk-or-v1-abcdefghijklmnopqrstuvwxyz0123456789")
    );
    let reason = reason.as_deref().unwrap_or_default();
    assert!(reason.contains("ANTHROPIC_API_KEY=[REDACTED_SECRET]"));
    assert!(!reason.contains("sk-ant-secret-value"));
    Ok(())
}

#[test]
fn test_summarize_tool_calls_includes_tool_only_assistant_messages() {
    let mut session = Session::create_with_id(
        "session_tool_summary_test".to_string(),
        None,
        Some("tool summary test".to_string()),
    );

    session.add_message(
        Role::Assistant,
        vec![ContentBlock::ToolUse {
            id: "tool_1".to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({
                "command": "pwd"
            }),
            thought_signature: None,
        }],
    );

    let summaries = summarize_tool_calls(&session, 10);
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].tool_name, "bash");
    assert!(summaries[0].brief_output.contains("pwd"));
}

#[test]
fn test_render_messages_honors_system_display_role_override() {
    let mut session = Session::create_with_id(
        "session_display_role_test".to_string(),
        None,
        Some("display role test".to_string()),
    );

    session.add_message_with_display_role(
        Role::User,
        vec![ContentBlock::Text {
            text: "[Background Task Completed]\nTask: abc123 (bash)".to_string(),
            cache_control: None,
        }],
        Some(StoredDisplayRole::System),
    );

    let rendered = render_messages(&session);
    assert_eq!(rendered.len(), 1);
    assert_eq!(rendered[0].role, "system");
    assert!(rendered[0].content.contains("Background Task Completed"));
}

#[test]
fn test_render_messages_renders_persisted_reasoning() {
    use jcode_render_core::REASONING_SENTINEL;

    let _env_lock = lock_env();
    let _mode = EnvVarGuard::set("JCODE_REASONING_DISPLAY", "full");
    crate::config::invalidate_config_cache();

    let mut session = Session::create_with_id(
        "session_render_reasoning_test".to_string(),
        None,
        Some("render reasoning test".to_string()),
    );

    session.add_message(
        Role::Assistant,
        vec![
            ContentBlock::ReasoningTrace {
                text: "step one\nstep two".to_string(),
            },
            ContentBlock::Text {
                text: "Here is the answer.".to_string(),
                cache_control: None,
            },
        ],
    );

    let rendered = render_messages(&session);
    assert_eq!(rendered.len(), 1);
    let content = &rendered[0].content;
    // Reasoning lines are rendered as dim/italic markup with the sentinel.
    assert!(
        content.contains(&format!("*{0}step one{0}*", REASONING_SENTINEL)),
        "expected reasoning markup, got: {content:?}"
    );
    assert!(
        content.contains(&format!("*{0}step two{0}*", REASONING_SENTINEL)),
        "expected reasoning markup, got: {content:?}"
    );
    // Answer text follows the reasoning block.
    assert!(content.contains("Here is the answer."));
    let reasoning_end = content.find("step two").unwrap();
    let answer_start = content.find("Here is the answer.").unwrap();
    assert!(
        reasoning_end < answer_start,
        "reasoning should precede the answer text: {content:?}"
    );
}

#[test]
fn test_render_messages_renders_legacy_reasoning_variant() {
    use jcode_render_core::REASONING_SENTINEL;

    let _env_lock = lock_env();
    let _mode = EnvVarGuard::set("JCODE_REASONING_DISPLAY", "full");
    crate::config::invalidate_config_cache();

    let mut session = Session::create_with_id(
        "session_render_legacy_reasoning_test".to_string(),
        None,
        Some("render legacy reasoning test".to_string()),
    );

    session.add_message(
        Role::Assistant,
        vec![ContentBlock::Reasoning {
            text: "legacy thought".to_string(),
        }],
    );

    let rendered = render_messages(&session);
    assert_eq!(rendered.len(), 1);
    assert!(
        rendered[0]
            .content
            .contains(&format!("*{0}legacy thought{0}*", REASONING_SENTINEL)),
        "expected legacy reasoning markup, got: {:?}",
        rendered[0].content
    );
}

#[test]
fn test_render_messages_hides_persisted_reasoning_in_current_mode() {
    use jcode_render_core::REASONING_SENTINEL;

    let _env_lock = lock_env();
    let _mode = EnvVarGuard::set("JCODE_REASONING_DISPLAY", "current");
    crate::config::invalidate_config_cache();

    let mut session = Session::create_with_id(
        "session_render_reasoning_current_test".to_string(),
        None,
        Some("render reasoning current test".to_string()),
    );

    session.add_message(
        Role::Assistant,
        vec![
            ContentBlock::ReasoningTrace {
                text: "step one\nstep two\nstep three".to_string(),
            },
            ContentBlock::Text {
                text: "Here is the answer.".to_string(),
                cache_control: None,
            },
        ],
    );

    let rendered = render_messages(&session);
    assert_eq!(rendered.len(), 1);
    let content = &rendered[0].content;
    // In `current` mode only the *live* reasoning block is ever shown; it streams
    // then is discarded once the model answers. Re-rendered history therefore
    // shows no past reasoning at all (no trace line, no lines, no sentinel).
    assert!(
        !content.contains(REASONING_SENTINEL),
        "no reasoning markup expected in current mode on reload: {content:?}"
    );
    assert!(
        !content.contains("step one")
            && !content.contains("step two")
            && !content.contains("thought"),
        "individual reasoning lines/trace must not be replayed in current mode: {content:?}"
    );
    // The answer text is preserved.
    assert!(content.contains("Here is the answer."));
}

#[test]
fn test_render_messages_hides_persisted_reasoning_in_off_mode() {
    use jcode_render_core::REASONING_SENTINEL;

    let _env_lock = lock_env();
    let _mode = EnvVarGuard::set("JCODE_REASONING_DISPLAY", "off");
    crate::config::invalidate_config_cache();

    let mut session = Session::create_with_id(
        "session_render_reasoning_off_test".to_string(),
        None,
        Some("render reasoning off test".to_string()),
    );

    session.add_message(
        Role::Assistant,
        vec![
            ContentBlock::ReasoningTrace {
                text: "secret thought".to_string(),
            },
            ContentBlock::Text {
                text: "Here is the answer.".to_string(),
                cache_control: None,
            },
        ],
    );

    let rendered = render_messages(&session);
    assert_eq!(rendered.len(), 1);
    let content = &rendered[0].content;
    assert!(
        !content.contains(REASONING_SENTINEL) && !content.contains("secret thought"),
        "reasoning must be hidden entirely in off mode: {content:?}"
    );
    assert!(content.contains("Here is the answer."));
}

#[test]
fn test_render_messages_honors_background_task_display_role_override() {
    let mut session = Session::create_with_id(
        "session_background_task_role_test".to_string(),
        None,
        Some("background task role test".to_string()),
    );

    session.add_message_with_display_role(
            Role::User,
            vec![ContentBlock::Text {
                text: "**Background task** `abc123` · `bash` · ✓ completed · 7.1s · exit 0\n\n_No output captured._\n\n_Full output:_ `bg action=\"output\" task_id=\"abc123\"`".to_string(),
                cache_control: None,
            }],
            Some(StoredDisplayRole::BackgroundTask),
        );

    let rendered = render_messages(&session);
    assert_eq!(rendered.len(), 1);
    assert_eq!(rendered[0].role, "background_task");
    assert!(rendered[0].content.contains("**Background task**"));
}

#[test]
fn test_render_messages_hides_internal_system_reminders() {
    let mut session = Session::create_with_id(
        "session_hidden_system_reminder_test".to_string(),
        None,
        Some("hidden reminder test".to_string()),
    );

    assert!(session.ensure_initial_session_context_message());
    session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "visible prompt".to_string(),
            cache_control: None,
        }],
    );

    let rendered = render_messages(&session);
    assert_eq!(rendered.len(), 1);
    assert_eq!(rendered[0].role, "user");
    assert_eq!(rendered[0].content, "visible prompt");
}

#[test]
fn test_render_messages_shows_recent_compacted_history_by_default() {
    let mut session = Session::create_with_id(
        "session_render_compacted_history_test".to_string(),
        None,
        Some("render compacted history test".to_string()),
    );

    session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "old prompt".to_string(),
            cache_control: None,
        }],
    );
    session.add_message(
        Role::Assistant,
        vec![ContentBlock::Text {
            text: "old response".to_string(),
            cache_control: None,
        }],
    );
    session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "current prompt".to_string(),
            cache_control: None,
        }],
    );
    session.compaction = Some(StoredCompactionState {
        summary_text: "old prompt and response".to_string(),
        openai_encrypted_content: None,
        covers_up_to_turn: 2,
        original_turn_count: 2,
        compacted_count: 2,
    });

    let rendered = render_messages(&session);
    assert_eq!(rendered.len(), 4);
    assert_eq!(rendered[0].role, "system");
    assert!(rendered[0].content.contains("showing all 2"));
    assert_eq!(rendered[1].role, "user");
    assert_eq!(rendered[1].content, "old prompt");
    assert_eq!(rendered[2].role, "assistant");
    assert_eq!(rendered[2].content, "old response");
    assert_eq!(rendered[3].role, "user");
    assert_eq!(rendered[3].content, "current prompt");
}

#[test]
fn test_render_messages_can_expand_compacted_history_window() {
    let mut session = Session::create_with_id(
        "session_render_compacted_history_expand_test".to_string(),
        None,
        Some("render compacted history expand test".to_string()),
    );

    session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "old prompt".to_string(),
            cache_control: None,
        }],
    );
    session.add_message(
        Role::Assistant,
        vec![ContentBlock::Text {
            text: "old response".to_string(),
            cache_control: None,
        }],
    );
    session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "current prompt".to_string(),
            cache_control: None,
        }],
    );
    session.compaction = Some(StoredCompactionState {
        summary_text: "old prompt and response".to_string(),
        openai_encrypted_content: None,
        covers_up_to_turn: 2,
        original_turn_count: 2,
        compacted_count: 2,
    });

    // A small compacted prefix (few renderable messages, a single turn) must
    // never be truncated, even when a tiny visible window is requested. A single
    // long turn in particular must always render in full.
    let (rendered, _images, info) = render_messages_and_images_with_compacted_history(&session, 1);
    let info = info.expect("compacted info");
    assert_eq!(info.total_messages, 2);
    assert_eq!(info.visible_messages, 2);
    assert_eq!(info.remaining_messages, 0);
    assert_eq!(info.hidden_user_prompts, 0);
    assert_eq!(rendered.len(), 4);
    assert!(rendered[0].content.contains("showing all 2"));
    assert_eq!(rendered[1].content, "old prompt");
    assert_eq!(rendered[2].content, "old response");
    assert_eq!(rendered[3].content, "current prompt");

    let (rendered_all, _images, info_all) =
        render_messages_and_images_with_compacted_history(&session, usize::MAX);
    let info_all = info_all.expect("compacted info");
    assert_eq!(info_all.visible_messages, 2);
    assert_eq!(info_all.remaining_messages, 0);
    assert_eq!(info_all.hidden_user_prompts, 0);
    assert_eq!(rendered_all.len(), 4);
    assert!(rendered_all[0].content.contains("showing all 2"));
    assert_eq!(rendered_all[1].content, "old prompt");
    assert_eq!(rendered_all[2].content, "old response");
    assert_eq!(rendered_all[3].content, "current prompt");
}

#[test]
fn test_compacted_history_truncates_only_when_long_and_many_turns() {
    let mut session = Session::create_with_id(
        "session_render_compacted_history_truncate_test".to_string(),
        None,
        Some("render compacted history truncate test".to_string()),
    );

    // Build a large compacted prefix: many turns, each with several visible
    // messages, well past both guardrails (>80 renderable, >5 turns).
    let prefix_turns = 20usize;
    for t in 0..prefix_turns {
        session.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: format!("prompt {t}"),
                cache_control: None,
            }],
        );
        // 4 assistant messages per turn -> 5 renderable per turn.
        for r in 0..4 {
            session.add_message(
                Role::Assistant,
                vec![ContentBlock::Text {
                    text: format!("response {t}.{r}"),
                    cache_control: None,
                }],
            );
        }
    }
    // Current (uncompacted) prompt after the compacted prefix.
    session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "current prompt".to_string(),
            cache_control: None,
        }],
    );

    let compacted_count = prefix_turns * 5; // every prefix message is compacted
    session.compaction = Some(StoredCompactionState {
        summary_text: "older compacted context".to_string(),
        openai_encrypted_content: None,
        covers_up_to_turn: prefix_turns,
        original_turn_count: prefix_turns,
        compacted_count,
    });

    let total_renderable = prefix_turns * 5; // 100

    // Request a small window: truncation kicks in because the prefix is long
    // and has many turns.
    let (rendered, _images, info) = render_messages_and_images_with_compacted_history(&session, 10);
    let info = info.expect("compacted info");
    assert_eq!(info.total_messages, total_renderable);
    assert!(info.visible_messages < total_renderable);
    assert!(info.remaining_messages > 0);
    assert_eq!(
        info.visible_messages + info.remaining_messages,
        total_renderable
    );
    assert!(info.hidden_user_prompts > 0);
    // The first rendered body message (after the marker) must be a user prompt
    // because we snap the window to a turn boundary.
    assert_eq!(rendered[1].role, "user");

    // Requesting everything shows the whole prefix with no hidden prompts.
    let (_rendered_all, _images, info_all) =
        render_messages_and_images_with_compacted_history(&session, usize::MAX);
    let info_all = info_all.expect("compacted info");
    assert_eq!(info_all.visible_messages, total_renderable);
    assert_eq!(info_all.remaining_messages, 0);
    assert_eq!(info_all.hidden_user_prompts, 0);
}

#[test]
fn test_compacted_history_never_truncates_single_long_turn() {
    let mut session = Session::create_with_id(
        "session_render_compacted_history_single_turn_test".to_string(),
        None,
        Some("render compacted history single turn test".to_string()),
    );

    // A single turn with a huge number of visible messages (well over 80).
    session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "the one long prompt".to_string(),
            cache_control: None,
        }],
    );
    for r in 0..150 {
        session.add_message(
            Role::Assistant,
            vec![ContentBlock::Text {
                text: format!("long response chunk {r}"),
                cache_control: None,
            }],
        );
    }
    session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "current prompt".to_string(),
            cache_control: None,
        }],
    );

    let compacted_count = 151; // prompt + 150 responses
    session.compaction = Some(StoredCompactionState {
        summary_text: "older compacted context".to_string(),
        openai_encrypted_content: None,
        covers_up_to_turn: 1,
        original_turn_count: 1,
        compacted_count,
    });

    // Even with a tiny requested window, a single long turn is never truncated.
    let (_rendered, _images, info) = render_messages_and_images_with_compacted_history(&session, 5);
    let info = info.expect("compacted info");
    assert_eq!(info.total_messages, compacted_count);
    assert_eq!(info.visible_messages, compacted_count);
    assert_eq!(info.remaining_messages, 0);
    assert_eq!(info.hidden_user_prompts, 0);
}

#[test]
fn test_compacted_history_window_counts_renderable_messages_not_hidden_reminders() {
    let mut session = Session::create_with_id(
        "session_render_compacted_history_hidden_budget_test".to_string(),
        None,
        Some("render compacted history hidden budget test".to_string()),
    );

    session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "older visible prompt".to_string(),
            cache_control: None,
        }],
    );
    session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "<system-reminder>hidden reminder one</system-reminder>".to_string(),
            cache_control: None,
        }],
    );
    session.add_message(
        Role::Assistant,
        vec![ContentBlock::Text {
            text: "previous visible assistant response".to_string(),
            cache_control: None,
        }],
    );
    session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "<system-reminder>hidden reminder two</system-reminder>".to_string(),
            cache_control: None,
        }],
    );
    session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "current prompt".to_string(),
            cache_control: None,
        }],
    );
    session.compaction = Some(StoredCompactionState {
        summary_text: "older compacted context".to_string(),
        openai_encrypted_content: None,
        covers_up_to_turn: 4,
        original_turn_count: 4,
        compacted_count: 4,
    });

    let (rendered, _images, info) = render_messages_and_images_with_compacted_history(&session, 1);
    let info = info.expect("compacted info");

    // Hidden system reminders are never counted as renderable messages, so the
    // small prefix (2 renderable, 1 turn) is shown in full rather than truncated.
    assert_eq!(info.total_messages, 2);
    assert_eq!(info.visible_messages, 2);
    assert_eq!(info.remaining_messages, 0);
    assert_eq!(info.hidden_user_prompts, 0);
    assert_eq!(rendered.len(), 4);
    assert!(rendered[0].content.contains("showing all 2"));
    assert_eq!(rendered[1].role, "user");
    assert_eq!(rendered[1].content, "older visible prompt");
    assert_eq!(rendered[2].role, "assistant");
    assert_eq!(rendered[2].content, "previous visible assistant response");
    assert_eq!(rendered[3].content, "current prompt");
    assert!(
        rendered
            .iter()
            .all(|msg| !msg.content.contains("hidden reminder"))
    );
}

#[test]
fn test_render_messages_and_images_share_tool_resolution_and_labels() {
    let mut session = Session::create_with_id(
        "session_render_bundle_test".to_string(),
        None,
        Some("render bundle test".to_string()),
    );

    session.add_message(
        Role::Assistant,
        vec![
            ContentBlock::ToolUse {
                id: "tool_img_1".to_string(),
                name: "view_image".to_string(),
                input: serde_json::json!({"file_path": "/tmp/screenshot.png"}),
                thought_signature: None,
            },
            ContentBlock::ToolResult {
                tool_use_id: "tool_img_1".to_string(),
                content: "rendered image".to_string(),
                is_error: None,
            },
            ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: "abcd".to_string(),
            },
            ContentBlock::Text {
                text: "[Attached image associated with the preceding tool result: screenshot.png]"
                    .to_string(),
                cache_control: None,
            },
        ],
    );

    let (rendered, images) = render_messages_and_images(&session);
    assert_eq!(rendered.len(), 2);
    assert_eq!(rendered[0].role, "tool");
    assert_eq!(rendered[0].content, "rendered image");
    assert_eq!(
        rendered[0]
            .tool_data
            .as_ref()
            .map(|tool| tool.name.as_str()),
        Some("view_image")
    );

    assert_eq!(images.len(), 1);
    assert_eq!(images[0].label.as_deref(), Some("screenshot.png"));
    assert_eq!(images[0].media_type, "image/png");
    assert_eq!(
        images[0].source,
        RenderedImageSource::ToolResult {
            tool_name: "view_image".to_string(),
        }
    );
}

#[test]
fn reasoning_trace_survives_session_save_and_load() -> Result<()> {
    let _env_lock = lock_env();
    let temp_home = tempfile::Builder::new()
        .prefix("jcode-reasoning-persist-test-")
        .tempdir()
        .map_err(|e| anyhow!(e))?;
    let _home = EnvVarGuard::set("JCODE_HOME", temp_home.path().as_os_str());

    let session_id = "session_reasoning_trace_roundtrip";
    let mut session = Session::create_with_id(session_id.to_string(), None, None);
    session.append_stored_message(StoredMessage {
        id: "msg_assistant".to_string(),
        role: Role::Assistant,
        content: vec![
            ContentBlock::ReasoningTrace {
                text: "step 1: consider the run loop ordering".to_string(),
            },
            ContentBlock::Text {
                text: "Here is my answer.".to_string(),
                cache_control: None,
            },
        ],
        display_role: None,
        timestamp: Some(Utc::now()),
        tool_duration_ms: None,
        token_usage: None,
    });
    session.save()?;

    // The reasoning must be persisted to the on-disk transcript, not just held
    // in memory, so it can be recalled/debugged after a restart.
    let raw = std::fs::read_to_string(session_path(session_id)?)?;
    assert!(
        raw.contains("reasoning_trace"),
        "transcript should serialize reasoning_trace block"
    );
    assert!(raw.contains("step 1: consider the run loop ordering"));

    let loaded = Session::load(session_id)?;
    let assistant = loaded
        .messages
        .iter()
        .find(|m| m.role == Role::Assistant)
        .ok_or_else(|| anyhow!("assistant message missing after reload"))?;
    let has_trace = assistant.content.iter().any(|b| {
        matches!(
            b,
            ContentBlock::ReasoningTrace { text }
                if text == "step 1: consider the run loop ordering"
        )
    });
    assert!(has_trace, "ReasoningTrace must survive save/load roundtrip");
    Ok(())
}
