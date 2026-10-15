#[test]
fn test_finish_turn_does_not_challenge_moderate_or_unrecorded_confidence_jumps() {
    use crate::todo::ConfidenceState;
    with_temp_jcode_home(|| {
        for (planning, completion, history) in [
            (
                ConfidenceState::Plausible,
                ConfidenceState::Verified,
                vec![ConfidenceState::Plausible, ConfidenceState::Verified],
            ),
            (
                ConfidenceState::Speculative,
                ConfidenceState::Validated,
                vec![ConfidenceState::Speculative, ConfidenceState::Validated],
            ),
            // Legacy planning/completion disagreement is not a recorded jump.
            (
                ConfidenceState::Speculative,
                ConfidenceState::Verified,
                vec![],
            ),
            (
                ConfidenceState::Speculative,
                ConfidenceState::Verified,
                vec![ConfidenceState::Verified],
            ),
            // Stale history must end at the current completion confidence.
            (
                ConfidenceState::Speculative,
                ConfidenceState::Validated,
                vec![ConfidenceState::Speculative, ConfidenceState::Verified],
            ),
        ] {
            let mut app = create_test_app();
            app.is_remote = false;
            app.auto_poke_incomplete_todos = true;
            app.auto_poke_default_on = false;
            crate::todo::save_todos(
                &app.session.id,
                &[crate::todo::TodoItem {
                    id: "validated-result".into(),
                    content: "Validate the requested result".into(),
                    status: "completed".into(),
                    confidence: Some(planning),
                    completion_confidence: Some(completion),
                    confidence_history: history,
                    ..Default::default()
                }],
            )
            .unwrap();
            // Missing goal metadata is not unfinished ownership work either.
            crate::todo::save_goals(&app.session.id, &[]).unwrap();
            for id in 42..44 {
                app.is_processing = true;
                super::local::finish_turn(&mut app);
                assert!(!app.todo_confidence_spike_challenged);
                if id == 42 {
                    assert_eq!(
                        app.queued_messages,
                        vec![crate::todo::TODO_FINAL_RESPONSE_CONTINUATION_MESSAGE.to_string()]
                    );
                    assert!(app.pending_queued_dispatch);
                } else {
                    assert!(app.queued_messages.is_empty());
                    assert!(!app.pending_queued_dispatch);
                }
                app.queued_messages.clear();
                app.pending_queued_dispatch = false;
            }
            assert!(!app.auto_poke_incomplete_todos);
            assert!(app.hidden_queued_system_messages.is_empty());
            assert!(
                !app.display_messages()
                    .iter()
                    .any(|message| message.content.contains("Double-checking confidence jumps"))
            );
        }
    });
}
