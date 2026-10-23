//! Recover transport affinity, never the response chain, after an interrupted turn.
use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum RecoveryPhase {
    Idle,
    InFlight,
    Completed,
}

/// Completion can win the transport race just before the consumer cancels.
/// In that case a completed state may already be stored, but its tool calls
/// were not necessarily observed/executed. Never implicitly carry those calls
/// forward unless the caller supplies their matching outputs.
pub(super) fn has_unanswered_tool_calls(output: &[Value], input: &[Value]) -> bool {
    output.iter().any(|item| {
        let output_type = match item.get("type").and_then(Value::as_str) {
            Some("function_call") => "function_call_output",
            Some("custom_tool_call") => "custom_tool_call_output",
            _ => return false,
        };
        let Some(call_id) = item.get("call_id").and_then(Value::as_str) else {
            return true;
        };
        !input.iter().any(|item| {
            item.get("type").and_then(Value::as_str) == Some(output_type)
                && item.get("call_id").and_then(Value::as_str) == Some(call_id)
        })
    })
}

/// Own the mutex as well as the socket. Cancellation releases the producer
/// immediately, but a successor must wait for a bounded drain before sending.
pub(super) struct RecoverableAttempt {
    pub state: Option<PersistentWsState>,
    pub phase: RecoveryPhase,
    pub slot: Option<tokio::sync::OwnedMutexGuard<Option<PersistentWsState>>>,
}

impl Drop for RecoverableAttempt {
    fn drop(&mut self) {
        if self.phase == RecoveryPhase::Idle {
            return;
        }
        let (Some(mut state), Some(mut slot)) = (self.state.take(), self.slot.take()) else {
            return;
        };
        // A cancelled future can itself be dropped after runtime teardown.
        // Destructors must never panic trying to spawn in that case. Dropping
        // the socket and empty slot is the safe fallback.
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let completed = self.phase == RecoveryPhase::Completed;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(500);
        runtime.spawn(async move {
            // Absolute deadline plus frame/byte budgets: activity cannot keep a
            // cancelled generation alive indefinitely. No response events are
            // forwarded to the caller or tool executor during this drain.
            let recovered = completed
                || tokio::time::timeout_at(deadline, drain(&mut state))
                    .await
                    .unwrap_or(false);
            if recovered {
                state.last_response_id.clear();
                state.last_response_output.clear();
                state.last_input_item_count = 0;
                state.last_input_item_hashes.clear();
                state.message_count = 0;
                state.last_activity_at = Instant::now();
                state.last_response_completed_at = Instant::now();
                *slot = Some(state);
            }
            log_openai_stream_lifecycle(
                jcode_base::logging::LogLevel::Info,
                "persistent_cancel_recovery",
                vec![
                    ("transport_retained", recovered.to_string()),
                    ("response_chain_retained", "false".to_string()),
                    ("drain_timeout_ms", "500".to_string()),
                ],
            );
        });
    }
}

