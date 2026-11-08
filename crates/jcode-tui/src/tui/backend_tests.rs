use super::*;
use std::time::Duration;

#[test]
fn recent_history_negotiation_preserves_local_verified_prefix() {
    let base = HistoryBase {
        count: 7,
        fingerprint: 123,
        images_fingerprint: Some(456),
        recent_messages: None,
    };
    let local = subscribe_history_base(Some(base), false).unwrap();
    assert_eq!(local.count, 7);
    assert_eq!(local.fingerprint, 123);
    let remote = subscribe_history_base(Some(base), true).unwrap();
    assert_eq!(remote.count, 0);
    assert_eq!(remote.recent_messages, Some(128));
    assert!(subscribe_history_base(None, false).is_none());
    assert_eq!(
        subscribe_history_base(None, true).unwrap().recent_messages,
        Some(128)
    );
}

#[tokio::test]
async fn recent_history_page_request_explicitly_selects_normal_transcript() {
    let mut remote = RemoteConnection::dummy();
    let id = remote.get_compacted_history(192, true).await.unwrap();
    let peer = remote._dummy_peer.as_ref().unwrap();
    peer.readable().await.unwrap();
    let mut bytes = [0; 256];
    let count = peer.try_read(&mut bytes).unwrap();
    assert!(
        matches!(serde_json::from_slice::<Request>(&bytes[..count]).unwrap(),
        Request::GetCompactedHistory { id: actual, visible_messages: 192, recent_history: true } if actual == id)
    );
}

#[tokio::test]
async fn slow_usage_hydration_does_not_block_subscribe_or_async_runtime() {
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let hydration = RemoteConnection::spawn_usage_hydration(move || {
        started_tx.send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        false
    });
    started_rx.await.unwrap();
    let remote = RemoteConnection::dummy();
    let peer = remote._dummy_peer.as_ref().unwrap();
    let start = Instant::now();
    let result = tokio::time::timeout(Duration::from_millis(250), async {
        remote
            .send_request(Request::GetHistory { id: 9 })
            .await
            .unwrap();
        let mut bytes = [0; 256];
        peer.readable().await.unwrap();
        let count = peer.try_read(&mut bytes).unwrap();
        assert!(matches!(
            serde_json::from_slice::<Request>(&bytes[..count]).unwrap(),
            Request::GetHistory { id: 9 }
        ));
    })
    .await;
    let elapsed = start.elapsed();
    release_tx.send(()).unwrap();
    hydration.await.unwrap();
    result.expect("quota disk reads must not delay bootstrap requests");
    eprintln!(
        "bootstrap while snapshot read blocked: {}us",
        elapsed.as_micros()
    );
}

#[test]
fn measure_local_quota_hydration_cost() {
    let socket = server::socket_path();
    let started = Instant::now();
    let imported = crate::usage::hydrate_remote_account_usage(&socket);
    eprintln!(
        "local quota hydration: {}us, imported={imported}",
        started.elapsed().as_micros()
    );
}

#[tokio::test]
async fn ssh_readiness_waits_for_daemon_and_preserves_buffered_events() {
    let mut remote = RemoteConnection::dummy();
    let peer = remote.take_dummy_peer().unwrap();
    let (reader, mut writer) = peer.into_split();
    let (ready, released) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let mut reader = BufReader::new(reader);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        let Request::Ping { id } = serde_json::from_str(&line).unwrap() else {
            panic!("readiness must precede Subscribe");
        };
        released.await.unwrap();
        writer
            .write_all(
                format!(
                    "{{\"type\":\"pong\",\"id\":{id},\"native_ssh_protocol\":1}}\nnext-event\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
    });
    {
        let readiness = remote.verify_ssh_ready(Duration::from_secs(1));
        tokio::pin!(readiness);
        assert!(
            tokio::time::timeout(Duration::from_millis(5), &mut readiness)
                .await
                .is_err()
        );
        ready.send(()).unwrap();
        readiness.await.unwrap();
    }
    assert!(remote.session_id.is_none());
    let mut next = String::new();
    remote.reader.read_line(&mut next).await.unwrap();
    assert_eq!(next, "next-event\n");
    server.await.unwrap();
}

