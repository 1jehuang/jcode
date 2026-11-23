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
