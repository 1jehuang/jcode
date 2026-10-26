use super::*;
use crate::agent_profile::AgentProfile;
use crate::instruction_sources::{InstructionSourceKind, InstructionSourceSnapshot};

fn profile() -> AgentProfile {
    AgentProfile {
        name: "writer".into(),
        instructions: "CHOSEN_PROFILE_89341".into(),
        instruction_sources: Vec::new(),
    }
}

fn write(path: impl AsRef<std::path::Path>, content: &str) {
    let path = path.as_ref();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

async fn turn(agent: &mut Agent, streaming: bool) {
    agent.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "Continue the task".into(),
            cache_control: None,
        }],
    );
    if streaming {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        agent.resume_turn_streaming_mpsc(None, tx).await.unwrap();
    } else {
        agent.run_turn(false).await.unwrap();
    }
}

#[tokio::test]
async fn profile_requests_exclude_unselected_files_and_swarm_descriptions_on_both_paths() {
    let _home = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let jcode = crate::storage::jcode_dir().unwrap();
    write(
        crate::storage::user_home_path("AGENTS.md").unwrap(),
        "GLOBAL_AGENTS_FORBIDDEN_67431",
    );
    write(
        cwd.path().join("AGENTS.md"),
        "PROJECT_AGENTS_FORBIDDEN_67431",
    );
    for name in [
        "system-prompt.md",
        "prompt-overlay.md",
        "preferred-tools.md",
        "swarm-prompt.md",
    ] {
        write(
            jcode.join(name),
            &format!("GLOBAL_FILE_FORBIDDEN_67431_{name}"),
        );
        write(
            cwd.path().join(".jcode").join(name),
            &format!("PROJECT_FILE_FORBIDDEN_67431_{name}"),
        );
    }
    for streaming in [false, true] {
        let provider = Arc::new(PromptCaptureProvider::default());
        let registry = Registry::empty();
        registry
            .register(
                "swarm".into(),
                Arc::new(crate::tool::communicate::CommunicateTool::new()),
            )
            .await;
        let mut agent = Agent::new(provider.clone(), registry);
        agent.set_working_dir(cwd.path().to_str().unwrap());
        agent.memory_enabled = false;
        agent.session.system_prompt = Some("CUSTOM_FORBIDDEN_67431".into());
        // Pin the old description first: selecting must drop cached tools.
        let before = agent.tool_definitions().await;
        assert!(before.iter().any(|tool| {
            tool.description
                .contains("GLOBAL_FILE_FORBIDDEN_67431_swarm")
        }));
        agent.provider_session_id = Some("old-live-upstream".into());
        agent.session.provider_session_id = Some("old-durable-upstream".into());
        agent.set_agent_profile(Some(profile())).unwrap();
        assert!(agent.locked_tools.is_none());
        assert!(agent.session.provider_session_id.is_none());
        turn(&mut agent, streaming).await;
        assert_eq!(
            *provider.lifecycle.lock().unwrap(),
            vec!["reset", "request"]
        );
        assert_eq!(*provider.resume_hints.lock().unwrap(), vec![None]);
        let requests = provider.requests.lock().unwrap();
        let static_text = &requests[0].0;
        assert!(static_text.contains("You are Jcode."));
        assert_eq!(static_text.matches("CHOSEN_PROFILE_89341").count(), 1);
        assert!(!static_text.contains("FORBIDDEN_67431"));
        let tools = provider.tool_requests.lock().unwrap();
        let swarm = tools[0].iter().find(|tool| tool.name == "swarm").unwrap();
        assert!(!swarm.description.contains("FORBIDDEN_67431"));
        assert!(
            swarm
                .description
                .contains(crate::prompt::DEFAULT_SWARM_PROMPT.trim())
        );
    }
}

