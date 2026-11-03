use super::*;

#[tokio::test]
#[allow(
    clippy::await_holding_lock,
    reason = "test intentionally serializes process-wide JCODE_HOME/env state across async recovery assertions"
)]
async fn startup_recovery_resumes_interrupted_headless_sessions_after_reload() -> Result<()> {
    let _storage_guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new()?;
    let _env = configure_test_env(&temp);

    let provider = Arc::new(StreamingMockProvider::default());
    for _ in 0..2 {
        provider.queue_response(vec![
            StreamEvent::TextDelta("continued after reload".to_string()),
            StreamEvent::MessageEnd { stop_reason: None },
        ]);
    }

    let mut initiator = crate::session::Session::create(None, Some("initiator".to_string()));
    initiator.set_canary("self-dev");
    initiator.add_message(
        Role::User,
        vec![crate::message::ContentBlock::ToolResult {
            tool_use_id: "tool_reload".to_string(),
            content: "Reload initiated. Process restarting...".to_string(),
            is_error: Some(false),
        }],
    );
    initiator.save()?;

    ReloadContext {
        task_context: Some("Verify multi-session reload recovery".to_string()),
        version_before: "old-build".to_string(),
        version_after: "new-build".to_string(),
        session_id: initiator.id.clone(),
        timestamp: "2026-04-19T00:00:00Z".to_string(),
    }
    .save()?;

    let mut peer = crate::session::Session::create(None, Some("peer".to_string()));
    peer.add_message(
        Role::User,
        vec![crate::message::ContentBlock::ToolResult {
            tool_use_id: "tool_bash".to_string(),
            content: "[Tool 'bash' interrupted by server reload after 0.2s]".to_string(),
            is_error: Some(true),
        }],
    );
    peer.save()?;

    let swarm_id = "swarm-reload-recovery";
    persist_swarm_state_snapshot(
        swarm_id,
        None,
        None,
        &[
            persisted_headless_member(&initiator.id, swarm_id, "running", "selfdev reload"),
            persisted_headless_member(&peer.id, swarm_id, "running", "bash tool"),
        ],
    );

    let server = Server::new(provider.clone());
    {
        let members = server.swarm_state.members.read().await;
        assert_eq!(
            members
                .get(&initiator.id)
                .map(|member| member.status.as_str()),
            Some("crashed"),
            "persisted running headless sessions should load as crashed before recovery"
        );
        assert_eq!(
            members.get(&peer.id).map(|member| member.status.as_str()),
            Some("crashed")
        );
    }

    server.recover_headless_sessions_on_startup().await;

    timeout(Duration::from_secs(5), async {
        loop {
            let sessions = server.sessions.read().await;
            let Some(initiator_agent) = sessions.get(&initiator.id).cloned() else {
                drop(sessions);
                tokio::time::sleep(Duration::from_millis(25)).await;
                continue;
            };
            let Some(peer_agent) = sessions.get(&peer.id).cloned() else {
                drop(sessions);
                tokio::time::sleep(Duration::from_millis(25)).await;
                continue;
            };
            drop(sessions);

            let initiator_done = {
                let guard = initiator_agent.lock().await;
                guard.messages().iter().any(|message| {
                    message.role == Role::Assistant
                        && message.content_preview().contains("continued after reload")
                })
            };
            let peer_done = {
                let guard = peer_agent.lock().await;
                guard.messages().iter().any(|message| {
                    message.role == Role::Assistant
                        && message.content_preview().contains("continued after reload")
                })
            };
            let statuses_ready = {
                let members = server.swarm_state.members.read().await;
                members
                    .get(&initiator.id)
                    .map(|member| member.status.as_str())
                    == Some("ready")
                    && members.get(&peer.id).map(|member| member.status.as_str()) == Some("ready")
            };

            if initiator_done && peer_done && statuses_ready {
                break;
            }

            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("headless reload recovery should resume both sessions");

    assert!(
        ReloadContext::peek_for_session(&initiator.id)?.is_none(),
        "initiator reload context should be consumed by headless recovery"
    );

    Ok(())
}

#[tokio::test]
#[allow(
    clippy::await_holding_lock,
    reason = "test intentionally serializes process-wide JCODE_HOME/env state across async recovery assertions"
)]
async fn startup_recovery_honors_pending_intent_for_stopped_worker() -> Result<()> {
    let _storage_guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new()?;
    let _env = configure_test_env(&temp);
    let provider = Arc::new(StreamingMockProvider::default());
    provider.queue_response(vec![
        StreamEvent::TextDelta("recovered durable work".to_string()),
        StreamEvent::MessageEnd { stop_reason: None },
    ]);

    // Shutdown may finish an aborted turn before persisting swarm state,
    // leaving no interruption marker in the session and a ready snapshot.
    let mut worker = crate::session::Session::create(None, Some("worker".to_string()));
    worker.add_message(
        Role::Assistant,
        vec![crate::message::ContentBlock::Text {
            text: "Waiting for acceptance results".to_string(),
            cache_control: None,
        }],
    );
    worker.save()?;
    let mut idle = crate::session::Session::create(None, Some("idle".to_string()));
    idle.save()?;
    let mut headed = crate::session::Session::create(None, Some("headed".to_string()));
    headed.save()?;
    for id in [&worker.id, &headed.id, &idle.id] {
        super::super::reload_recovery::persist_intent(
            "reload-stopped-worker",
            id,
            super::super::reload_recovery::ReloadRecoveryRole::Headless,
            crate::tool::selfdev::ReloadRecoveryDirective {
                reconnect_notice: None,
                continuation_message: "Continue interrupted work".to_string(),
            },
            "work in flight before shutdown",
        )?;
    }
    assert!(
        super::super::reload_recovery::mark_delivered_if_matching_continuation(
            &idle.id,
            "Continue interrupted work",
            "test-completed-recovery",
        )?
    );
    let swarm_id = "swarm-stopped-recovery";
    let mut headed_member = persisted_headless_member(&headed.id, swarm_id, "ready", "attached");
    headed_member.is_headless = false;
    persist_swarm_state_snapshot(
        swarm_id,
        None,
        None,
        &[
            persisted_headless_member(&worker.id, swarm_id, "ready", "aborted turn"),
            persisted_headless_member(&idle.id, swarm_id, "ready", "genuinely idle"),
            headed_member,
        ],
    );
    let server = Server::new(provider);
    assert_eq!(
        server.swarm_state.members.read().await[&worker.id].status,
        "stopped"
    );
    server.recover_headless_sessions_on_startup().await;
    timeout(Duration::from_secs(5), async {
        loop {
            if !super::super::reload_recovery::has_pending_for_session(&worker.id)
                && server.swarm_state.members.read().await[&worker.id].status == "ready"
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("durable intent must resume even a stopped worker");
    let sessions = server.sessions.read().await;
    assert!(
        !sessions.contains_key(&idle.id),
        "idle workers must not be resurrected"
    );
    assert!(
        !sessions.contains_key(&headed.id),
        "headed recovery belongs to reconnect"
    );
    let agent = sessions[&worker.id].clone();
    drop(sessions);
    assert!(agent.lock().await.messages().iter().any(|message| {
        message.role == Role::Assistant
            && message.content_preview().contains("recovered durable work")
    }));
    assert!(super::super::reload_recovery::has_pending_for_session(
        &headed.id
    ));
    Ok(())
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn startup_recovery_preserves_headed_session_reload_context_for_later_reconnect() -> Result<()>
{
    let _storage_guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new()?;
    let _env = configure_test_env(&temp);

    let provider = Arc::new(StreamingMockProvider::default());
    provider.queue_response(vec![
        StreamEvent::TextDelta("continued after reload".to_string()),
        StreamEvent::MessageEnd { stop_reason: None },
    ]);

    let mut headless = crate::session::Session::create(None, Some("headless".to_string()));
    headless.add_message(
        Role::User,
        vec![crate::message::ContentBlock::ToolResult {
            tool_use_id: "tool_bash".to_string(),
            content: "[Tool 'bash' interrupted by server reload after 0.2s]".to_string(),
            is_error: Some(true),
        }],
    );
    headless.save()?;

    ReloadContext {
        task_context: Some("resume headless worker".to_string()),
        version_before: "old-headless".to_string(),
        version_after: "new-headless".to_string(),
        session_id: headless.id.clone(),
        timestamp: "2026-04-19T00:00:00Z".to_string(),
    }
    .save()?;

    let headed_session_id = crate::id::new_id("headed-reconnect");
    ReloadContext {
        task_context: Some("resume headed reconnecting session".to_string()),
        version_before: "old-headed".to_string(),
        version_after: "new-headed".to_string(),
        session_id: headed_session_id.clone(),
        timestamp: "2026-04-19T00:00:01Z".to_string(),
    }
    .save()?;

    let swarm_id = "swarm-reload-headed-mixed";
    persist_swarm_state_snapshot(
        swarm_id,
        None,
        None,
        &[persisted_headless_member(
            &headless.id,
            swarm_id,
            "running",
            "bash tool",
        )],
    );

    let server = Server::new(provider.clone());
    server.recover_headless_sessions_on_startup().await;

    timeout(Duration::from_secs(5), async {
        loop {
            let sessions = server.sessions.read().await;
            let Some(headless_agent) = sessions.get(&headless.id).cloned() else {
                drop(sessions);
                tokio::time::sleep(Duration::from_millis(25)).await;
                continue;
            };
            drop(sessions);

            let headless_done = {
                let guard = headless_agent.lock().await;
                guard.messages().iter().any(|message| {
                    message.role == Role::Assistant
                        && message.content_preview().contains("continued after reload")
                })
            };
            if headless_done {
                break;
            }

            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("headless reload recovery should complete");

    assert!(
        ReloadContext::peek_for_session(&headless.id)?.is_none(),
        "headless session reload context should be consumed by startup recovery"
    );
    assert!(
        ReloadContext::peek_for_session(&headed_session_id)?.is_some(),
        "headed reconnecting session reload context should remain available for later reconnect"
    );

    Ok(())
}

#[cfg(unix)]
#[tokio::test]
#[allow(
    clippy::await_holding_lock,
    reason = "test intentionally serializes process-wide JCODE_HOME/env state across async startup assertions"
)]
async fn startup_ready_signal_is_not_blocked_by_headless_recovery_delay() -> Result<()> {
    use std::os::unix::io::FromRawFd;
    use tokio::io::AsyncReadExt;

    let _storage_guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new()?;
    let _env = configure_test_env(&temp);
    let _delay_guard = ScopedEnvVar::set("JCODE_TEST_HEADLESS_STARTUP_RECOVERY_DELAY_MS", "500");

    let mut headless =
        crate::session::Session::create(None, Some("headless-ready-delay".to_string()));
    headless.save()?;

    let swarm_id = "swarm-ready-before-recovery";
    persist_swarm_state_snapshot(
        swarm_id,
        None,
        None,
        &[persisted_headless_member(
            &headless.id,
            swarm_id,
            "running",
            "delay startup recovery",
        )],
    );

    let provider = Arc::new(StreamingMockProvider::default());
    provider.queue_response(vec![StreamEvent::MessageEnd { stop_reason: None }]);
    let server = Server::new(provider);

    let mut ready_fds = [0; 2];
    let pipe_rc = unsafe { libc::pipe(ready_fds.as_mut_ptr()) };
    assert_eq!(pipe_rc, 0, "pipe() should succeed");
    let read_fd = ready_fds[0];
    let write_fd = ready_fds[1];
    let _ready_fd_guard = ScopedEnvVar::set("JCODE_READY_FD", write_fd.to_string());

    let main_listener = crate::transport::Listener::bind(&server.socket_path)?;
    let debug_listener = crate::transport::Listener::bind(&server.debug_socket_path)?;

    let startup = tokio::spawn(async move {
        server
            .finish_startup_after_bind(main_listener, debug_listener, Instant::now())
            .await
    });

    let read_file = unsafe { std::fs::File::from_raw_fd(read_fd) };
    let mut async_read = tokio::fs::File::from_std(read_file);
    let mut ready = [0u8; 1];
    timeout(Duration::from_millis(200), async {
        async_read.read_exact(&mut ready).await
    })
    .await
    .expect("ready signal should arrive before delayed startup recovery completes")?;
    assert_eq!(ready, [b'R']);
    assert!(
        !startup.is_finished(),
        "startup task should still be blocked on delayed recovery even though ready was already signaled"
    );

    let (runtime, main_handle, debug_handle) = timeout(Duration::from_secs(2), startup)
        .await
        .expect("startup should finish after delayed recovery")
        .expect("startup task should succeed");
    timeout(Duration::from_secs(1), runtime.shutdown())
        .await
        .expect("runtime should shut down");
    timeout(Duration::from_secs(1), async {
        main_handle.await.expect("main accept loop");
        debug_handle.await.expect("debug accept loop");
    })
    .await
    .expect("accept loops should observe runtime cancellation");

    Ok(())
}
