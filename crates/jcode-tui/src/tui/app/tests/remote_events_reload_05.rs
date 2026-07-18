// Regression tests for issue #391: a queued message must survive a reload or
// disconnect that races the turn-end dispatch, staying queued until the turn
// naturally completes instead of silently disappearing.
//
// The drop happened when a queued follow-up had already been dequeued into an
// in-flight send. That in-flight shape lives only in
// `rate_limit_pending_message` as `is_system && !auto_retry`, which has no
// retry path: the tick resend requires a `rate_limit_reset` timestamp and the
// disconnect resend requires `auto_retry`. Both the disconnect handler and the
// reload snapshot must therefore fold it back into the queue.

#[test]
fn test_disconnect_recovers_inflight_queued_continuation_to_queue() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();

    // A queued follow-up was dequeued and handed to begin_remote_send: it now
    // lives only in rate_limit_pending_message with the queued-continuation
    // shape (is_system, no auto-retry, no scheduled reset).
    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    app.current_message_id = Some(12);
    app.rate_limit_pending_message = Some(PendingRemoteMessage {
        content: "queued follow-up in flight".to_string(),
        images: vec![],
        is_system: true,
        system_reminder: Some("hidden reminder".to_string()),
        auto_retry: false,
        retry_attempts: 0,
        retry_at: None,
    });
    app.rate_limit_reset = None;

    let mut state = remote::RemoteRunState::default();
    remote::handle_disconnect(&mut app, &mut state, None);

    // The in-flight continuation must be back on the queue, not dropped.
    assert_eq!(app.queued_messages(), &["queued follow-up in flight"]);
    assert_eq!(app.hidden_queued_system_messages, vec!["hidden reminder"]);
    assert!(
        app.rate_limit_pending_message.is_none(),
        "recovered continuation must not linger as an unreachable pending message"
    );
}

#[test]
fn test_disconnect_still_clears_pending_for_non_queued_shapes() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();

    // A plain user message pending (not the queued-continuation shape) keeps
    // the old behavior when it cannot schedule a retry: exhausted attempts
    // clear it rather than re-queueing as a system continuation.
    app.is_processing = true;
    app.rate_limit_pending_message = Some(PendingRemoteMessage {
        content: "user retry message".to_string(),
        images: vec![],
        is_system: false,
        system_reminder: None,
        auto_retry: true,
        retry_attempts: u8::MAX,
        retry_at: None,
    });

    let mut state = remote::RemoteRunState::default();
    remote::handle_disconnect(&mut app, &mut state, None);

    assert!(
        app.queued_messages().is_empty(),
        "non-continuation pending shapes must not be converted into queued messages"
    );
}

#[test]
fn test_save_input_for_reload_persists_inflight_queued_continuation() {
    let mut app = create_test_app();
    let session_id = format!("test-391-inflight-{}", std::process::id());

    // Simulate the reload racing the dispatch: one message still queued, one
    // already dequeued into the in-flight pending slot.
    app.queued_messages.push("still queued".to_string());
    app.rate_limit_pending_message = Some(PendingRemoteMessage {
        content: "dispatched but unfinished".to_string(),
        images: vec![],
        is_system: true,
        system_reminder: Some("hidden reminder".to_string()),
        auto_retry: false,
        retry_attempts: 0,
        retry_at: None,
    });
    app.rate_limit_reset = None;

    app.save_input_for_reload(&session_id);

    let restored = App::restore_input_for_reload(&session_id).expect("reload state should exist");
    assert_eq!(
        restored.queued_messages,
        vec!["dispatched but unfinished", "still queued"],
        "the in-flight continuation must be persisted at the front of the queue"
    );
    assert_eq!(
        restored.hidden_queued_system_messages,
        vec!["hidden reminder"]
    );
    assert!(
        restored.rate_limit_pending_message.is_none(),
        "the continuation must not also restore as an unreachable pending message"
    );
}