#[tokio::test]
async fn profile_selected_extras_are_additive_and_swarm_snapshot_replaces_disk_guidance() {
    let _home = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let overlay = cwd.path().join(".jcode/prompt-overlay.md");
    let swarm = crate::storage::jcode_dir().unwrap().join("swarm-prompt.md");
    write(&overlay, "CHOSEN_OVERLAY_59117");
    write(&swarm, "CHOSEN_SWARM_59117");
    let provider = Arc::new(PromptCaptureProvider::default());
    let registry = Registry::empty();
    registry
        .register(
            "swarm".into(),
            Arc::new(crate::tool::communicate::CommunicateTool::new()),
        )
        .await;
    let mut agent = Agent::new(provider.clone(), registry);
    agent.set_working_dir(cwd.path().to_str().unwrap());
    agent.memory_enabled = false;
    agent.session.system_prompt = Some(CUSTOM_PROMPT.into());
    let ids: Vec<_> =
        crate::instruction_sources::choices(Some(cwd.path()), Some(CUSTOM_PROMPT), &[])
            .into_iter()
            .filter(|source| source.available)
            .map(|source| source.id)
            .collect();
    let mut selected = profile();
    selected.instruction_sources = crate::instruction_sources::select(
        Some(cwd.path()),
        Some(CUSTOM_PROMPT),
        &[],
        &serde_json::to_string(&ids).unwrap(),
    )
    .unwrap();
    agent.set_agent_profile(Some(selected)).unwrap();
    write(&swarm, "DISK_SWARM_CHANGED_FORBIDDEN_59117");
    turn(&mut agent, true).await;
    let requests = provider.requests.lock().unwrap();
    assert!(requests[0].0.contains("You are Jcode."));
    assert!(requests[0].0.contains("CHOSEN_OVERLAY_59117"));
    assert!(requests[0].0.contains(CUSTOM_PROMPT.trim()));
    assert!(!requests[0].0.contains("CHOSEN_SWARM_59117"));
    let tools = provider.tool_requests.lock().unwrap();
    let description = &tools[0]
        .iter()
        .find(|tool| tool.name == "swarm")
        .unwrap()
        .description;
    assert!(description.contains("CHOSEN_SWARM_59117"));
    assert!(!description.contains("DISK_SWARM_CHANGED_FORBIDDEN_59117"));
    drop(tools);
    drop(requests);
    agent.set_agent_profile(None).unwrap();
    assert_eq!(
        agent.build_system_prompt_split(None).static_part,
        CUSTOM_PROMPT
    );
}

#[test]
fn profile_save_failure_restores_profile_and_upstream_state_without_resetting() {
    let _home = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let provider = Arc::new(PromptCaptureProvider::default());
    let mut agent = Agent::new(provider.clone(), Registry::empty());
    agent.session.agent_profile = Some(profile());
    agent.provider_session_id = Some("live-upstream".into());
    agent.session.provider_session_id = Some("saved-upstream".into());
    agent.locked_tools = Some(Vec::new());
    // Establish a real saved profile before testing off. Otherwise removing the
    // only explicit state lets the empty-session save guard return without I/O.
    agent.session.save_prepared().unwrap();
    let path = crate::session::session_path(&agent.session.id).unwrap();
    assert!(path.is_file());
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir_all(&path).unwrap();
    // Fail both journal append and its snapshot fallback deterministically.
    let journal = crate::session::session_journal_path(&agent.session.id).unwrap();
    if journal.is_file() {
        std::fs::remove_file(&journal).unwrap();
    }
    std::fs::create_dir_all(&journal).unwrap();
    assert!(agent.set_agent_profile(None).is_err());
    assert_eq!(agent.session.agent_profile, Some(profile()));
    assert_eq!(
        agent.session.provider_session_id.as_deref(),
        Some("saved-upstream")
    );
    assert_eq!(agent.provider_session_id.as_deref(), Some("live-upstream"));
    assert!(!agent.provider_conversation_reset_pending);
    assert!(!agent.preserve_compaction_on_provider_reset);
    assert!(agent.locked_tools.is_some());
    assert!(provider.lifecycle.lock().unwrap().is_empty());
}

#[tokio::test]
async fn profile_legacy_constructor_and_restore_never_reuse_stale_upstream_prompts() {
    let _home = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    for streaming in [false, true] {
        let provider = Arc::new(PromptCaptureProvider::default());
        let mut session = crate::session::Session::create(None, None);
        session.agent_profile = Some(
            serde_json::from_value(serde_json::json!({
                "name":"legacy", "instructions":"LEGACY_PROFILE_BODY_45391"
            }))
            .unwrap(),
        );
        session.provider_session_id = Some("legacy-upstream-with-old-md".into());
        session.save().unwrap();
        let session_id = session.id.clone();
        let mut agent = Agent::new_with_session(provider.clone(), Registry::empty(), session, None);
        agent.memory_enabled = false;
        assert!(agent.provider_conversation_reset_pending);
        assert!(agent.session.provider_session_id.is_none());
        turn(&mut agent, streaming).await;
        assert_eq!(
            *provider.lifecycle.lock().unwrap(),
            vec!["reset", "request"]
        );
        agent
            .restore_session_with_working_dir(&session_id, None)
            .unwrap();
        assert!(agent.provider_conversation_reset_pending);
        assert!(agent.provider_session_id.is_none());
        turn(&mut agent, streaming).await;
        assert_eq!(
            *provider.lifecycle.lock().unwrap(),
            vec!["reset", "request", "reset", "request"]
        );
    }
}

