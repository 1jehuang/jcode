use super::*;

/// Thirty text deltas 20ms apart, the shape of a provider stream.
#[derive(Clone, Default)]
struct SteadyDeltaProvider;

const STEADY_DELTAS: usize = 30;

#[async_trait]
impl Provider for SteadyDeltaProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        Ok(Box::pin(stream::unfold(0_usize, |step| async move {
            if step < STEADY_DELTAS {
                tokio::time::sleep(Duration::from_millis(20)).await;
                Some((Ok(StreamEvent::TextDelta(format!("w{step} "))), step + 1))
            } else if step == STEADY_DELTAS {
                Some((
                    Ok(StreamEvent::MessageEnd {
                        stop_reason: Some("end_turn".to_string()),
                    }),
                    step + 1,
                ))
            } else {
                None
            }
        })))
    }

    fn name(&self) -> &str {
        "steady-delta"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self)
    }
}

/// A streamed turn publishes one text event per provider delta while a client
/// is focused or has never said, and a few coalesced ones once every attached
/// client reported that it is unfocused. The text itself must be identical.
#[tokio::test(start_paused = true)]
async fn streamed_text_is_coalesced_only_when_every_client_is_unfocused() {
    let _guard = crate::storage::lock_test_env();
    let _env = IsolatedReloadRecoveryEnv::new();
    let expected: String = (0..STEADY_DELTAS).map(|step| format!("w{step} ")).collect();

    for unfocused in [false, true] {
        let provider: Arc<dyn Provider> = Arc::new(SteadyDeltaProvider);
        let registry = Registry::new(Arc::clone(&provider)).await;
        let mut agent = Agent::new(provider, registry);
        let session_id = agent.session_id().to_string();
        let connection = format!("steady-conn-{unfocused}");
        if unfocused {
            crate::stream_batching::set_connection_focus(&connection, &session_id, false, false);
        }
        // What the session fanout does with the attached connections.
        crate::stream_batching::note_session_targets(&session_id, [&connection]);

        let (tx, mut rx) = mpsc::unbounded_channel();
        agent
            .run_once_streaming_mpsc("go", Vec::new(), None, tx)
            .await
            .expect("turn completes");

        let mut text = String::new();
        let mut text_events = 0usize;
        let mut text_after_end = false;
        let mut ended = false;
        while let Ok(event) = rx.try_recv() {
            match event {
                ServerEvent::TextDelta { text: delta } => {
                    text_after_end |= ended;
                    text.push_str(&delta);
                    text_events += 1;
                }
                ServerEvent::MessageEnd { .. } | ServerEvent::Done { .. } => ended = true,
                _ => {}
            }
        }
        assert_eq!(text, expected, "unfocused={unfocused}");
        assert!(
            !text_after_end,
            "held text must precede the end of the turn"
        );
        if unfocused {
            // 600ms of stream in 250ms batches.
            assert!(
                (2..=4).contains(&text_events),
                "expected a few batches, got {text_events}"
            );
        } else {
            assert_eq!(text_events, STEADY_DELTAS);
        }
        crate::stream_batching::forget_connection(&connection);
    }
}

#[tokio::test]
async fn terminal_reconciliation_waits_for_terminal_enqueue_for_done_and_error() {
    let _guard = crate::storage::lock_test_env();
    let _env = IsolatedReloadRecoveryEnv::new();
    for failed in [false, true] {
        let (done_tx, mut done_rx) = mpsc::unbounded_channel();
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let mut task = Some(tokio::spawn(async move {
            let result = if failed {
                Err(anyhow::anyhow!("terminal test error"))
            } else {
                Ok(())
            };
            done_tx
                .send((7, result, None, tokio::task::id(), None))
                .unwrap();
            // Deterministically simulate preemption between the two sends.
            release_rx.await.unwrap();
            let terminal = if failed {
                ServerEvent::Error {
                    id: 7,
                    message: "terminal test error".into(),
                    retry_after_secs: Some(3),
                }
            } else {
                ServerEvent::Done { id: 7 }
            };
            event_tx.send(terminal).unwrap();
        }));
        let completion = done_rx.recv().await.unwrap();
        let mut processing = true;
        let mut message_id = Some(7);
        let mut session_id = Some("session_terminal_enqueue_gap".to_string());
        let mut state = ProcessingState {
            client_is_processing: &mut processing,
            message_id: &mut message_id,
            session_id: &mut session_id,
            task: &mut task,
        };
        let (disconnect_tx, _) = mpsc::unbounded_channel();
        let connections = Arc::new(RwLock::new(HashMap::from([(
            "connection".into(),
            ClientConnectionInfo {
                client_id: "connection".into(),
                session_id: "session_terminal_enqueue_gap".into(),
                client_instance_id: None,
                debug_client_id: None,
                connected_at: Instant::now(),
                last_seen: Instant::now(),
                is_processing: true,
                current_tool_name: Some("old tool".into()),
                terminal_env: Vec::new(),
                disconnect_tx,
            },
        )])));
        let members = Arc::new(RwLock::new(HashMap::new()));
        let swarms = Arc::new(RwLock::new(HashMap::new()));
        let history = Arc::new(RwLock::new(std::collections::VecDeque::new()));
        let counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let (swarm_tx, _) = broadcast::channel(8);
        let swarm = SwarmStatusRefs {
            members: &members,
            swarms_by_id: &swarms,
            event_history: &history,
            event_counter: &counter,
            event_tx: &swarm_tx,
        };
        {
            let reconcile = reconcile_processing_completions(
                Some(completion),
                &mut done_rx,
                &mut state,
                "connection",
                &connections,
                &swarm,
            );
            tokio::pin!(reconcile);
            assert!(
                futures::poll!(&mut reconcile).is_pending(),
                "must not retire before terminal enqueue"
            );
            assert!(event_rx.try_recv().is_err());
            release_tx.send(()).unwrap();
            reconcile.await;
        }
        assert!(!*state.client_is_processing);
        assert!(state.message_id.is_none());
        assert!(state.session_id.is_none());
        assert!(state.task.is_none());
        let connections = connections.read().await;
        assert!(!connections["connection"].is_processing);
        assert!(connections["connection"].current_tool_name.is_none());
        let terminal = event_rx
            .try_recv()
            .expect("terminal is queued before new turn can start");
        if failed {
            assert!(matches!(
                terminal,
                ServerEvent::Error {
                    id: 7,
                    retry_after_secs: Some(3),
                    ..
                }
            ));
        } else {
            assert!(matches!(terminal, ServerEvent::Done { id: 7 }));
        }
        assert!(event_rx.try_recv().is_err(), "one terminal event only");
    }
}
