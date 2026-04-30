use super::*;
use anyhow::{Result, anyhow};

#[test]
fn detects_reload_interrupted_generation_text() {
    let agent = test_agent(vec![crate::session::StoredMessage {
        id: "msg_1".to_string(),
        role: crate::message::Role::Assistant,
        content: vec![ContentBlock::Text {
            text: "partial\n\n[generation interrupted - server reloading]".to_string(),
            cache_control: None,
        }],
        display_role: None,
        timestamp: None,
        tool_duration_ms: None,
        token_usage: None,
    }]);

    assert!(session_was_interrupted_by_reload(&agent));
}

#[test]
fn detects_reload_interrupted_tool_result() {
    let agent = test_agent(vec![crate::session::StoredMessage {
        id: "msg_2".to_string(),
        role: crate::message::Role::User,
        content: vec![ContentBlock::ToolResult {
            tool_use_id: "tool_1".to_string(),
            content: "[Tool 'bash' interrupted by server reload after 0.2s]".to_string(),
            is_error: Some(true),
        }],
        display_role: None,
        timestamp: None,
        tool_duration_ms: None,
        token_usage: None,
    }]);

    assert!(session_was_interrupted_by_reload(&agent));
}

#[test]
fn detects_reload_skipped_tool_result() {
    let agent = test_agent(vec![crate::session::StoredMessage {
        id: "msg_3".to_string(),
        role: crate::message::Role::User,
        content: vec![ContentBlock::ToolResult {
            tool_use_id: "tool_2".to_string(),
            content: "[Skipped - server reloading]".to_string(),
            is_error: Some(true),
        }],
        display_role: None,
        timestamp: None,
        tool_duration_ms: None,
        token_usage: None,
    }]);

    assert!(session_was_interrupted_by_reload(&agent));
}

#[test]
fn detects_selfdev_reload_tool_result_even_when_not_marked_error() {
    let agent = test_agent(vec![crate::session::StoredMessage {
        id: "msg_3b".to_string(),
        role: crate::message::Role::User,
        content: vec![ContentBlock::ToolResult {
            tool_use_id: "tool_2b".to_string(),
            content: "Reload initiated. Process restarting...".to_string(),
            is_error: Some(false),
        }],
        display_role: None,
        timestamp: None,
        tool_duration_ms: None,
        token_usage: None,
    }]);

    assert!(session_was_interrupted_by_reload(&agent));
}

#[test]
fn ignores_normal_tool_errors() {
    let agent = test_agent(vec![crate::session::StoredMessage {
        id: "msg_4".to_string(),
        role: crate::message::Role::User,
        content: vec![ContentBlock::ToolResult {
            tool_use_id: "tool_3".to_string(),
            content: "Error: file not found".to_string(),
            is_error: Some(true),
        }],
        display_role: None,
        timestamp: None,
        tool_duration_ms: None,
        token_usage: None,
    }]);

    assert!(!session_was_interrupted_by_reload(&agent));
}

#[test]
fn restored_closed_session_with_reload_marker_still_counts_as_interrupted() {
    let agent = test_agent(vec![crate::session::StoredMessage {
        id: "msg_5".to_string(),
        role: crate::message::Role::Assistant,
        content: vec![ContentBlock::Text {
            text: "partial\n\n[generation interrupted - server reloading]".to_string(),
            cache_control: None,
        }],
        display_role: None,
        timestamp: None,
        tool_duration_ms: None,
        token_usage: None,
    }]);

    assert!(restored_session_was_interrupted(
        "session_test_reload",
        &crate::session::SessionStatus::Closed,
        &agent,
    ));
}

#[test]
fn restored_closed_session_with_pending_user_message_during_reload_should_count_as_interrupted()
-> Result<()> {
    let _guard = crate::storage::lock_test_env();
    let runtime = tempfile::TempDir::new().map_err(|e| anyhow!(e))?;
    let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
    crate::env::set_var("JCODE_RUNTIME_DIR", runtime.path());
    crate::server::write_reload_state(
        "reload-pending-user",
        "test-hash",
        crate::server::ReloadPhase::Starting,
        Some("session_test_reload".to_string()),
    );

    let agent = test_agent(vec![crate::session::StoredMessage {
        id: "msg_pending_reload".to_string(),
        role: crate::message::Role::User,
        content: vec![ContentBlock::Text {
            text: "continue this after reload".to_string(),
            cache_control: None,
        }],
        display_role: None,
        timestamp: None,
        tool_duration_ms: None,
        token_usage: None,
    }]);

    let interrupted = restored_session_was_interrupted(
        "session_test_reload",
        &crate::session::SessionStatus::Closed,
        &agent,
    );

    crate::server::clear_reload_marker();
    if let Some(prev_runtime) = prev_runtime {
        crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
    } else {
        crate::env::remove_var("JCODE_RUNTIME_DIR");
    }

    assert!(
        interrupted,
        "a session closed by reload cleanup while processing should be auto-resumed"
    );
    Ok(())
}