#[tokio::test]
async fn ssh_readiness_rejects_locally_accepted_but_closed_adapter() {
    let mut remote = RemoteConnection::dummy();
    let peer = remote.take_dummy_peer().unwrap();
    let server = tokio::spawn(async move {
        let mut reader = BufReader::new(peer);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        // Simulate the adapter discovering a failed ProxyCommand handshake.
    });
    assert!(
        remote
            .verify_ssh_ready(Duration::from_secs(1))
            .await
            .is_err()
    );
    server.await.unwrap();
}

#[tokio::test]
async fn ssh_readiness_times_out_without_remote_reply() {
    let mut remote = RemoteConnection::dummy();
    let _peer = remote.take_dummy_peer().unwrap();
    let error = remote
        .verify_ssh_ready(Duration::from_millis(10))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("timed out"));
}

#[tokio::test]
async fn ssh_readiness_rejects_wrong_id_old_protocol_and_oversized_reply() {
    for reply in [
        "{\"type\":\"pong\",\"id\":0,\"native_ssh_protocol\":1}\n".to_string(),
        "{\"type\":\"pong\",\"id\":42}\n".to_string(),
        "x".repeat(8193),
    ] {
        let mut remote = RemoteConnection::dummy();
        remote.next_request_id = 42;
        let peer = remote.take_dummy_peer().unwrap();
        let (reader, mut writer) = peer.into_split();
        let server = tokio::spawn(async move {
            let mut reader = BufReader::new(reader);
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            writer.write_all(reply.as_bytes()).await.unwrap();
        });
        assert!(
            remote
                .verify_ssh_ready(Duration::from_secs(1))
                .await
                .is_err()
        );
        server.await.unwrap();
    }
}

#[tokio::test]
async fn runtime_command_writes_control_request_without_prompt() {
    let mut remote = RemoteConnection::dummy();
    let peer = remote.take_dummy_peer().unwrap();
    let (reader, _writer) = peer.into_split();
    let mut reader = BufReader::new(reader);
    let args = vec!["set".to_string(), "model with spaces".to_string()];
    let id = remote
        .runtime_command("subagent_model", args.clone())
        .await
        .unwrap();
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(1), reader.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    let request: Request = serde_json::from_str(&line).unwrap();
    assert!(
        matches!(request, Request::RuntimeCommand { id: wire_id, command, args: wire_args }
        if wire_id == id && command == "subagent_model" && wire_args == args)
    );
    assert_eq!(remote.next_request_id_for_test(), id + 1);
    assert!(remote.control_done_ids.lock().unwrap().is_empty());
    line.clear();
    assert!(
        tokio::time::timeout(Duration::from_millis(10), reader.read_line(&mut line))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn cloud_session_switch_revokes_and_aborts_before_request() {
    let mut remote = RemoteConnection::dummy();
    let peer = remote.take_dummy_peer().unwrap();
    let (reader, _writer) = peer.into_split();
    let mut reader = BufReader::new(reader);
    remote.session_id = Some("old".into());
    let live = Arc::new(std::sync::atomic::AtomicBool::new(true));
    remote.bind_cloud_authority(&live);
    let callback = tokio::spawn(std::future::pending::<()>());
    remote.track_cloud_callback(&callback);
    remote.resume_session("new").await.unwrap();
    assert!(!live.load(std::sync::atomic::Ordering::SeqCst));
    assert!(callback.await.unwrap_err().is_cancelled());
    for kind in ["configure_tools", "resume_session"] {
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["type"], kind);
    }
}

#[tokio::test]
async fn cloud_clear_revokes_before_new_conversation() {
    let mut remote = RemoteConnection::dummy();
    let live = Arc::new(std::sync::atomic::AtomicBool::new(true));
    remote.bind_cloud_authority(&live);
    remote.clear().await.unwrap();
    assert!(!live.load(std::sync::atomic::Ordering::SeqCst));
}

#[tokio::test]
async fn cloud_configuration_is_written_before_message() {
    let mut remote = RemoteConnection::dummy();
    let peer = remote.take_dummy_peer().unwrap();
    let (reader, _writer) = peer.into_split();
    let mut reader = BufReader::new(reader);
    remote
        .configure_cloud_tools(crate::protocol::SessionToolConfig::default())
        .await
        .unwrap();
    remote.send_message("continue".into()).await.unwrap();
    for kind in ["configure_tools", "message"] {
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["type"], kind);
    }
}