#[test]
fn test_reload_preserves_completed_confidence_spike_challenge() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        let reload_session_id = format!("test-confidence-spike-reload-{}", std::process::id());
        app.todo_confidence_spike_challenged = true;
        app.save_input_for_reload(&reload_session_id);

        let restored = App::restore_input_for_reload(&reload_session_id)
            .expect("confidence challenge state should survive reload");
        let mut reloaded_app = create_test_app();
        reloaded_app.apply_restored_reload_input(restored);
        assert!(reloaded_app.todo_confidence_spike_challenged);

        crate::todo::save_todos(
            &reloaded_app.session.id,
            &[crate::todo::TodoItem {
                id: "todo-1".to_string(),
                content: "Validate release result".to_string(),
                status: "completed".to_string(),
                priority: "high".to_string(),
                confidence: Some(100),
                completion_confidence: Some(100),
                confidence_history: vec![70, 100],
                ..Default::default()
            }],
        )
        .expect("save completed todo");

        assert!(!reloaded_app.schedule_auto_poke_followup_if_needed());
        assert!(!reloaded_app.auto_poke_incomplete_todos);
        assert!(!reloaded_app.todo_confidence_spike_challenged);
        assert!(reloaded_app.hidden_queued_system_messages.is_empty());
    });
}

#[test]
fn test_completion_gate_nudges_stop_after_budget_exhausted() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.auto_poke_incomplete_todos = true;

        // A completed todo with confidence below the gate threshold keeps the
        // completion gate failing on every check.
        crate::todo::save_todos(
            &app.session.id,
            &[crate::todo::TodoItem {
                id: "todo-1".to_string(),
                content: "Ship the fix".to_string(),
                status: "completed".to_string(),
                priority: "high".to_string(),
                confidence: Some(50),
                completion_confidence: Some(50),
                confidence_history: vec![50],
                ..Default::default()
            }],
        )
        .expect("save low-confidence completed todo");

        // Each scheduled nudge consumes budget. Simulate the dispatch loop by
        // clearing the queued state between iterations (as if the turn ran and
        // the model made no todo progress).
        for attempt in 0..App::TODO_COMPLETION_GATE_MAX_ATTEMPTS {
            assert!(
                app.schedule_auto_poke_followup_if_needed(),
                "attempt {attempt} should still schedule a gate nudge"
            );
            app.hidden_queued_system_messages.clear();
            app.pending_queued_dispatch = false;
        }

        // Budget exhausted: the gate must stop scheduling and disarm auto-poke
        // instead of looping forever (observed live as one API call per ~5s).
        assert!(
            !app.schedule_auto_poke_followup_if_needed(),
            "exhausted gate must not schedule another nudge"
        );
        assert!(!app.auto_poke_incomplete_todos);
        assert!(!app.pending_queued_dispatch);
        assert!(app.hidden_queued_system_messages.is_empty());
        assert_eq!(app.todo_completion_gate_attempts, 0);
    });
}

#[test]
fn test_save_input_for_reload_removes_stale_file_when_state_is_empty() {
    let mut app = create_test_app();
    let session_id = format!("test-391-stale-{}", std::process::id());

    // First reload snapshot holds a queued message.
    app.queued_messages.push("old queued".to_string());
    app.save_input_for_reload(&session_id);

    let path = crate::storage::jcode_dir()
        .expect("jcode dir")
        .join(format!("client-input-{}", session_id));
    assert!(path.exists(), "first save should write the reload file");

    // An empty save while the file is FRESH must preserve it: another client
    // attached to the same session may have just saved its own queued
    // messages during the same reload handoff.
    app.queued_messages.clear();
    app.save_input_for_reload(&session_id);
    assert!(
        path.exists(),
        "an empty save must not delete a fresh reload file (multi-client safety)"
    );

    // Backdate the file past the staleness window; now an empty save must
    // remove it so a long-stale queue cannot resurrect on a later restore.
    #[cfg(unix)]
    {
        let stale_age_secs = 400; // > 300s staleness cutoff
        let target = std::time::SystemTime::now() - Duration::from_secs(stale_age_secs);
        let since_epoch = target
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .expect("clock before epoch");
        let times = [
            libc::timespec {
                tv_sec: since_epoch.as_secs() as libc::time_t,
                tv_nsec: 0,
            },
            libc::timespec {
                tv_sec: since_epoch.as_secs() as libc::time_t,
                tv_nsec: 0,
            },
        ];
        let c_path = std::ffi::CString::new(path.to_str().expect("utf8 path")).expect("c path");
        let rc = unsafe { libc::utimensat(libc::AT_FDCWD, c_path.as_ptr(), times.as_ptr(), 0) };
        assert_eq!(rc, 0, "backdating the reload file mtime should succeed");

        app.save_input_for_reload(&session_id);
        assert!(
            !path.exists(),
            "an empty save must remove a stale reload file so old queued messages cannot resurrect"
        );
    }
    #[cfg(not(unix))]
    {
        let _ = std::fs::remove_file(&path);
    }
}

