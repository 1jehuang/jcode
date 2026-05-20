use super::events::desktop_event_from_server_value;
use super::*;
use serde_json::{Value, json};
use std::io::{self, BufRead, BufReader, Write};
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
#[cfg(unix)]
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(unix)]
static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn validates_safe_session_ids() -> Result<()> {
    validate_resume_session_id("session_cow_123-abc.def")?;
    assert!(validate_resume_session_id("bad/id").is_err());
    assert!(validate_resume_session_id("bad id").is_err());
    Ok(())
}

#[test]
fn compact_title_shortens_long_titles() {
    let title = compact_title("this is a very long title that should become shorter for terminals");
    assert!(title.ends_with('…'));
    assert!(title.chars().count() <= 49);
}

#[test]
fn desktop_event_parser_maps_streaming_server_events() {
    assert_eq!(
        desktop_event_from_server_value(&json!({"type": "text_delta", "text": "hello"})),
        Some(DesktopSessionEvent::TextDelta("hello".to_string()))
    );
    assert_eq!(
        desktop_event_from_server_value(&json!({"type": "done", "id": 2})),
        Some(DesktopSessionEvent::Done)
    );
    assert_eq!(
        desktop_event_from_server_value(&json!({"type": "tool_start", "name": "bash"})),
        Some(DesktopSessionEvent::ToolStarted {
            name: "bash".to_string()
        })
    );
    assert_eq!(
        desktop_event_from_server_value(&json!({"type": "tool_input", "delta": "{\"command\":"})),
        Some(DesktopSessionEvent::ToolInput {
            delta: "{\"command\":".to_string()
        })
    );
    assert_eq!(
        desktop_event_from_server_value(&json!({"type": "tool_exec", "name": "bash"})),
        Some(DesktopSessionEvent::ToolExecuting {
            name: "bash".to_string()
        })
    );
    assert_eq!(
        desktop_event_from_server_value(&json!({
            "type": "tool_done",
            "name": "bash",
            "output": "hello\nworld"
        })),
        Some(DesktopSessionEvent::ToolFinished {
            name: "bash".to_string(),
            summary: "hello".to_string(),
            is_error: false
        })
    );
    assert_eq!(
        desktop_event_from_server_value(&json!({"type": "interrupted"})),
        Some(DesktopSessionEvent::Status(
            DesktopSessionStatus::Interrupted
        ))
    );
    assert_eq!(
        desktop_event_from_server_value(
            &json!({"type": "connection_phase", "phase": "using tool bash"})
        ),
        Some(DesktopSessionEvent::Status(
            DesktopSessionStatus::External {
                label: "using tool bash".to_string(),
                in_flight: true,
            }
        ))
    );
    assert_eq!(
        desktop_event_from_server_value(
            &json!({"type": "reasoning_effort_changed", "effort": "high"})
        ),
        Some(DesktopSessionEvent::Status(
            DesktopSessionStatus::ReasoningEffort("high".to_string())
        ))
    );
    assert_eq!(
        desktop_event_from_server_value(&json!({
            "type": "service_tier_changed",
            "service_tier": "priority"
        })),
        Some(DesktopSessionEvent::Status(
            DesktopSessionStatus::ServiceTier("priority".to_string())
        ))
    );
    assert_eq!(
        desktop_event_from_server_value(&json!({
            "type": "transport_changed",
            "transport": "websocket"
        })),
        Some(DesktopSessionEvent::Status(
            DesktopSessionStatus::Transport("websocket".to_string())
        ))
    );
    assert_eq!(
        desktop_event_from_server_value(&json!({
            "type": "compaction_mode_changed",
            "mode": "semantic"
        })),
        Some(DesktopSessionEvent::Status(
            DesktopSessionStatus::CompactionMode("semantic".to_string())
        ))
    );
    assert_eq!(
        desktop_event_from_server_value(&json!({
            "type": "compact_result",
            "message": "Compaction started",
            "success": true
        })),
        Some(DesktopSessionEvent::Status(
            DesktopSessionStatus::CompactResult {
                message: "Compaction started".to_string(),
                success: true,
            }
        ))
    );
    assert_eq!(
        desktop_event_from_server_value(&json!({
            "type": "session_renamed",
            "session_id": "session_test",
            "title": "Custom title",
            "display_title": "Custom title"
        })),
        Some(DesktopSessionEvent::SessionRenamed {
            title: Some("Custom title".to_string()),
            display_title: "Custom title".to_string(),
        })
    );
    assert_eq!(
        desktop_event_from_server_value(&json!({
            "type": "reloading",
            "new_socket": "/tmp/jcode-new.sock"
        })),
        Some(DesktopSessionEvent::Reloading {
            new_socket: Some("/tmp/jcode-new.sock".to_string())
        })
    );
    assert_eq!(
        desktop_event_from_server_value(&json!({
            "type": "model_changed",
            "model": "claude-opus-4-5",
            "provider_name": "Claude"
        })),
        Some(DesktopSessionEvent::ModelChanged {
            model: "claude-opus-4-5".to_string(),
            provider_name: Some("Claude".to_string()),
            error: None
        })
    );
    assert_eq!(
        desktop_event_from_server_value(&json!({
            "type": "history",
            "id": 7,
            "session_id": "session_test",
            "messages": [],
            "provider_name": "Claude",
            "provider_model": "claude-sonnet-4-5",
            "reasoning_effort": "high",
            "service_tier": "priority",
            "compaction_mode": "semantic",
            "available_model_routes": [
                {
                    "model": "claude-sonnet-4-5",
                    "provider": "claude",
                    "api_method": "responses",
                    "available": true,
                    "detail": "active account"
                }
            ]
        })),
        Some(DesktopSessionEvent::ModelCatalog {
            current_model: Some("claude-sonnet-4-5".to_string()),
            provider_name: Some("Claude".to_string()),
            models: vec![DesktopModelChoice {
                model: "claude-sonnet-4-5".to_string(),
                provider: Some("claude".to_string()),
                api_method: Some("responses".to_string()),
                detail: Some("active account".to_string()),
                available: true,
            }],
            reasoning_effort: Some("high".to_string()),
            service_tier: Some("priority".to_string()),
            compaction_mode: Some("semantic".to_string()),
        })
    );
    assert_eq!(
        desktop_event_from_server_value(&json!({
            "type": "stdin_request",
            "request_id": "stdin-1",
            "prompt": "Password:",
            "is_password": true,
            "tool_call_id": "tool-1"
        })),
        Some(DesktopSessionEvent::StdinRequest {
            request_id: "stdin-1".to_string(),
            prompt: "Password:".to_string(),
            is_password: true,
            tool_call_id: "tool-1".to_string()
        })
    );
    assert_eq!(
        desktop_event_from_server_value(&json!({
            "type": "stdin_request",
            "prompt": "Password:",
            "is_password": true,
            "tool_call_id": "tool-1"
        })),
        None,
        "malformed stdin requests must not fall back to request_id=unknown"
    );
    assert_eq!(
        desktop_event_from_server_value(&json!({
            "type": "stdin_request",
            "request_id": "stdin-1",
            "prompt": "Password:",
            "is_password": true
        })),
        None,
        "malformed stdin requests must not fall back to tool_call_id=unknown"
    );
}