#[tokio::test]
async fn cloud_expiry_revokes_without_midturn_protocol_write() {
    let mut remote = RemoteConnection::dummy();
    let peer = remote.take_dummy_peer().unwrap();
    let (reader, _writer) = peer.into_split();
    let mut reader = BufReader::new(reader);
    let live = Arc::new(std::sync::atomic::AtomicBool::new(true));
    RemoteConnection::cloud_expiry_revoke(live.clone(), tokio::time::Instant::now())
        .await
        .unwrap();
    assert!(!live.load(std::sync::atomic::Ordering::SeqCst));
    let mut line = String::new();
    assert!(
        tokio::time::timeout(Duration::from_millis(10), reader.read_line(&mut line))
            .await
            .is_err()
    );
}

#[test]
fn account_usage_poll_is_immediate_then_throttled() {
    let now = Instant::now();
    assert!(account_usage_poll_due(None, now));
    assert!(!account_usage_poll_due(Some(now), now));
    assert!(!account_usage_poll_due(
        Some(now),
        now + Duration::from_secs(29)
    ));
    assert!(account_usage_poll_due(
        Some(now),
        now + Duration::from_secs(30)
    ));
}

#[tokio::test]
async fn account_usage_poll_sends_once_and_filters_control_done() {
    let mut remote = RemoteConnection::dummy();
    let peer = remote.take_dummy_peer().unwrap();
    let (reader, _writer) = peer.into_split();
    let mut reader = BufReader::new(reader);
    remote.poll_account_usage().await.unwrap();
    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();
    let request: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(request["type"], "account_usage");
    let id = request["id"].as_u64().unwrap();
    let next_id = remote.next_request_id;
    remote.poll_account_usage().await.unwrap();
    assert_eq!(remote.next_request_id, next_id);
    let mut stray = 0;
    let done = serde_json::to_vec(&ServerEvent::Done { id }).unwrap();
    assert!(matches!(
        remote.classify_protocol_line(done, &mut stray),
        LineOutcome::Skip
    ));
}

#[tokio::test]
async fn route_selection_wire_marks_only_automatic_failover() {
    let mut remote = RemoteConnection::dummy();
    let peer = remote.take_dummy_peer().unwrap();
    let (reader, _writer) = peer.into_split();
    let mut reader = BufReader::new(reader);
    let selection = crate::provider::RouteSelection {
        model: "gpt-test".into(),
        runtime_key: jcode_provider_core::RuntimeKey::OpenAIOAuth,
        api_method: "openai-oauth@openai-fox".into(),
        provider_label: "OpenAI".into(),
        detail: String::new(),
    };
    remote.set_route_selection(selection.clone()).await.unwrap();
    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();
    let manual: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(manual["type"], "set_route");
    assert!(manual.get("automatic_failover").is_none());
    remote
        .set_route_selection_for_failover(selection)
        .await
        .unwrap();
    line.clear();
    reader.read_line(&mut line).await.unwrap();
    let automatic: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(automatic["type"], "set_route");
    assert_eq!(automatic["automatic_failover"], true);
}

#[tokio::test]
async fn detached_auth_changed_notification_does_not_wait_for_writer_lock() {
    let mut remote = RemoteConnection::dummy();
    let writer = remote.writer();
    let _guard = writer.lock().await;

    let start = Instant::now();
    remote.notify_auth_changed_detached();
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_millis(50),
        "detached notify_auth_changed should return immediately, took {:?}",
        elapsed
    );
    assert_eq!(remote.next_request_id, 2);
}