/// Repeated provider guardrail refusals must trip the circuit breaker and
/// disarm auto-poke instead of re-sending the refused request forever
/// (observed live: `[guardrail] ... refusal` alternating with
/// `Auto-poking: N incomplete todos` every ~7s, one refused API call each).
#[test]
fn test_repeated_guardrail_refusals_stop_auto_poke_loop() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        remote.mark_history_loaded();

        app.is_remote = true;
        app.auto_poke_incomplete_todos = true;

        crate::todo::save_todos(
            &app.session.id,
            &[crate::todo::TodoItem {
                id: "todo-1".to_string(),
                content: "Never-finishing task".to_string(),
                status: "in_progress".to_string(),
                priority: "high".to_string(),
                ..Default::default()
            }],
        )
        .expect("save incomplete todo");

        let run_refused_turn = |app: &mut App, remote: &mut _, id: u64| {
            app.is_processing = true;
            app.status = ProcessingStatus::Streaming;
            app.current_message_id = Some(id);
            app.handle_server_event(
                crate::protocol::ServerEvent::ProviderGuardrail {
                    stop_reason: Some("refusal".to_string()),
                    message: "Provider guardrail stopped the response (stop_reason: refusal)."
                        .to_string(),
                },
                remote,
            );
            app.handle_server_event(crate::protocol::ServerEvent::Done { id }, remote);
            // Simulate the queued poke actually dispatching before the next turn.
            app.queued_messages.clear();
            app.hidden_queued_system_messages.clear();
            app.pending_queued_dispatch = false;
        };

        // First refusal: still under budget, auto-poke may schedule again.
        run_refused_turn(&mut app, &mut remote, 1);
        assert!(
            app.auto_poke_incomplete_todos,
            "one refusal alone must not disarm auto-poke"
        );
        assert_eq!(app.consecutive_guardrail_stops, 1);

        // Second consecutive refusal: circuit breaker must trip.
        run_refused_turn(&mut app, &mut remote, 2);
        assert!(
            !app.auto_poke_incomplete_todos,
            "repeated refusals must disarm auto-poke"
        );
        assert!(
            app.queued_messages.is_empty(),
            "no poke follow-up may stay queued after the breaker trips"
        );
        assert!(
            app.display_messages()
                .iter()
                .any(|m| m.role == "system" && m.content.contains("guardrail refused")),
            "the user should be told why auto-poke stopped"
        );

        // A successful turn resets the streak once auto-poke is re-armed.
        commands::activate_auto_poke(&mut app);
        assert_eq!(app.consecutive_guardrail_stops, 0);
        app.is_processing = true;
        app.status = ProcessingStatus::Streaming;
        app.current_message_id = Some(3);
        app.handle_server_event(crate::protocol::ServerEvent::Done { id: 3 }, &mut remote);
        assert_eq!(app.consecutive_guardrail_stops, 0);
        assert!(
            app.auto_poke_incomplete_todos,
            "a clean turn must keep auto-poke armed"
        );
    });
}
