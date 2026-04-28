use crate::test_support::*;

/// Test that multi-turn conversation works with session resume
#[tokio::test]
async fn test_multi_turn_conversation() -> Result<()> {
    let _env = setup_test_env()?;
    let provider = MockProvider::new();

    // First turn response
    provider.queue_response(vec![
        StreamEvent::TextDelta("I'll remember that.".to_string()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        },
        StreamEvent::SessionId("session-abc".to_string()),
    ]);

    // Second turn response
    provider.queue_response(vec![
        StreamEvent::TextDelta("You said hello earlier.".to_string()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        },
        StreamEvent::SessionId("session-abc".to_string()),
    ]);

    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let registry = Registry::new(provider.clone()).await;
    let mut agent = Agent::new(provider, registry);

    // First turn
    let response1 = agent.run_once_capture("Hello").await?;
    assert_eq!(response1, "I'll remember that.");

    // Second turn - should use session resume
    let response2 = agent.run_once_capture("What did I say?").await?;
    assert_eq!(response2, "You said hello earlier.");

    Ok(())
}

/// Test that token usage is tracked
#[tokio::test]
async fn test_token_usage() -> Result<()> {
    let _env = setup_test_env()?;
    let provider = MockProvider::new();

    provider.queue_response(vec![
        StreamEvent::TokenUsage {
            input_tokens: Some(10),
            output_tokens: Some(20),
            cache_read_input_tokens: None,
            cache_creation_input_tokens: None,
        },
        StreamEvent::TextDelta("Response".to_string()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        },
        StreamEvent::SessionId("session-123".to_string()),
    ]);

    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let registry = Registry::new(provider.clone()).await;
    let mut agent = Agent::new(provider, registry);

    let response = agent.run_once_capture("Test").await?;
    assert_eq!(response, "Response");

    Ok(())
}

/// Test error handling
#[tokio::test]
async fn test_stream_error() -> Result<()> {
    let _env = setup_test_env()?;
    let provider = MockProvider::new();

    provider.queue_response(vec![
        StreamEvent::TextDelta("Starting...".to_string()),
        StreamEvent::Error {
            message: "Something went wrong".to_string(),
            retry_after_secs: None,
        },
    ]);

    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let registry = Registry::new(provider.clone()).await;
    let mut agent = Agent::new(provider, registry);

    let result = agent.run_once_capture("Test").await;
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("Something went wrong")
    );

    Ok(())
}