#[tokio::test]
async fn native_resume_filters_duplicate_control_done_but_preserves_turn_done() {
    let mut remote = RemoteConnection::dummy();
    let peer = remote.take_dummy_peer().unwrap();
    let (reader, mut writer) = peer.into_split();
    let mut reader = BufReader::new(reader);
    remote.resume_session("running-session").await.unwrap();
    let mut request = String::new();
    reader.read_line(&mut request).await.unwrap();
    let id = match serde_json::from_str::<Request>(&request).unwrap() {
        Request::ResumeSession { id, .. } => id,
        other => panic!("expected resume, got {other:?}"),
    };
    writer.write_all(format!(
        "{{\"type\":\"done\",\"id\":{id}}}\n{{\"type\":\"done\",\"id\":{id}}}\n{{\"type\":\"text_delta\",\"text\":\"Still working\"}}\n{{\"type\":\"done\",\"id\":44}}\n"
    ).as_bytes()).await.unwrap();
    assert!(matches!(remote.next_event().await,
        RemoteRead::Event(ServerEvent::TextDelta { text }) if text == "Still working"));
    assert!(matches!(
        remote.next_event().await,
        RemoteRead::Event(ServerEvent::Done { id: 44 })
    ));

    // An ordinary Message's Done still completes normally on this client.
    let message_id = remote.send_message("next turn".to_string()).await.unwrap();
    writer
        .write_all(format!("{{\"type\":\"done\",\"id\":{message_id}}}\n").as_bytes())
        .await
        .unwrap();
    assert!(matches!(remote.next_event().await,
        RemoteRead::Event(ServerEvent::Done { id }) if id == message_id));
}

#[tokio::test]
async fn explicit_resume_rearms_history_replay_without_appending_locally() {
    let mut remote = RemoteConnection::dummy();
    let peer = remote
        ._dummy_peer
        .take()
        .expect("dummy remote should retain peer stream");
    let (reader, _writer) = peer.into_split();
    let mut reader = BufReader::new(reader);
    remote.mark_history_loaded();

    remote
        .resume_session("session_orphaned_after_reload")
        .await
        .expect("resume request should send");

    assert!(
        !remote.has_loaded_history(),
        "target persisted History must not be mistaken for the source session's replay"
    );
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .await
        .expect("resume request should be readable by peer");
    assert!(matches!(
        serde_json::from_str::<Request>(&line).expect("resume request should deserialize"),
        Request::ResumeSession { session_id, .. }
            if session_id == "session_orphaned_after_reload"
    ));
}

#[tokio::test]
async fn detached_auth_changed_notification_sends_provider_hint() {
    let mut remote = RemoteConnection::dummy();
    let peer = remote
        ._dummy_peer
        .take()
        .expect("dummy remote should retain peer stream");
    let (reader, _writer) = peer.into_split();
    let mut reader = BufReader::new(reader);

    remote.notify_auth_changed_for_provider_detached(Some("azure-openai"));

    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(1), reader.read_line(&mut line))
        .await
        .expect("auth changed request should be sent before timeout")
        .expect("auth changed request should be readable by peer");

    assert_eq!(remote.next_request_id, 2);
    assert!(matches!(
        serde_json::from_str::<Request>(&line).expect("auth changed request should deserialize"),
        Request::NotifyAuthChanged {
            id: 1,
            provider: Some(provider),
            auth: None,
            prefer_strongest: false,
        } if provider == "azure-openai"
    ));
}

#[tokio::test]
async fn next_event_skips_stray_non_json_lines_before_valid_event() {
    let mut remote = RemoteConnection::dummy();
    let peer = remote
        ._dummy_peer
        .take()
        .expect("dummy remote should retain peer stream");
    let (_reader, mut writer) = peer.into_split();

    writer
        .write_all(b"raw tool output leaked onto protocol\n")
        .await
        .expect("stray line should write");
    writer
        .write_all(crate::protocol::encode_event(&ServerEvent::Done { id: 7 }).as_bytes())
        .await
        .expect("valid event should write");

    match remote.next_event().await {
        RemoteRead::Event(ServerEvent::Done { id }) => assert_eq!(id, 7),
        other => panic!("expected Done event after stray line, got {other:?}"),
    }
}