#[test]
fn restored_closed_session_with_pending_user_message_during_socket_ready_handoff_counts_as_interrupted()
-> Result<()> {
    let _guard = crate::storage::lock_test_env();
    let runtime = tempfile::TempDir::new().map_err(|e| anyhow!(e))?;
    let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
    crate::env::set_var("JCODE_RUNTIME_DIR", runtime.path());
    crate::server::write_reload_state(
        "reload-pending-user-ready",
        "test-hash",
        crate::server::ReloadPhase::SocketReady,
        Some("session_test_reload".to_string()),
    );

    let agent = test_agent(vec![crate::session::StoredMessage {
        id: "msg_pending_reload_ready".to_string(),
        role: crate::message::Role::User,
        content: vec![ContentBlock::Text {
            text: "continue this after socket-ready handoff".to_string(),
            cache_control: None,
        }],
        display_role: None,
        timestamp: None,
        tool_duration_ms: None,
        token_usage: None,
    }]);

    let interrupted = restored_session_was_interrupted(
        "session_test_reload",
        &crate::session::SessionStatus::Closed,
        &agent,
    );

    crate::server::clear_reload_marker();
    if let Some(prev_runtime) = prev_runtime {
        crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
    } else {
        crate::env::remove_var("JCODE_RUNTIME_DIR");
    }

    assert!(
        interrupted,
        "a pending user turn closed during socket-ready handoff should still be auto-resumed"
    );
    Ok(())
}

#[test]
fn restored_closed_session_with_pending_user_message_without_reload_marker_is_not_interrupted() {
    let _guard = crate::storage::lock_test_env();
    let runtime = tempfile::TempDir::new().expect("runtime dir");
    let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
    crate::env::set_var("JCODE_RUNTIME_DIR", runtime.path());
    crate::server::clear_reload_marker();

    let agent = test_agent(vec![crate::session::StoredMessage {
        id: "msg_pending_normal_close".to_string(),
        role: crate::message::Role::User,
        content: vec![ContentBlock::Text {
            text: "normal pending user text".to_string(),
            cache_control: None,
        }],
        display_role: None,
        timestamp: None,
        tool_duration_ms: None,
        token_usage: None,
    }]);

    let interrupted = restored_session_was_interrupted(
        "session_test_reload",
        &crate::session::SessionStatus::Closed,
        &agent,
    );

    if let Some(prev_runtime) = prev_runtime {
        crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
    } else {
        crate::env::remove_var("JCODE_RUNTIME_DIR");
    }

    assert!(!interrupted);
}

#[test]
fn restored_closed_session_without_reload_marker_is_not_interrupted() {
    let agent = test_agent(vec![crate::session::StoredMessage {
        id: "msg_6".to_string(),
        role: crate::message::Role::Assistant,
        content: vec![ContentBlock::Text {
            text: "finished normally".to_string(),
            cache_control: None,
        }],
        display_role: None,
        timestamp: None,
        tool_duration_ms: None,
        token_usage: None,
    }]);

    assert!(!restored_session_was_interrupted(
        "session_test_reload",
        &crate::session::SessionStatus::Closed,
        &agent,
    ));
}

#[test]
fn mark_remote_reload_started_writes_starting_marker() -> Result<()> {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().map_err(|e| anyhow!(e))?;
    let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
    crate::env::set_var("JCODE_RUNTIME_DIR", temp.path());

    mark_remote_reload_started("reload-test");

    let state = crate::server::recent_reload_state(std::time::Duration::from_secs(5))
        .ok_or_else(|| anyhow!("reload state should exist"))?;
    assert_eq!(state.request_id, "reload-test");
    assert_eq!(state.phase, crate::server::ReloadPhase::Starting);

    crate::server::clear_reload_marker();
    if let Some(prev_runtime) = prev_runtime {
        crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
    } else {
        crate::env::remove_var("JCODE_RUNTIME_DIR");
    }
    Ok(())
}