async fn drain(state: &mut PersistentWsState) -> bool {
    let mut bytes = 0usize;
    for _ in 0..256 {
        match state.ws_stream.next().await {
            Some(Ok(WsMessage::Text(text))) => {
                bytes = bytes.saturating_add(text.len());
                if bytes > 1024 * 1024 {
                    return false;
                }
                let Ok(event) = serde_json::from_str::<Value>(&text) else {
                    return false;
                };
                match event.get("type").and_then(Value::as_str) {
                    Some("response.completed") => return true,
                    Some("error" | "response.failed" | "response.incomplete") => return false,
                    _ => {}
                }
            }
            Some(Ok(WsMessage::Ping(payload))) => {
                if state
                    .ws_stream
                    .send(WsMessage::Pong(payload))
                    .await
                    .is_err()
                {
                    return false;
                }
            }
            Some(Ok(WsMessage::Pong(_))) => {}
            _ => return false,
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_but_unseen_tools_require_full_replay() {
        for (call_type, output_type) in [
            ("function_call", "function_call_output"),
            ("custom_tool_call", "custom_tool_call_output"),
        ] {
            let calls = vec![serde_json::json!({"type":call_type,"call_id":"unseen"})];
            assert!(has_unanswered_tool_calls(&calls, &[]));
            assert!(has_unanswered_tool_calls(&calls, &calls));
            assert!(has_unanswered_tool_calls(
                &calls,
                &[serde_json::json!({"type":output_type,"call_id":"different"})]
            ));
            assert!(!has_unanswered_tool_calls(
                &calls,
                &[serde_json::json!({"type":output_type,"call_id":"unseen","output":"cancelled"})]
            ));
        }
        assert!(!has_unanswered_tool_calls(
            &[serde_json::json!({"type":"message"})],
            &[]
        ));
    }

    async fn pair() -> (
        PersistentWsState,
        tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            tokio_tungstenite::accept_async(tcp).await.unwrap()
        });
        let (ws_stream, _) = connect_ws(format!("ws://{addr}")).await.unwrap();
        let state = PersistentWsState {
            ws_stream,
            identity: ("fixture".into(), None, "fixture".into(), None),
            cache_key: Some("fixture-cache".into()),
            last_response_id: "unseen-response".into(),
            connected_at: Instant::now(),
            last_activity_at: Instant::now(),
            last_response_completed_at: Instant::now(),
            message_count: 2,
            last_input_item_count: 1,
            last_input_item_hashes: vec![42],
            last_response_output: vec![serde_json::json!({"type":"function_call"})],
        };
        (state, server.await.unwrap())
    }

    fn attempt(
        state: PersistentWsState,
        phase: RecoveryPhase,
    ) -> (RecoverableAttempt, Arc<Mutex<Option<PersistentWsState>>>) {
        let slot = Arc::new(Mutex::new(None));
        let guard = Arc::clone(&slot).try_lock_owned().unwrap();
        (
            RecoverableAttempt {
                state: Some(state),
                phase,
                slot: Some(guard),
            },
            slot,
        )
    }

    #[tokio::test]
    async fn cancellation_drain_rejects_errors_malformed_frames_and_budgets() {
        let _env_lock = jcode_base::storage::lock_test_env();
        let cases = vec![
            vec![WsMessage::Text("not json".into())],
            vec![WsMessage::Text(
                serde_json::json!({"type":"error"}).to_string(),
            )],
            vec![WsMessage::Text(
                serde_json::json!({"type":"response.failed"}).to_string(),
            )],
            vec![WsMessage::Text(
                serde_json::json!({"type":"response.incomplete"}).to_string(),
            )],
            vec![WsMessage::Binary(vec![1])],
            vec![WsMessage::Close(None)],
            vec![WsMessage::Text(
                serde_json::json!({"type":"response.output_text.delta",
                "delta":"x".repeat(1024 * 1024 + 1)})
                .to_string(),
            )],
            vec![WsMessage::Pong(Vec::new()); 256],
        ];
        for mut frames in cases {
            // If an invalid frame or exhausted budget is wrongly ignored,
            // this terminal event would make recovery succeed and fail the
            // assertion. Merely waiting for a timeout would miss that bug.
            frames.push(WsMessage::Text(
                serde_json::json!({"type":"response.completed"}).to_string(),
            ));
            let (state, mut peer) = pair().await;
            let sender = tokio::spawn(async move {
                for frame in frames {
                    if peer.send(frame).await.is_err() {
                        break;
                    }
                }
                // Do not make EOF the reason recovery stops. The deadline or
                // budget must work even when the server keeps the socket open.
                std::future::pending::<()>().await;
            });
            let (attempt, slot) = attempt(state, RecoveryPhase::InFlight);
            drop(attempt);
            let guard = tokio::time::timeout(Duration::from_secs(2), slot.lock())
                .await
                .unwrap();
            assert!(guard.is_none());
            sender.abort();
        }
    }

    #[tokio::test]
    async fn cancellation_drain_answers_ping_and_clears_entire_response_chain() {
        let _env_lock = jcode_base::storage::lock_test_env();
        let (state, mut peer) = pair().await;
        let connected_at = state.connected_at;
        let server = tokio::spawn(async move {
            peer.send(WsMessage::Ping(vec![1, 2, 3])).await.unwrap();
            assert_eq!(
                peer.next().await.unwrap().unwrap(),
                WsMessage::Pong(vec![1, 2, 3])
            );
            peer.send(WsMessage::Text(
                serde_json::json!({"type":"response.completed"}).to_string(),
            ))
            .await
            .unwrap();
            peer
        });
        let (attempt, slot) = attempt(state, RecoveryPhase::InFlight);
        drop(attempt);
        let guard = tokio::time::timeout(Duration::from_secs(2), slot.lock())
            .await
            .unwrap();
        let state = guard
            .as_ref()
            .expect("clean terminal event retains transport");
        assert_eq!(state.connected_at, connected_at);
        assert_eq!(state.cache_key.as_deref(), Some("fixture-cache"));
        assert!(state.last_response_id.is_empty());
        assert!(state.last_response_output.is_empty());
        assert!(state.last_input_item_hashes.is_empty());
        assert_eq!(state.last_input_item_count, 0);
        assert_eq!(state.message_count, 0);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn cancellation_after_terminal_consumed_does_not_drain_next_response() {
        let _env_lock = jcode_base::storage::lock_test_env();
        let (state, _peer) = pair().await;
        let (attempt, slot) = attempt(state, RecoveryPhase::Completed);
        drop(attempt);
        let guard = tokio::time::timeout(Duration::from_millis(100), slot.lock())
            .await
            .unwrap();
        assert!(guard.as_ref().unwrap().last_response_id.is_empty());
    }

    #[test]
    fn cancellation_guard_can_be_dropped_after_runtime_shutdown() {
        let _env_lock = jcode_base::storage::lock_test_env();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (attempt, slot, peer) = runtime.block_on(async {
            let (state, peer) = pair().await;
            let (attempt, slot) = attempt(state, RecoveryPhase::InFlight);
            (attempt, slot, peer)
        });
        drop(runtime);
        drop(attempt);
        assert!(slot.try_lock().unwrap().is_none());
        drop(peer);
    }
}