/// Regression for issue #422: a single corrupt frame (e.g. the tail half of
/// a split multi-megabyte event, or an event variant this client build does
/// not know) must not permanently kill the session. The client should skip
/// the bad line, resync on the next newline, and deliver the next valid
/// event.
#[tokio::test]
async fn next_event_skips_corrupt_json_frame_and_recovers() {
    let mut remote = RemoteConnection::dummy();
    let peer = remote
        ._dummy_peer
        .take()
        .expect("dummy remote should retain peer stream");
    let (_reader, mut writer) = peer.into_split();

    // Corrupt JSON that passes the '{' prefix check but fails to parse.
    writer
        .write_all(b"{\"type\":\"done\",\"id\":\n")
        .await
        .expect("corrupt frame should write");
    // Valid JSON that is not a ServerEvent (unknown variant / wrong shape).
    writer
        .write_all(b"{\"type\":\"event_from_a_newer_server_version\"}\n")
        .await
        .expect("unknown-variant frame should write");
    writer
        .write_all(crate::protocol::encode_event(&ServerEvent::Done { id: 9 }).as_bytes())
        .await
        .expect("valid event should write");

    match remote.next_event().await {
        RemoteRead::Event(ServerEvent::Done { id }) => assert_eq!(id, 9),
        other => panic!("expected Done event after corrupt frames, got {other:?}"),
    }
}

/// A stream that keeps failing to parse (real protocol/version mismatch)
/// must still disconnect once the stray-line budget is exhausted, instead
/// of spinning forever.
#[tokio::test]
async fn next_event_disconnects_after_too_many_corrupt_json_frames() {
    let mut remote = RemoteConnection::dummy();
    let peer = remote
        ._dummy_peer
        .take()
        .expect("dummy remote should retain peer stream");
    let (_reader, mut writer) = peer.into_split();

    for _ in 0..MAX_STRAY_REMOTE_PROTOCOL_LINES {
        writer
            .write_all(b"{\"not\":\"a server event\"}\n")
            .await
            .expect("corrupt frame should write");
    }

    match remote.next_event().await {
        RemoteRead::Disconnected(RemoteDisconnectReason::Protocol(message)) => {
            assert!(
                message.contains("too many unparseable protocol lines"),
                "unexpected protocol disconnect message: {message}"
            );
        }
        other => panic!("expected protocol disconnect after budget exhaustion, got {other:?}"),
    }
}

#[tokio::test]
async fn clear_sends_clear_request_to_remote_server() {
    let mut remote = RemoteConnection::dummy();
    let peer = remote
        ._dummy_peer
        .take()
        .expect("dummy remote should retain peer stream");
    let (reader, _writer) = peer.into_split();
    let mut reader = BufReader::new(reader);

    let request_id = remote.clear().await.expect("clear request should send");

    let mut line = String::new();
    reader
        .read_line(&mut line)
        .await
        .expect("clear request should be readable by peer");
    assert_eq!(request_id, 1);
    assert_eq!(remote.next_request_id, 2);
    assert!(matches!(
        serde_json::from_str::<Request>(&line).expect("clear request should deserialize"),
        Request::Clear { id: 1 }
    ));
}