#[test]
fn desktop_session_handle_sends_cancel_command() {
    let (command_tx, command_rx) = mpsc::channel();
    let handle = DesktopSessionHandle { command_tx };

    handle.cancel().unwrap();

    assert_eq!(command_rx.try_recv(), Ok(DesktopSessionCommand::Cancel));
}

#[test]
fn desktop_session_handle_sends_stdin_response_command() {
    let (command_tx, command_rx) = mpsc::channel();
    let handle = DesktopSessionHandle { command_tx };

    handle
        .send_stdin_response("stdin-1".to_string(), "secret".to_string())
        .unwrap();

    assert_eq!(
        command_rx.try_recv(),
        Ok(DesktopSessionCommand::StdinResponse {
            request_id: "stdin-1".to_string(),
            input: "secret".to_string()
        })
    );
}

#[test]
fn desktop_session_worker_slots_are_bounded_and_released() -> Result<()> {
    let counter = AtomicUsize::new(0);
    let first = try_acquire_desktop_session_worker_slot(&counter, 2)?;
    let second = try_acquire_desktop_session_worker_slot(&counter, 2)?;

    assert!(try_acquire_desktop_session_worker_slot(&counter, 2).is_err());
    drop(first);
    let third = try_acquire_desktop_session_worker_slot(&counter, 2)?;
    assert!(try_acquire_desktop_session_worker_slot(&counter, 2).is_err());
    drop(second);
    drop(third);
    assert_eq!(counter.load(Ordering::Relaxed), 0);
    Ok(())
}