#[test]
fn profile_restore_to_ordinary_session_keeps_pending_reset_and_clears_target_hint() {
    let _home = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    for pending_off in [false, true] {
        let provider = Arc::new(PromptCaptureProvider::default());
        let mut target = crate::session::Session::create(None, None);
        target.provider_session_id = Some("ordinary-target-upstream".into());
        // An upstream hint alone does not materialize an empty ordinary session.
        target.save_prepared().unwrap();
        assert!(crate::session::session_path(&target.id).unwrap().is_file());
        let mut agent = Agent::new(provider, Registry::empty());
        if pending_off {
            agent.provider_conversation_reset_pending = true;
        } else {
            agent.session.agent_profile = Some(profile());
        }
        agent
            .restore_session_with_working_dir(&target.id, None)
            .unwrap();
        assert!(agent.provider_conversation_reset_pending);
        assert!(agent.provider_session_id.is_none());
        assert!(agent.session.provider_session_id.is_none());
    }
}

#[test]
fn profile_native_provider_selected_replacements_use_project_precedence() {
    let _home = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let provider = Arc::new(PromptCaptureProvider {
        own_system_prompt: true,
        ..Default::default()
    });
    let mut agent = Agent::new(provider, Registry::empty());
    let mut selected = profile();
    selected.instruction_sources = ["PROJECT_SELECTED_BASE", "GLOBAL_SHADOWED_BASE"]
        .into_iter()
        .enumerate()
        .map(|(priority, content)| InstructionSourceSnapshot {
            id: format!("file:/base-{priority}"),
            kind: InstructionSourceKind::SystemPrompt,
            label: "Base prompt".into(),
            path: None,
            content: content.into(),
            priority: priority as u8,
        })
        .collect();
    agent.session.agent_profile = Some(selected);
    let prompt = agent.build_system_prompt_split(None);
    assert!(prompt.static_part.contains("PROJECT_SELECTED_BASE"));
    assert!(!prompt.static_part.contains("GLOBAL_SHADOWED_BASE"));
    assert!(prompt.static_part.contains("CHOSEN_PROFILE_89341"));
}

#[tokio::test]
async fn profile_instruction_reset_preserves_compacted_view_on_mutation_and_restore() {
    let _home = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let provider = Arc::new(PromptCaptureProvider::default());
    let mut session = crate::session::Session::create(None, None);
    session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "OLDER_RAW_TRANSCRIPT_92713".into(),
            cache_control: None,
        }],
    );
    session.agent_profile = Some(profile());
    session.provider_session_id = Some("old-compacted-native-prompt".into());
    session.compaction = Some(crate::session::StoredCompactionState {
        summary_text: "ACTIVE_PROFILE_SUMMARY_92713".into(),
        openai_encrypted_content: None,
        covers_up_to_turn: 1,
        original_turn_count: 1,
        compacted_count: 1,
    });
    session.save().unwrap();
    let session_id = session.id.clone();
    let mut agent = Agent::new_with_session(provider.clone(), Registry::empty(), session, None);
    for restore in [false, true] {
        if restore {
            agent
                .restore_session_with_working_dir(&session_id, None)
                .unwrap();
        }
        let manager = agent.registry.compaction();
        let before = serde_json::to_value(manager.read().await.persisted_state()).unwrap();
        assert!(before.to_string().contains("ACTIVE_PROFILE_SUMMARY_92713"));
        assert!(agent.provider_conversation_reset_pending);
        assert!(agent.preserve_compaction_on_provider_reset);
        agent.ensure_provider_conversation_reset().await.unwrap();
        assert_eq!(
            serde_json::to_value(manager.read().await.persisted_state()).unwrap(),
            before
        );
        assert!(agent.provider_session_id.is_none());
        assert!(agent.session.provider_session_id.is_none());
        agent.set_agent_profile(Some(profile())).unwrap();
        assert!(agent.preserve_compaction_on_provider_reset);
        agent.ensure_provider_conversation_reset().await.unwrap();
        assert_eq!(
            serde_json::to_value(manager.read().await.persisted_state()).unwrap(),
            before
        );
    }
    assert_eq!(
        *provider.lifecycle.lock().unwrap(),
        vec!["reset", "reset", "reset", "reset"]
    );
}

#[tokio::test]
async fn profile_mutation_does_not_override_pending_history_reset_reason() {
    let _home = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let provider = Arc::new(PromptCaptureProvider::default());
    let mut agent = Agent::new(provider, Registry::empty());
    agent.provider_conversation_reset_pending = true;
    agent.preserve_compaction_on_provider_reset = false;
    agent.set_agent_profile(Some(profile())).unwrap();
    assert!(!agent.preserve_compaction_on_provider_reset);
    agent.ensure_provider_conversation_reset().await.unwrap();
    assert!(!agent.provider_conversation_reset_pending);
    assert!(!agent.preserve_compaction_on_provider_reset);
}