/// Regression test for the "stuck on loading session…" bug.
///
/// `next_event` runs as a branch in the client `tokio::select!`. If it were
/// not cancellation safe, a large `History` payload that is mid-read when a
/// peer branch (redraw tick, terminal event) wins the race would lose the
/// bytes already consumed from the socket and desync the protocol stream.
/// Here we cancel `next_event` repeatedly while a large event is still
/// streaming in, then confirm the event is delivered intact.
#[tokio::test]
async fn next_event_is_cancellation_safe_for_large_payloads() {
    let mut remote = RemoteConnection::dummy();
    let peer = remote
        ._dummy_peer
        .take()
        .expect("dummy remote should retain peer stream");
    let (_reader, mut writer) = peer.into_split();

    // A History-sized payload: a single string field large enough to span
    // many socket reads, mimicking the multi-megabyte base64 image data
    // carried by a real `History` event for an image-bearing session.
    let big_text = "x".repeat(2 * 1024 * 1024);
    let event = ServerEvent::StatusDetail {
        detail: big_text.clone(),
    };
    let encoded = crate::protocol::encode_event(&event);
    let encoded_len = encoded.len();

    // Feed the encoded event in small chunks from a background task, so the
    // reader sees a partially-available line for most of the test.
    let writer_task = tokio::spawn(async move {
        for chunk in encoded.as_bytes().chunks(4096) {
            writer
                .write_all(chunk)
                .await
                .expect("chunk should write to peer");
            // Yield so the reader gets a chance to observe a partial line.
            tokio::task::yield_now().await;
        }
    });

    // Repeatedly start and immediately cancel `next_event` (the `select!`
    // peer "wins" via a zero-delay timeout) until the full line arrives.
    let event = loop {
        tokio::select! {
            biased;
            read = remote.next_event() => break read,
            _ = tokio::time::sleep(Duration::from_micros(50)) => {
                // Cancellation point: the in-flight `next_event` future is
                // dropped here. A cancellation-unsafe reader would lose
                // buffered bytes and never reassemble the event.
            }
        }
    };

    writer_task.await.expect("writer task should finish");

    match event {
        RemoteRead::Event(ServerEvent::StatusDetail { detail }) => {
            assert_eq!(
                detail.len(),
                big_text.len(),
                "large payload must survive repeated cancellations intact"
            );
            assert!(detail.bytes().all(|b| b == b'x'));
        }
        other => panic!("expected intact event after cancellations, got {other:?}"),
    }
    assert_eq!(
        remote.protocol_bytes_scanned, encoded_len,
        "fragmented frame assembly must inspect each protocol byte exactly once"
    );
}

/// A single logical event split across multiple socket writes (no trailing
/// newline until the end) must be reassembled into one event.
#[tokio::test]
async fn next_event_reassembles_event_split_across_reads() {
    let mut remote = RemoteConnection::dummy();
    let peer = remote
        ._dummy_peer
        .take()
        .expect("dummy remote should retain peer stream");
    let (_reader, mut writer) = peer.into_split();

    let encoded = crate::protocol::encode_event(&ServerEvent::Done { id: 9 });
    let bytes = encoded.as_bytes();
    let mid = bytes.len() / 2;
    writer
        .write_all(&bytes[..mid])
        .await
        .expect("first half should write");
    // Give the reader a chance to observe the partial line.
    tokio::task::yield_now().await;
    writer
        .write_all(&bytes[mid..])
        .await
        .expect("second half should write");

    match remote.next_event().await {
        RemoteRead::Event(ServerEvent::Done { id }) => assert_eq!(id, 9),
        other => panic!("expected reassembled Done event, got {other:?}"),
    }
}

/// Two events delivered back-to-back in a single socket write must both be
/// returned, with the second served from the buffer without another read.
#[tokio::test]
async fn next_event_serves_multiple_buffered_events() {
    let mut remote = RemoteConnection::dummy();
    let peer = remote
        ._dummy_peer
        .take()
        .expect("dummy remote should retain peer stream");
    let (_reader, mut writer) = peer.into_split();

    let mut payload = crate::protocol::encode_event(&ServerEvent::Done { id: 1 });
    payload.push_str(&crate::protocol::encode_event(&ServerEvent::Done { id: 2 }));
    writer
        .write_all(payload.as_bytes())
        .await
        .expect("both events should write in one chunk");
    drop(writer);

    match remote.next_event().await {
        RemoteRead::Event(ServerEvent::Done { id }) => assert_eq!(id, 1),
        other => panic!("expected first Done event, got {other:?}"),
    }
    match remote.next_event().await {
        RemoteRead::Event(ServerEvent::Done { id }) => assert_eq!(id, 2),
        other => panic!("expected second Done event, got {other:?}"),
    }
}

