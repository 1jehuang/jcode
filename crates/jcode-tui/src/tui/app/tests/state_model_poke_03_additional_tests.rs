use super::*;

/// Regression: the completion-gate circuit breaker clears
/// `auto_poke_incomplete_todos`, and the only restore used to live in the
/// all-complete branch. Since the breaker can only trip while every todo is
/// complete, an agent that added open work immediately afterwards left
/// auto-poke disarmed for the rest of the session, and the guard at the top
/// of `schedule_auto_poke_followup_if_needed` returned false immediately.
#[test]
fn test_open_work_rearms_auto_poke_after_completion_gate_breaker() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();

        crate::todo::save_todos(
            &app.session.id,
            &[crate::todo::TodoItem {
                lane_after: None,
                lane_owner: None,
                stage: None,
                details: None,
                group: None,
                id: "open".to_string(),
                content: "Still open".to_string(),
                status: "in_progress".to_string(),
                priority: "high".to_string(),
                blocked_by: Vec::new(),
                assigned_to: None,
                confidence: None,
                completion_confidence: None,
                confidence_history: Vec::new(),
                waiting_reason: None,
                requires_user_auth: None,
                waiting_on: Vec::new(),
            }],
        )
        .expect("save todos");

        // The exact state the circuit breaker leaves behind: flag cleared,
        // configured default still on, nothing else touched.
        app.auto_poke_default_on = true;
        app.auto_poke_incomplete_todos = false;
        app.last_auto_poke_fingerprint = None;
        app.final_response_todo_fingerprint = None;
        app.todo_final_response_requested = false;
        app.pending_queued_dispatch = false;
        app.pending_turn = false;

        let poked = app.schedule_auto_poke_followup_if_needed();

        assert!(
            app.auto_poke_incomplete_todos,
            "open work must re-arm auto-poke after the completion-gate breaker"
        );
        assert!(poked, "open work must still produce a poke");
    });
}