/// Test model cycling over the socket interface (server + client)
#[tokio::test]
async fn test_socket_model_cycle_supported_models() -> Result<()> {
    let _env = setup_test_env()?;
    let runtime_dir = short_runtime_dir(format!(
        "jcode-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&runtime_dir)?;
    let socket_path = runtime_dir.join("jcode.sock");
    let debug_socket_path = runtime_dir.join("jcode-debug.sock");

    let provider = MockProvider::with_models(vec!["gpt-5.2-codex", "claude-opus-4-5-20251101"]);
    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let server_instance =
        server::Server::new_with_paths(provider, socket_path.clone(), debug_socket_path.clone());

    let server_handle = tokio::spawn(async move { server_instance.run().await });

    let mut client = wait_for_server_client(&socket_path).await?;
    let request_id = client.cycle_model(1).await?;

    let mut saw_model_changed = false;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let event = tokio::time::timeout(Duration::from_secs(1), client.read_event()).await??;
        match event {
            ServerEvent::Ack { .. } => continue,
            ServerEvent::ModelChanged {
                id, model, error, ..
            } if id == request_id => {
                assert!(error.is_none(), "Expected successful model change");
                assert_eq!(model, "claude-opus-4-5-20251101");
                saw_model_changed = true;
                break;
            }
            _ => {}
        }
    }

    abort_server_and_cleanup(&server_handle, &socket_path, &debug_socket_path);

    assert!(saw_model_changed, "Did not receive model_changed event");
    Ok(())
}

/// Test that resume restores model selection and tool output in history
#[tokio::test]
async fn test_resume_restores_model_and_tool_history() -> Result<()> {
    let _env = setup_test_env()?;
    let runtime_dir = short_runtime_dir(format!(
        "jcode-resume-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&runtime_dir)?;

    let mut session = Session::create(None, Some("Resume Test".to_string()));
    session.model = Some("gpt-5.2-codex".to_string());
    session.add_message(
        jcode::message::Role::User,
        vec![jcode::message::ContentBlock::Text {
            text: "Run a tool".to_string(),
            cache_control: None,
        }],
    );
    session.add_message(
        jcode::message::Role::Assistant,
        vec![
            jcode::message::ContentBlock::Text {
                text: "Running...".to_string(),
                cache_control: None,
            },
            jcode::message::ContentBlock::ToolUse {
                id: "tool-1".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({"cmd": "echo hi"}),
            },
        ],
    );
    session.add_message(
        jcode::message::Role::User,
        vec![jcode::message::ContentBlock::ToolResult {
            tool_use_id: "tool-1".to_string(),
            content: "hi\n".to_string(),
            is_error: None,
        }],
    );
    session.save()?;

    let socket_path = runtime_dir.join("jcode.sock");
    let debug_socket_path = runtime_dir.join("jcode-debug.sock");

    // Default model = claude, resume should switch to gpt-5.2-codex
    let provider = MockProvider::with_models(vec!["claude-opus-4-5-20251101", "gpt-5.2-codex"]);
    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let server_instance =
        server::Server::new_with_paths(provider, socket_path.clone(), debug_socket_path.clone());
    let server_handle = tokio::spawn(async move { server_instance.run().await });

    let mut client = wait_for_server_client(&socket_path).await?;
    let resume_id = client.resume_session(&session.id).await?;

    let mut history_event = None;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let event = tokio::time::timeout(Duration::from_secs(1), client.read_event()).await??;
        match event {
            ServerEvent::History {
                id,
                messages,
                provider_model,
                ..
            } if id == resume_id => {
                history_event = Some((messages, provider_model));
                break;
            }
            _ => {}
        }
    }

    abort_server_and_cleanup(&server_handle, &socket_path, &debug_socket_path);

    let (messages, provider_model) =
        history_event.ok_or_else(|| anyhow::anyhow!("Did not receive history event"))?;

    assert_eq!(provider_model, Some("gpt-5.2-codex".to_string()));

    let tool_msg = messages
        .iter()
        .find(|m| m.role == "tool")
        .ok_or_else(|| anyhow::anyhow!("Tool message missing in history"))?;
    assert!(tool_msg.content.contains("hi"));
    let tool_data = tool_msg
        .tool_data
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Tool metadata missing in history"))?;
    assert_eq!(tool_data.name, "bash");

    Ok(())
}

/// Test that subscribe selfdev hint marks the session as canary
#[tokio::test]
async fn test_resume_session_with_local_history_uses_metadata_only_history() -> Result<()> {
    let _env = setup_test_env()?;
    let runtime_dir = short_runtime_dir(format!(
        "jcode-target-subscribe-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&runtime_dir)?;

    let mut session = Session::create(None, Some("Target Subscribe Test".to_string()));
    session.model = Some("model-a".to_string());
    session.provider_session_id = Some("provider-resume-123".to_string());
    session.add_message(
        jcode::message::Role::User,
        vec![jcode::message::ContentBlock::Text {
            text: "Existing local history".to_string(),
            cache_control: None,
        }],
    );
    session.add_message(
        jcode::message::Role::Assistant,
        vec![jcode::message::ContentBlock::Text {
            text: "Existing assistant response".to_string(),
            cache_control: None,
        }],
    );
    session.save()?;

    let socket_path = runtime_dir.join("jcode.sock");
    let debug_socket_path = runtime_dir.join("jcode-debug.sock");

    let provider = Arc::new(MockProvider::with_models(vec!["model-a"]));
    provider.queue_response(vec![
        StreamEvent::TextDelta("resumed ok".to_string()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        },
    ]);

    let provider_dyn: Arc<dyn jcode::provider::Provider> = provider.clone();
    let server_instance = server::Server::new_with_paths(
        provider_dyn,
        socket_path.clone(),
        debug_socket_path.clone(),
    );
    let server_handle = tokio::spawn(async move { server_instance.run().await });

    let mut client = wait_for_server_client(&socket_path).await?;
    let subscribe_id = client.subscribe().await?;

    let mut saw_done = false;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let event = tokio::time::timeout(Duration::from_secs(1), client.read_event()).await??;
        match event {
            ServerEvent::Done { id } if id == subscribe_id => {
                saw_done = true;
                break;
            }
            _ => {}
        }
    }

    assert!(saw_done, "Did not receive subscribe done event");

    let resume_id = client
        .resume_session_with_options(&session.id, true, false)
        .await?;
    let mut history_checked = false;
    let mut resume_done = false;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && !(history_checked && resume_done) {
        let event = tokio::time::timeout(Duration::from_secs(1), client.read_event()).await??;
        match event {
            ServerEvent::Ack { .. } => continue,
            ServerEvent::History {
                id,
                session_id,
                messages,
                provider_model,
                ..
            } if id == resume_id => {
                assert_eq!(session_id, session.id);
                assert_eq!(
                    messages.len(),
                    2,
                    "resume history should include persisted local messages"
                );
                assert_eq!(messages[0].role, "user");
                assert!(messages[0].content.contains("Existing local history"));
                assert_eq!(messages[1].role, "assistant");
                assert!(
                    messages[1].content.contains("Existing assistant response"),
                    "resume history should include persisted assistant response"
                );
                assert_eq!(provider_model, Some("model-a".to_string()));
                history_checked = true;
            }
            ServerEvent::Done { id } if id == resume_id => {
                resume_done = true;
            }
            ServerEvent::Error { id, message, .. } if id == resume_id => {
                anyhow::bail!("resume_session failed: {}", message);
            }
            _ => {}
        }
    }

    assert!(history_checked, "Did not receive resume history event");
    assert!(resume_done, "Did not receive resume done event");

    let msg_id = client.send_message("continue resumed session").await?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut saw_message_done = false;
    let mut seen_events = Vec::new();
    while Instant::now() < deadline {
        let event = tokio::time::timeout(Duration::from_secs(1), client.read_event()).await??;
        seen_events.push(format!("{event:?}"));
        if matches!(event, ServerEvent::Done { id } if id == msg_id) {
            saw_message_done = true;
            break;
        }
    }
    assert!(
        saw_message_done,
        "Did not receive Done for resumed message. Seen events: {}\nstate={}\nhistory={}\nlogs={}",
        seen_events.join(" | "),
        debug_run_command(debug_socket_path.clone(), "state", Some(&session.id))
            .await
            .unwrap_or_else(|err| format!("<state error: {err}>")),
        debug_run_command(debug_socket_path.clone(), "history", Some(&session.id))
            .await
            .unwrap_or_else(|err| format!("<history error: {err}>")),
        std::env::var_os("JCODE_HOME")
            .and_then(|home| latest_log_excerpt(std::path::Path::new(&home)))
            .unwrap_or_else(|| "<no logs>".to_string())
    );

    let resume_ids = provider.captured_resume_session_ids.lock().unwrap().clone();
    assert_eq!(
        resume_ids.last().cloned(),
        Some(Some("provider-resume-123".to_string()))
    );

    abort_server_and_cleanup(&server_handle, &socket_path, &debug_socket_path);

    Ok(())
}

#[tokio::test]
async fn test_resume_session_reports_reload_interruption_for_peer_sessions() -> Result<()> {
    let _env = setup_test_env()?;
    let runtime_dir = short_runtime_dir(format!(
        "jcode-reload-interruption-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&runtime_dir)?;

    let mut session = Session::create(None, Some("Reload Interrupted Session".to_string()));
    session.model = Some("model-a".to_string());
    session.add_message(
        jcode::message::Role::User,
        vec![jcode::message::ContentBlock::ToolResult {
            tool_use_id: "tool_bash_1".to_string(),
            content: "[Tool 'bash' interrupted by server reload after 0.2s]".to_string(),
            is_error: Some(true),
        }],
    );
    session.save()?;

    let socket_path = runtime_dir.join("jcode.sock");
    let debug_socket_path = runtime_dir.join("jcode-debug.sock");

    let provider = Arc::new(MockProvider::with_models(vec!["model-a"]));
    let provider_dyn: Arc<dyn jcode::provider::Provider> = provider.clone();
    let server_instance = server::Server::new_with_paths(
        provider_dyn,
        socket_path.clone(),
        debug_socket_path.clone(),
    );
    let server_handle = tokio::spawn(async move { server_instance.run().await });

    let mut client = wait_for_server_client(&socket_path).await?;
    let subscribe_id = client.subscribe().await?;
    let subscribe_events = collect_until_done_unix(&mut client, subscribe_id).await?;
    assert!(
        subscribe_events
            .iter()
            .any(|event| matches!(event, ServerEvent::Done { id } if *id == subscribe_id)),
        "expected subscribe to finish, got: {subscribe_events:?}"
    );

    let resume_id = client.resume_session(&session.id).await?;
    let events = collect_until_done_unix(&mut client, resume_id).await?;

    let history_event = events
        .iter()
        .find_map(|event| match event {
            ServerEvent::History {
                id,
                session_id,
                was_interrupted,
                ..
            } if *id == resume_id => Some((session_id.clone(), *was_interrupted)),
            _ => None,
        })
        .ok_or_else(|| anyhow::anyhow!("Did not receive resume history event: {events:?}"))?;

    assert_eq!(history_event.0, session.id);
    assert_eq!(
        history_event.1,
        Some(true),
        "reload-interrupted peer sessions should be marked interrupted so clients auto-continue"
    );

    abort_server_and_cleanup(&server_handle, &socket_path, &debug_socket_path);

    Ok(())
}

/// Test that subscribe selfdev hint marks the session as canary
#[tokio::test]
async fn test_subscribe_selfdev_hint_marks_canary() -> Result<()> {
    let _env = setup_test_env()?;
    let runtime_dir = short_runtime_dir(format!(
        "jcode-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&runtime_dir)?;
    let socket_path = runtime_dir.join("jcode.sock");
    let debug_socket_path = runtime_dir.join("jcode-debug.sock");

    let provider = MockProvider::new();
    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let server_instance =
        server::Server::new_with_paths(provider, socket_path.clone(), debug_socket_path.clone());

    let server_handle = tokio::spawn(async move { server_instance.run().await });

    let mut client = wait_for_server_client(&socket_path).await?;
    let subscribe_id = client
        .subscribe_with_info(None, Some(true), None, false, false)
        .await?;

    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let event = tokio::time::timeout(Duration::from_secs(1), client.read_event()).await??;
        if matches!(event, ServerEvent::Done { id } if id == subscribe_id) {
            break;
        }
    }

    let history_event = client.get_history_event().await?;
    match history_event {
        ServerEvent::History { is_canary, .. } => {
            assert_eq!(is_canary, Some(true));
        }
        _ => anyhow::bail!("Expected history event after subscribe"),
    }

    abort_server_and_cleanup(&server_handle, &socket_path, &debug_socket_path);

    Ok(())
}

/// Test that working_dir alone no longer upgrades a session to self-dev.
#[tokio::test]
async fn test_subscribe_working_dir_without_selfdev_hint_stays_normal() -> Result<()> {
    let _env = setup_test_env()?;
    let runtime_dir = short_runtime_dir(format!(
        "jcode-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&runtime_dir)?;
    let socket_path = runtime_dir.join("jcode.sock");
    let debug_socket_path = runtime_dir.join("jcode-debug.sock");

    let fake_repo = tempfile::tempdir()?;
    std::fs::create_dir_all(fake_repo.path().join(".git"))?;
    std::fs::write(
        fake_repo.path().join("Cargo.toml"),
        "[package]\nname = \"jcode\"\nversion = \"0.0.0\"\n",
    )?;
    let nested_dir = fake_repo.path().join("nested").join("worktree");
    std::fs::create_dir_all(&nested_dir)?;

    let provider = MockProvider::new();
    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let server_instance =
        server::Server::new_with_paths(provider, socket_path.clone(), debug_socket_path.clone());

    let server_handle = tokio::spawn(async move { server_instance.run().await });

    let mut client = wait_for_server_client(&socket_path).await?;
    let subscribe_id = client
        .subscribe_with_info(
            Some(nested_dir.display().to_string()),
            None,
            None,
            false,
            false,
        )
        .await?;

    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let event = tokio::time::timeout(Duration::from_secs(1), client.read_event()).await??;
        if matches!(event, ServerEvent::Done { id } if id == subscribe_id) {
            break;
        }
    }

    let history_event = client.get_history_event().await?;
    match history_event {
        ServerEvent::History { is_canary, .. } => {
            assert_eq!(is_canary, Some(false));
        }
        _ => anyhow::bail!("Expected history event after subscribe"),
    }

    abort_server_and_cleanup(&server_handle, &socket_path, &debug_socket_path);

    Ok(())
}

/// Test that switching models resets the provider resume session
#[tokio::test]
async fn test_model_switch_resets_provider_session() -> Result<()> {
    let _env = setup_test_env()?;
    let runtime_dir = short_runtime_dir(format!(
        "jcode-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&runtime_dir)?;
    let socket_path = runtime_dir.join("jcode.sock");
    let debug_socket_path = runtime_dir.join("jcode-debug.sock");

    let provider = Arc::new(MockProvider::with_models(vec!["model-a", "model-b"]));
    provider.queue_response(vec![
        StreamEvent::TextDelta("hello".to_string()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        },
        StreamEvent::SessionId("session-1".to_string()),
    ]);
    provider.queue_response(vec![
        StreamEvent::TextDelta("again".to_string()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        },
    ]);

    let provider_dyn: Arc<dyn jcode::provider::Provider> = provider.clone();
    let server_instance = server::Server::new_with_paths(
        provider_dyn,
        socket_path.clone(),
        debug_socket_path.clone(),
    );

    let server_handle = tokio::spawn(async move { server_instance.run().await });

    let mut client = wait_for_server_client(&socket_path).await?;

    let msg_id = client.send_message("hello").await?;
    let mut saw_done1 = false;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let event = tokio::time::timeout(Duration::from_secs(1), client.read_event()).await??;
        if matches!(event, ServerEvent::Done { id } if id == msg_id) {
            saw_done1 = true;
            break;
        }
    }
    assert!(saw_done1, "Did not receive Done for first message");

    let model_id = client.cycle_model(1).await?;
    let mut saw_model = false;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let event = tokio::time::timeout(Duration::from_secs(1), client.read_event()).await??;
        if matches!(event, ServerEvent::ModelChanged { id, error: None, .. } if id == model_id) {
            saw_model = true;
            break;
        }
    }
    assert!(saw_model, "Did not receive ModelChanged after cycle");

    let msg2_id = client.send_message("second").await?;
    let mut saw_done2 = false;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let event = tokio::time::timeout(Duration::from_secs(1), client.read_event()).await??;
        if matches!(event, ServerEvent::Done { id } if id == msg2_id) {
            saw_done2 = true;
            break;
        }
    }
    assert!(saw_done2, "Did not receive Done for second message");

    let resume_ids = provider.captured_resume_session_ids.lock().unwrap().clone();
    assert_eq!(resume_ids.len(), 2);
    assert_eq!(resume_ids[0], None);
    assert_eq!(resume_ids[1], None);

    abort_server_and_cleanup(&server_handle, &socket_path, &debug_socket_path);

    Ok(())
}

/// Test that switching models only affects the active session
#[tokio::test]
async fn test_model_switch_is_per_session() -> Result<()> {
    let _env = setup_test_env()?;
    let runtime_dir = short_runtime_dir(format!(
        "jcode-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&runtime_dir)?;
    let socket_path = runtime_dir.join("jcode.sock");
    let debug_socket_path = runtime_dir.join("jcode-debug.sock");

    let provider = Arc::new(MockProvider::with_models(vec!["model-a", "model-b"]));
    provider.queue_response(vec![
        StreamEvent::TextDelta("one".to_string()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        },
        StreamEvent::SessionId("session-1".to_string()),
    ]);
    provider.queue_response(vec![
        StreamEvent::TextDelta("two".to_string()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        },
        StreamEvent::SessionId("session-2".to_string()),
    ]);
    provider.queue_response(vec![
        StreamEvent::TextDelta("three".to_string()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        },
    ]);

    let provider_dyn: Arc<dyn jcode::provider::Provider> = provider.clone();
    let server_instance = server::Server::new_with_paths(
        provider_dyn,
        socket_path.clone(),
        debug_socket_path.clone(),
    );

    let server_handle = tokio::spawn(async move { server_instance.run().await });

    let mut client1 = wait_for_server_client(&socket_path).await?;
    let mut client2 = server::Client::connect_with_path(socket_path.clone()).await?;

    // Give server time to set up both client sessions
    tokio::time::sleep(Duration::from_millis(100)).await;

    let msg1 = client1.send_message("hello").await?;
    let mut done1 = false;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let event = tokio::time::timeout(Duration::from_secs(1), client1.read_event()).await??;
        if matches!(event, ServerEvent::Done { id } if id == msg1) {
            done1 = true;
            break;
        }
    }
    assert!(done1, "Did not receive Done for client1 message");

    let msg2 = client2.send_message("hello").await?;
    let mut done2 = false;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let event = tokio::time::timeout(Duration::from_secs(1), client2.read_event()).await??;
        if matches!(event, ServerEvent::Done { id } if id == msg2) {
            done2 = true;
            break;
        }
    }
    assert!(done2, "Did not receive Done for client2 message");

    let model_id = client1.cycle_model(1).await?;
    let mut saw_model = false;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let event = tokio::time::timeout(Duration::from_secs(1), client1.read_event()).await??;
        if matches!(event, ServerEvent::ModelChanged { id, error: None, .. } if id == model_id) {
            saw_model = true;
            break;
        }
    }
    assert!(saw_model, "Did not receive ModelChanged after cycle");

    let msg3 = client2.send_message("after").await?;
    let mut done3 = false;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let event = tokio::time::timeout(Duration::from_secs(1), client2.read_event()).await??;
        if matches!(event, ServerEvent::Done { id } if id == msg3) {
            done3 = true;
            break;
        }
    }
    assert!(done3, "Did not receive Done for client2 after switch");

    let models = provider.captured_models.lock().unwrap().clone();
    assert!(models.len() >= 3, "Expected at least 3 model captures");
    assert_eq!(models[2], "model-a");

    abort_server_and_cleanup(&server_handle, &socket_path, &debug_socket_path);

    Ok(())
}

/// Test that the system prompt does NOT identify the agent as "Claude Code"
/// The agent should identify as "jcode" or just a generic "coding assistant powered by Claude"
#[tokio::test]
async fn test_system_prompt_no_claude_code_identity() -> Result<()> {
    let _env = setup_test_env()?;
    let provider = Arc::new(MockProvider::new());

    // Queue a simple response
    provider.queue_response(vec![
        StreamEvent::TextDelta("I'm a coding assistant.".to_string()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        },
        StreamEvent::SessionId("test-identity-123".to_string()),
    ]);

    // Keep a clone of Arc<MockProvider> before converting to Arc<dyn Provider>
    let provider_for_check = provider.clone();
    let provider_dyn: Arc<dyn jcode::provider::Provider> = provider;
    let registry = Registry::new(provider_dyn.clone()).await;
    let mut agent = Agent::new(provider_dyn, registry);

    // Run a simple query - we just need to trigger a complete() call
    let _ = agent.run_once_capture("Who are you?").await?;

    // Get the captured system prompt from our Arc<MockProvider>
    let captured_prompts = provider_for_check.captured_system_prompts.lock().unwrap();

    assert!(
        !captured_prompts.is_empty(),
        "No system prompts were captured"
    );

    let system_prompt = &captured_prompts[0];

    // Check only the identity portion at the start of the system prompt.
    // User-provided instruction files may legitimately mention Claude Code CLI.
    // The first ~500 chars contain the identity statement
    let identity_portion = if system_prompt.len() > 500 {
        &system_prompt[..500]
    } else {
        system_prompt
    };
    let lower_identity = identity_portion.to_lowercase();

    // The identity portion should NOT say "you are claude code" or similar
    assert!(
        !lower_identity.contains("you are claude code"),
        "System prompt should NOT identify as 'You are Claude Code'. Found: {}",
        identity_portion
    );

    // Should identify as jcode
    assert!(
        lower_identity.contains("jcode"),
        "System prompt should identify as jcode. Found: {}",
        identity_portion
    );

    // It's OK if it says "powered by Claude" or just "Claude" (the model name)
    // It's OK if project context references "Claude Code CLI" as a tool

    Ok(())
}