#[test]
fn handle_reload_queues_signal_for_canary_session() -> Result<()> {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().map_err(|e| anyhow!(e))?;
    let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
    crate::env::set_var("JCODE_RUNTIME_DIR", temp.path());

    let rt = tokio::runtime::Runtime::new().map_err(|e| anyhow!(e))?;
    rt.block_on(async {
        let mut rx = crate::server::subscribe_reload_signal_for_tests();
        let provider: Arc<dyn Provider> = Arc::new(MockProvider);
        let registry = Registry::new(provider.clone()).await;
        let mut agent = build_test_agent(provider, registry, Vec::new());
        agent.set_canary("self-dev");
        let agent = Arc::new(Mutex::new(agent));
        let (tx, mut events) = mpsc::unbounded_channel::<ServerEvent>();
        let (peer_tx, mut peer_events) = mpsc::unbounded_channel::<ServerEvent>();
        let now = Instant::now();
        let swarm_members = Arc::new(RwLock::new(HashMap::from([
            (
                "session_test_reload".to_string(),
                SwarmMember {
                    session_id: "session_test_reload".to_string(),
                    event_tx: tx.clone(),
                    event_txs: HashMap::from([("conn-trigger".to_string(), tx.clone())]),
                    working_dir: None,
                    swarm_id: None,
                    swarm_enabled: false,
                    status: "ready".to_string(),
                    detail: None,
                    friendly_name: Some("trigger".to_string()),
                    report_back_to_session_id: None,
                    latest_completion_report: None,
                    role: "agent".to_string(),
                    joined_at: now,
                    last_status_change: now,
                    is_headless: false,
                },
            ),
            (
                "session_peer".to_string(),
                SwarmMember {
                    session_id: "session_peer".to_string(),
                    event_tx: peer_tx.clone(),
                    event_txs: HashMap::from([("conn-peer".to_string(), peer_tx.clone())]),
                    working_dir: None,
                    swarm_id: None,
                    swarm_enabled: false,
                    status: "ready".to_string(),
                    detail: None,
                    friendly_name: Some("peer".to_string()),
                    report_back_to_session_id: None,
                    latest_completion_report: None,
                    role: "agent".to_string(),
                    joined_at: now,
                    last_status_change: now,
                    is_headless: false,
                },
            ),
        ])));

        handle_reload(7, &agent, &swarm_members, &tx).await;

        let reloading = events
            .recv()
            .await
            .ok_or_else(|| anyhow!("reloading event"))?;
        assert!(matches!(reloading, ServerEvent::Reloading { .. }));
        let peer_reloading = peer_events
            .recv()
            .await
            .ok_or_else(|| anyhow!("peer reloading event"))?;
        assert!(matches!(peer_reloading, ServerEvent::Reloading { .. }));
        let done = events.recv().await.ok_or_else(|| anyhow!("done event"))?;
        assert!(matches!(done, ServerEvent::Done { id: 7 }));

        tokio::time::timeout(std::time::Duration::from_secs(1), rx.changed())
            .await
            .map_err(|_| anyhow!("reload signal timeout"))?
            .map_err(|e| anyhow!("reload signal should be delivered: {e}"))?;
        let signal = rx
            .borrow_and_update()
            .clone()
            .ok_or_else(|| anyhow!("reload signal payload should exist"))?;
        assert_eq!(
            signal.triggering_session.as_deref(),
            Some("session_test_reload")
        );
        assert!(signal.prefer_selfdev_binary);
        assert_eq!(signal.hash, env!("JCODE_GIT_HASH"));

        let state = crate::server::recent_reload_state(std::time::Duration::from_secs(5))
            .ok_or_else(|| anyhow!("reload state should exist"))?;
        assert_eq!(state.phase, crate::server::ReloadPhase::Starting);
        Ok::<_, anyhow::Error>(())
    })?;

    crate::server::clear_reload_marker();
    if let Some(prev_runtime) = prev_runtime {
        crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
    } else {
        crate::env::remove_var("JCODE_RUNTIME_DIR");
    }
    Ok(())
}

#[tokio::test]
async fn rename_shutdown_signal_moves_registration_to_restored_session() -> Result<()> {
    let signal = InterruptSignal::new();
    let shutdown_signals = Arc::new(RwLock::new(HashMap::from([(
        "session_old".to_string(),
        signal.clone(),
    )])));

    rename_shutdown_signal(&shutdown_signals, "session_old", "session_restored").await;

    let signals = shutdown_signals.read().await;
    assert!(!signals.contains_key("session_old"));
    let renamed = signals
        .get("session_restored")
        .ok_or_else(|| anyhow!("restored session should retain shutdown signal"))?;
    renamed.fire();
    assert!(signal.is_set());
    Ok(())
}