/// A single multi-megabyte protocol line (e.g. a `History` event with
/// embedded images) must not pin its full capacity inside the persistent
/// `read_buffer` for the rest of the connection. Once the line drains, the
/// buffer shrinks back to a bounded size, preserving any partial remainder.
#[tokio::test]
async fn take_buffered_line_shrinks_oversized_read_buffer() {
    let mut remote = RemoteConnection::dummy();
    let large_len = 4 * 1024 * 1024;
    remote.read_buffer.resize(large_len, b'x');
    remote.read_buffer.push(b'\n');
    // Trailing partial fragment of the next line must survive the shrink.
    remote.read_buffer.extend_from_slice(b"{\"partial");
    assert!(remote.read_buffer.capacity() > READ_BUFFER_SHRINK_THRESHOLD);

    let line = remote
        .take_buffered_line()
        .expect("large buffered line should be returned");
    assert_eq!(line.len(), large_len);
    assert_eq!(remote.read_buffer, b"{\"partial");
    assert!(
        remote.read_buffer.capacity() <= READ_BUFFER_RETAIN_CAPACITY,
        "read_buffer should shrink after a large line drains, capacity={}",
        remote.read_buffer.capacity()
    );
}

/// Steady-state streaming buffers (small capacity) must never shrink, so
/// normal traffic does not thrash between grow and shrink reallocations.
#[tokio::test]
async fn take_buffered_line_keeps_capacity_for_small_buffers() {
    let mut remote = RemoteConnection::dummy();
    remote.read_buffer.reserve(32 * 1024);
    let capacity = remote.read_buffer.capacity();
    remote.read_buffer.extend_from_slice(b"hello\n");

    let line = remote
        .take_buffered_line()
        .expect("buffered line should be returned");
    assert_eq!(line, b"hello");
    assert_eq!(
        remote.read_buffer.capacity(),
        capacity,
        "small read_buffer must retain its capacity"
    );
}

/// While a large backlog is still buffered (buffer mostly full), capacity
/// is retained so draining the remaining lines does not reallocate. Only
/// once the backlog empties out does the buffer shrink.
#[tokio::test]
async fn take_buffered_line_keeps_capacity_while_backlog_remains() {
    let mut remote = RemoteConnection::dummy();
    let line_len = 1024 * 1024;
    for _ in 0..3 {
        let start = remote.read_buffer.len();
        remote.read_buffer.resize(start + line_len, b'y');
        remote.read_buffer.push(b'\n');
    }
    let capacity = remote.read_buffer.capacity();

    let first = remote
        .take_buffered_line()
        .expect("first buffered line should be returned");
    assert_eq!(first.len(), line_len);
    assert_eq!(
        remote.read_buffer.capacity(),
        capacity,
        "capacity must be retained while a large backlog remains buffered"
    );

    remote
        .take_buffered_line()
        .expect("second buffered line should be returned");
    remote
        .take_buffered_line()
        .expect("third buffered line should be returned");
    assert!(
        remote.read_buffer.capacity() <= READ_BUFFER_RETAIN_CAPACITY,
        "read_buffer should shrink once the backlog drains, capacity={}",
        remote.read_buffer.capacity()
    );
}

#[test]
fn remote_protocol_frame_limit_rejects_oversize_and_overflow() {
    assert!(!remote_protocol_frame_exceeds_limit(
        MAX_REMOTE_PROTOCOL_FRAME_BYTES - 1,
        1,
    ));
    assert!(remote_protocol_frame_exceeds_limit(
        MAX_REMOTE_PROTOCOL_FRAME_BYTES,
        1,
    ));
    assert!(remote_protocol_frame_exceeds_limit(usize::MAX, 1));
}

#[tokio::test]
async fn request_send_does_not_block_ui_when_server_stops_reading() {
    // Regression: the TUI awaited socket writes inline, so when the daemon's
    // receive buffer filled while it was blocked writing to us, the UI loop
    // stopped reading and both ends deadlocked (frozen session).
    let mut remote = RemoteConnection::dummy();
    let writer = remote.writer();
    let _guard = writer.lock().await;

    let result = tokio::time::timeout(Duration::from_secs(1), async {
        for _ in 0..64 {
            remote.request_todos().await.expect("queued");
        }
    })
    .await;
    assert!(
        result.is_ok(),
        "request sends must not wait on the socket writer"
    );
}