#[cfg(unix)]
#[test]
fn desktop_worker_roundtrips_message_with_fake_server() -> Result<()> {
    let _guard = ENV_LOCK.lock().unwrap();
    let socket_path = std::env::temp_dir().join(format!(
        "jcode-desktop-worker-smoke-{}-{}.sock",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&socket_path);
    let listener = UnixListener::bind(&socket_path)?;
    let previous_socket = std::env::var_os("JCODE_SOCKET");
    unsafe {
        std::env::set_var("JCODE_SOCKET", &socket_path);
    }

    let server = std::thread::spawn(move || fake_desktop_server_roundtrip(listener));
    let (event_tx, event_rx) = mpsc::channel();
    let (_command_tx, command_rx) = mpsc::channel();

    let result = run_server_session(
        None,
        "hello desktop",
        vec![("image/png".to_string(), "abc123".to_string())],
        Some(event_tx),
        command_rx,
    );

    restore_env_var("JCODE_SOCKET", previous_socket);
    let _ = std::fs::remove_file(&socket_path);

    assert_eq!(result?, "session_desktop_fake");
    let requests = server.join().unwrap()?;
    assert_eq!(requests[0]["type"], "subscribe");
    assert_eq!(requests[1]["type"], "state");
    assert_eq!(requests[2]["type"], "message");
    assert_eq!(requests[2]["content"], "hello desktop");
    assert_eq!(requests[2]["images"], json!([["image/png", "abc123"]]));
    let events = event_rx.try_iter().collect::<Vec<_>>();
    assert!(events.contains(&DesktopSessionEvent::SessionStarted {
        session_id: "session_desktop_fake".to_string()
    }));
    assert!(events.contains(&DesktopSessionEvent::TextDelta(
        "fake assistant response".to_string()
    )));
    assert!(events.contains(&DesktopSessionEvent::Done));
    Ok(())
}

#[cfg(unix)]
#[test]
fn desktop_worker_emits_reloaded_before_real_done_after_fake_reload() -> Result<()> {
    let _guard = ENV_LOCK.lock().unwrap();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let socket_path = std::env::temp_dir().join(format!(
        "jcode-desktop-worker-reload-old-{}-{nonce}.sock",
        std::process::id(),
    ));
    let new_socket_path = std::env::temp_dir().join(format!(
        "jcode-desktop-worker-reload-new-{}-{nonce}.sock",
        std::process::id(),
    ));
    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_file(&new_socket_path);
    let listener = UnixListener::bind(&socket_path)?;
    let new_listener = UnixListener::bind(&new_socket_path)?;
    let previous_socket = std::env::var_os("JCODE_SOCKET");
    unsafe {
        std::env::set_var("JCODE_SOCKET", &socket_path);
    }

    let server = std::thread::spawn(move || {
        fake_desktop_server_reload_roundtrip(listener, new_listener, new_socket_path)
    });
    let (event_tx, event_rx) = mpsc::channel();
    let (_command_tx, command_rx) = mpsc::channel();

    let result = run_server_session(None, "hello reload", Vec::new(), Some(event_tx), command_rx);

    restore_env_var("JCODE_SOCKET", previous_socket);
    let _ = std::fs::remove_file(&socket_path);

    assert_eq!(result?, "session_desktop_reload_fake");
    let requests = server.join().unwrap()?;
    assert_eq!(requests[0]["type"], "subscribe");
    assert_eq!(requests[1]["type"], "state");
    assert_eq!(requests[2]["type"], "message");
    assert_eq!(requests[3]["type"], "subscribe");
    assert_eq!(
        requests[3]["target_session_id"],
        "session_desktop_reload_fake"
    );

    let events = event_rx.try_iter().collect::<Vec<_>>();
    let reload_index = events
        .iter()
        .position(|event| matches!(event, DesktopSessionEvent::Reloading { .. }))
        .expect("worker should forward reload event");
    let reloaded_index = events
        .iter()
        .position(|event| {
            matches!(
                event,
                DesktopSessionEvent::Reloaded { session_id }
                    if session_id == "session_desktop_reload_fake"
            )
        })
        .expect("worker should emit explicit reload completion");
    let done_index = events
        .iter()
        .position(|event| matches!(event, DesktopSessionEvent::Done))
        .expect("worker should forward real message Done after reconnect");
    assert!(reload_index < reloaded_index);
    assert!(reloaded_index < done_index);
    Ok(())
}

#[cfg(unix)]
#[test]
fn desktop_worker_rejects_reconnect_session_id_mismatch() -> Result<()> {
    let _guard = ENV_LOCK.lock().unwrap();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let socket_path = std::env::temp_dir().join(format!(
        "jcode-desktop-worker-reload-mismatch-old-{}-{nonce}.sock",
        std::process::id(),
    ));
    let new_socket_path = std::env::temp_dir().join(format!(
        "jcode-desktop-worker-reload-mismatch-new-{}-{nonce}.sock",
        std::process::id(),
    ));
    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_file(&new_socket_path);
    let listener = UnixListener::bind(&socket_path)?;
    let new_listener = UnixListener::bind(&new_socket_path)?;
    let previous_socket = std::env::var_os("JCODE_SOCKET");
    unsafe {
        std::env::set_var("JCODE_SOCKET", &socket_path);
    }

    let server = std::thread::spawn(move || {
        fake_desktop_server_reload_session_mismatch(listener, new_listener, new_socket_path)
    });
    let (event_tx, event_rx) = mpsc::channel();
    let (_command_tx, command_rx) = mpsc::channel();

    let result = run_server_session(None, "hello reload", Vec::new(), Some(event_tx), command_rx);

    restore_env_var("JCODE_SOCKET", previous_socket);
    let _ = std::fs::remove_file(&socket_path);

    let error = result.expect_err("reconnect must reject a different session id");
    assert!(
        format!("{error:#}").contains("unexpected session id"),
        "{error:#}"
    );
    let requests = server.join().unwrap()?;
    assert_eq!(
        requests[3]["target_session_id"],
        "session_desktop_reload_fake"
    );
    let events = event_rx.try_iter().collect::<Vec<_>>();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, DesktopSessionEvent::Reloading { .. }))
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, DesktopSessionEvent::Reloaded { .. })),
        "must not report reload success for the wrong session: {events:?}"
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn desktop_worker_rejects_malformed_server_event_lines() -> Result<()> {
    let _guard = ENV_LOCK.lock().unwrap();
    let socket_path = std::env::temp_dir().join(format!(
        "jcode-desktop-worker-malformed-{}-{}.sock",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&socket_path);
    let listener = UnixListener::bind(&socket_path)?;
    let previous_socket = std::env::var_os("JCODE_SOCKET");
    unsafe {
        std::env::set_var("JCODE_SOCKET", &socket_path);
    }

    let server = std::thread::spawn(move || fake_desktop_server_malformed_event(listener));
    let (event_tx, _event_rx) = mpsc::channel();
    let (_command_tx, command_rx) = mpsc::channel();

    let result = run_server_session(
        None,
        "hello malformed",
        Vec::new(),
        Some(event_tx),
        command_rx,
    );

    restore_env_var("JCODE_SOCKET", previous_socket);
    let _ = std::fs::remove_file(&socket_path);

    let error = result.expect_err("malformed JSON must fail the session worker");
    assert!(
        format!("{error:#}").contains("failed to parse jcode server event"),
        "{error:#}"
    );
    let requests = server.join().unwrap()?;
    assert_eq!(requests[2]["type"], "message");
    Ok(())
}

#[cfg(unix)]
#[test]
fn drain_session_events_treats_cancel_completion_as_terminal() -> Result<()> {
    let (client, server) = UnixStream::pair()?;
    client.set_read_timeout(Some(Duration::from_secs(2)))?;
    server.set_read_timeout(Some(Duration::from_secs(2)))?;
    let mut writer = client.try_clone()?;
    let reader = BufReader::new(client);
    let mut server_writer = server.try_clone()?;
    let mut server_reader = BufReader::new(server);

    let server = std::thread::spawn(move || -> Result<Value> {
        let request = read_fake_server_request(&mut server_reader)?;
        assert_eq!(request["type"], "cancel");
        write_json_line(
            &mut server_writer,
            json!({"type": "done", "id": request["id"]}),
        )?;
        Ok(request)
    });

    let (command_tx, command_rx) = mpsc::channel();
    command_tx.send(DesktopSessionCommand::Cancel).unwrap();
    let (event_tx, event_rx) = mpsc::channel();
    let mut next_request_id = 2_u64;

    let outcome = drain_session_events(
        reader,
        &mut writer,
        &mut next_request_id,
        Some(&event_tx),
        &command_rx,
        1,
    )?;

    assert!(matches!(outcome, DrainOutcome::Terminal));
    let request = server.join().unwrap()?;
    assert_eq!(request["id"], 2);
    let events = event_rx.try_iter().collect::<Vec<_>>();
    assert!(events.contains(&DesktopSessionEvent::Status(
        DesktopSessionStatus::Cancelling
    )));
    assert!(events.contains(&DesktopSessionEvent::Status(
        DesktopSessionStatus::Cancelled
    )));
    assert!(events.contains(&DesktopSessionEvent::Done));
    Ok(())
}

#[cfg(unix)]
#[test]
fn validate_reload_socket_path_requires_owned_socket_in_current_directory() -> Result<()> {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let current_socket = std::env::temp_dir().join(format!(
        "jcode-desktop-reload-validate-current-{}-{nonce}.sock",
        std::process::id(),
    ));
    let new_socket = std::env::temp_dir().join(format!(
        "jcode-desktop-reload-validate-new-{}-{nonce}.sock",
        std::process::id(),
    ));
    let non_socket = std::env::temp_dir().join(format!(
        "jcode-desktop-reload-validate-file-{}-{nonce}",
        std::process::id(),
    ));
    let _ = std::fs::remove_file(&current_socket);
    let _ = std::fs::remove_file(&new_socket);
    let _ = std::fs::remove_file(&non_socket);
    let current_listener = UnixListener::bind(&current_socket)?;
    let new_listener = UnixListener::bind(&new_socket)?;

    assert_eq!(
        validate_reload_socket_path(&current_socket, new_socket.to_str().unwrap())?,
        new_socket
    );
    assert!(validate_reload_socket_path(&current_socket, "relative.sock").is_err());

    std::fs::write(&non_socket, b"not a socket")?;
    let error = validate_reload_socket_path(&current_socket, non_socket.to_str().unwrap())
        .expect_err("plain files must be rejected as reload sockets");
    assert!(format!("{error:#}").contains("not a socket"), "{error:#}");

    drop(current_listener);
    drop(new_listener);
    let _ = std::fs::remove_file(&current_socket);
    let _ = std::fs::remove_file(&new_socket);
    let _ = std::fs::remove_file(&non_socket);
    Ok(())
}

#[cfg(unix)]
#[test]
fn workspace_send_message_uses_shared_server_instead_of_prompt_argv() -> Result<()> {
    let _guard = ENV_LOCK.lock().unwrap();
    let socket_path = std::env::temp_dir().join(format!(
        "jcode-desktop-workspace-send-{}-{}.sock",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&socket_path);
    let listener = UnixListener::bind(&socket_path)?;
    let previous_socket = std::env::var_os("JCODE_SOCKET");
    unsafe {
        std::env::set_var("JCODE_SOCKET", &socket_path);
    }

    let server = std::thread::spawn(move || fake_desktop_server_roundtrip(listener));

    send_message_to_session(
        "session_desktop_fake",
        "workspace title",
        "hello workspace without argv",
    )?;

    let requests = server.join().unwrap()?;

    restore_env_var("JCODE_SOCKET", previous_socket);
    let _ = std::fs::remove_file(&socket_path);

    assert_eq!(requests[0]["target_session_id"], "session_desktop_fake");
    assert_eq!(requests[2]["content"], "hello workspace without argv");
    Ok(())
}

#[cfg(unix)]
#[test]
fn desktop_workers_reconnect_independently_across_same_fake_reload() -> Result<()> {
    let _guard = ENV_LOCK.lock().unwrap();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let socket_path = std::env::temp_dir().join(format!(
        "jcode-desktop-worker-multi-reload-old-{}-{nonce}.sock",
        std::process::id(),
    ));
    let new_socket_path = std::env::temp_dir().join(format!(
        "jcode-desktop-worker-multi-reload-new-{}-{nonce}.sock",
        std::process::id(),
    ));
    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_file(&new_socket_path);
    let listener = UnixListener::bind(&socket_path)?;
    let new_listener = UnixListener::bind(&new_socket_path)?;
    let previous_socket = std::env::var_os("JCODE_SOCKET");
    unsafe {
        std::env::set_var("JCODE_SOCKET", &socket_path);
    }

    let server = std::thread::spawn(move || {
        fake_desktop_server_multi_client_reload_roundtrip(listener, new_listener, new_socket_path)
    });
    let (event_tx_one, event_rx_one) = mpsc::channel();
    let (event_tx_two, event_rx_two) = mpsc::channel();
    let (_command_tx_one, command_rx_one) = mpsc::channel();
    let (_command_tx_two, command_rx_two) = mpsc::channel();

    let client_one = std::thread::spawn(move || {
        run_server_session(
            None,
            "client one",
            Vec::new(),
            Some(event_tx_one),
            command_rx_one,
        )
    });
    let client_two = std::thread::spawn(move || {
        run_server_session(
            None,
            "client two",
            Vec::new(),
            Some(event_tx_two),
            command_rx_two,
        )
    });

    let result_one = client_one.join().unwrap()?;
    let result_two = client_two.join().unwrap()?;
    restore_env_var("JCODE_SOCKET", previous_socket);
    let _ = std::fs::remove_file(&socket_path);

    let mut results = vec![result_one.clone(), result_two.clone()];
    results.sort();
    assert_eq!(
        results,
        vec![
            "session_desktop_multi_reload_1".to_string(),
            "session_desktop_multi_reload_2".to_string(),
        ]
    );

    let requests = server.join().unwrap()?;
    let reconnect_targets = requests
        .iter()
        .filter(|request| request.get("type").and_then(Value::as_str) == Some("subscribe"))
        .filter_map(|request| request.get("target_session_id").and_then(Value::as_str))
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(reconnect_targets.len(), 2);
    assert!(reconnect_targets.contains("session_desktop_multi_reload_1"));
    assert!(reconnect_targets.contains("session_desktop_multi_reload_2"));

    assert_client_reload_sequence(event_rx_one.try_iter().collect(), &result_one);
    assert_client_reload_sequence(event_rx_two.try_iter().collect(), &result_two);
    Ok(())
}

#[cfg(unix)]
fn fake_desktop_server_roundtrip(listener: UnixListener) -> Result<Vec<Value>> {
    let (mut reader, mut writer, subscribe) = accept_first_requesting_client(&listener)?;
    write_json_line(&mut writer, json!({"type": "ack", "id": subscribe["id"]}))?;
    write_json_line(&mut writer, json!({"type": "mcp_status", "servers": []}))?;
    write_json_line(&mut writer, json!({"type": "done", "id": subscribe["id"]}))?;

    let state = read_fake_server_request(&mut reader)?;
    write_json_line(
        &mut writer,
        json!({
            "type": "state",
            "id": state["id"],
            "session_id": "session_desktop_fake",
            "message_count": 0,
            "is_processing": false,
        }),
    )?;

    let message = read_fake_server_request(&mut reader)?;
    write_json_line(&mut writer, json!({"type": "ack", "id": message["id"]}))?;
    write_json_line(
        &mut writer,
        json!({"type": "text_delta", "text": "fake assistant response"}),
    )?;
    write_json_line(&mut writer, json!({"type": "done", "id": message["id"]}))?;
    Ok(vec![subscribe, state, message])
}

#[cfg(unix)]
fn fake_desktop_server_reload_roundtrip(
    listener: UnixListener,
    new_listener: UnixListener,
    new_socket_path: PathBuf,
) -> Result<Vec<Value>> {
    let (mut reader, mut writer, subscribe) = accept_first_requesting_client(&listener)?;
    write_json_line(&mut writer, json!({"type": "ack", "id": subscribe["id"]}))?;
    write_json_line(&mut writer, json!({"type": "done", "id": subscribe["id"]}))?;

    let state = read_fake_server_request(&mut reader)?;
    write_json_line(
        &mut writer,
        json!({
            "type": "state",
            "id": state["id"],
            "session_id": "session_desktop_reload_fake",
            "message_count": 0,
            "is_processing": false,
        }),
    )?;

    let message = read_fake_server_request(&mut reader)?;
    write_json_line(&mut writer, json!({"type": "ack", "id": message["id"]}))?;
    write_json_line(
        &mut writer,
        json!({"type": "reloading", "new_socket": new_socket_path.display().to_string()}),
    )?;
    // This terminal event belongs to the socket generation that just announced reload.
    // The worker should leave that stream immediately and must not forward it.
    let _ = write_json_line(&mut writer, json!({"type": "done", "id": message["id"]}));
    drop(writer);
    drop(reader);

    let (new_reader, mut new_writer, reconnect_subscribe) =
        accept_first_requesting_client(&new_listener)?;
    write_json_line(
        &mut new_writer,
        json!({
            "type": "session",
            "session_id": "session_desktop_reload_fake",
        }),
    )?;
    write_json_line(
        &mut new_writer,
        json!({"type": "done", "id": message["id"]}),
    )?;
    drop(new_reader);

    let _ = std::fs::remove_file(new_socket_path);
    Ok(vec![subscribe, state, message, reconnect_subscribe])
}

#[cfg(unix)]
fn fake_desktop_server_reload_session_mismatch(
    listener: UnixListener,
    new_listener: UnixListener,
    new_socket_path: PathBuf,
) -> Result<Vec<Value>> {
    let (mut reader, mut writer, subscribe) = accept_first_requesting_client(&listener)?;
    write_json_line(&mut writer, json!({"type": "ack", "id": subscribe["id"]}))?;
    write_json_line(&mut writer, json!({"type": "done", "id": subscribe["id"]}))?;

    let state = read_fake_server_request(&mut reader)?;
    write_json_line(
        &mut writer,
        json!({
            "type": "state",
            "id": state["id"],
            "session_id": "session_desktop_reload_fake",
            "message_count": 0,
            "is_processing": false,
        }),
    )?;

    let message = read_fake_server_request(&mut reader)?;
    write_json_line(&mut writer, json!({"type": "ack", "id": message["id"]}))?;
    write_json_line(
        &mut writer,
        json!({"type": "reloading", "new_socket": new_socket_path.display().to_string()}),
    )?;
    drop(writer);
    drop(reader);

    let (new_reader, mut new_writer, reconnect_subscribe) =
        accept_first_requesting_client(&new_listener)?;
    write_json_line(
        &mut new_writer,
        json!({
            "type": "session",
            "session_id": "session_desktop_wrong_reload_fake",
        }),
    )?;
    drop(new_reader);

    let _ = std::fs::remove_file(new_socket_path);
    Ok(vec![subscribe, state, message, reconnect_subscribe])
}

#[cfg(unix)]
fn fake_desktop_server_malformed_event(listener: UnixListener) -> Result<Vec<Value>> {
    let (mut reader, mut writer, subscribe) = accept_first_requesting_client(&listener)?;
    write_json_line(&mut writer, json!({"type": "ack", "id": subscribe["id"]}))?;
    write_json_line(&mut writer, json!({"type": "done", "id": subscribe["id"]}))?;

    let state = read_fake_server_request(&mut reader)?;
    write_json_line(
        &mut writer,
        json!({
            "type": "state",
            "id": state["id"],
            "session_id": "session_desktop_malformed_fake",
            "message_count": 0,
            "is_processing": false,
        }),
    )?;

    let message = read_fake_server_request(&mut reader)?;
    write_json_line(&mut writer, json!({"type": "ack", "id": message["id"]}))?;
    writer.write_all(b"not-json\n")?;
    writer.flush()?;
    Ok(vec![subscribe, state, message])
}

#[cfg(unix)]
fn fake_desktop_server_multi_client_reload_roundtrip(
    listener: UnixListener,
    new_listener: UnixListener,
    new_socket_path: PathBuf,
) -> Result<Vec<Value>> {
    let first = fake_desktop_server_accept_old_reload_client(
        &listener,
        "session_desktop_multi_reload_1",
        &new_socket_path,
    )?;
    let second = fake_desktop_server_accept_old_reload_client(
        &listener,
        "session_desktop_multi_reload_2",
        &new_socket_path,
    )?;
    let message_ids = std::collections::HashMap::from([
        (first.session_id.clone(), first.message["id"].clone()),
        (second.session_id.clone(), second.message["id"].clone()),
    ]);

    let mut reconnect_requests = Vec::new();
    for _ in 0..2 {
        let (new_reader, mut new_writer, reconnect_subscribe) =
            accept_first_requesting_client(&new_listener)?;
        let session_id = reconnect_subscribe
            .get("target_session_id")
            .and_then(Value::as_str)
            .unwrap_or("missing-session")
            .to_string();
        write_json_line(
            &mut new_writer,
            json!({
                "type": "session",
                "session_id": session_id,
            }),
        )?;
        let message_id = message_ids
            .get(&session_id)
            .cloned()
            .unwrap_or_else(|| json!(3));
        write_json_line(&mut new_writer, json!({"type": "done", "id": message_id}))?;
        drop(new_reader);
        reconnect_requests.push(reconnect_subscribe);
    }

    let _ = std::fs::remove_file(new_socket_path);
    let mut requests = vec![
        first.subscribe,
        first.state,
        first.message,
        second.subscribe,
        second.state,
        second.message,
    ];
    requests.extend(reconnect_requests);
    Ok(requests)
}

#[cfg(unix)]
struct FakeReloadClientRequests {
    session_id: String,
    subscribe: Value,
    state: Value,
    message: Value,
}

#[cfg(unix)]
fn fake_desktop_server_accept_old_reload_client(
    listener: &UnixListener,
    session_id: &str,
    new_socket_path: &std::path::Path,
) -> Result<FakeReloadClientRequests> {
    let (mut reader, mut writer, subscribe) = accept_first_requesting_client(listener)?;
    write_json_line(&mut writer, json!({"type": "ack", "id": subscribe["id"]}))?;
    write_json_line(&mut writer, json!({"type": "done", "id": subscribe["id"]}))?;

    let state = read_fake_server_request(&mut reader)?;
    write_json_line(
        &mut writer,
        json!({
            "type": "state",
            "id": state["id"],
            "session_id": session_id,
            "message_count": 0,
            "is_processing": false,
        }),
    )?;

    let message = read_fake_server_request(&mut reader)?;
    write_json_line(&mut writer, json!({"type": "ack", "id": message["id"]}))?;
    write_json_line(
        &mut writer,
        json!({"type": "reloading", "new_socket": new_socket_path.display().to_string()}),
    )?;
    let _ = write_json_line(&mut writer, json!({"type": "done", "id": message["id"]}));
    drop(writer);
    drop(reader);

    Ok(FakeReloadClientRequests {
        session_id: session_id.to_string(),
        subscribe,
        state,
        message,
    })
}

fn assert_client_reload_sequence(events: Vec<DesktopSessionEvent>, session_id: &str) {
    let reload_index = events
        .iter()
        .position(|event| matches!(event, DesktopSessionEvent::Reloading { .. }))
        .expect("client should see reload start");
    let reloaded_index = events
        .iter()
        .position(|event| {
            matches!(
                event,
                DesktopSessionEvent::Reloaded { session_id: reloaded }
                    if reloaded == session_id
            )
        })
        .expect("client should see reload completion for its own session");
    let done_indices = events
        .iter()
        .enumerate()
        .filter_map(|(index, event)| matches!(event, DesktopSessionEvent::Done).then_some(index))
        .collect::<Vec<_>>();
    assert_eq!(
        done_indices.len(),
        1,
        "stale old-socket Done must not be forwarded: {events:?}"
    );
    assert!(reload_index < reloaded_index, "{events:?}");
    assert!(reloaded_index < done_indices[0], "{events:?}");
}

#[cfg(unix)]
fn accept_first_requesting_client(
    listener: &UnixListener,
) -> Result<(BufReader<UnixStream>, UnixStream, Value)> {
    loop {
        let (stream, _) = listener.accept()?;
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut first_line = String::new();
        match reader.read_line(&mut first_line) {
            Ok(0) => continue,
            Ok(_) => {
                let first_request = serde_json::from_str(first_line.trim())?;
                return Ok((reader, stream, first_request));
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(error) => return Err(error.into()),
        }
    }
}

#[cfg(unix)]
fn read_fake_server_request(reader: &mut BufReader<UnixStream>) -> Result<Value> {
    let mut line = String::new();
    reader.read_line(&mut line)?;
    Ok(serde_json::from_str(line.trim())?)
}

fn restore_env_var(key: &str, value: Option<std::ffi::OsString>) {
    unsafe {
        if let Some(value) = value {
            std::env::set_var(key, value);
        } else {
            std::env::remove_var(key);
        }
    }
}
