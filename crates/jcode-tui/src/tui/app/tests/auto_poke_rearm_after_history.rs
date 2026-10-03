// Auto-poke re-arm must survive the window in which the plan is not in view.
//
// The re-arm is judged from more than one place. A user turn reaches
// `begin_remote_send`, but that send is also reachable before the bootstrap
// History payload has been applied, so a re-arm judged there reads an empty
// plan, declines, and is never retried. One Esc interrupt would then leave
// auto-poke disarmed for the rest of the session even though the plan still had
// open items.
//
// Each test below names one falsifiable guarantee. If the defer/settle pair is
// ever narrowed to the remote send path again, or the owed-rearm latch is
// dropped, the specific assertion that fails says which guarantee went.

fn rearm_test_app() -> crate::tui::app::App {
    let mut app = create_test_app();
    app.is_remote = true;
    app.runtime_mode = crate::tui::app::AppRuntimeMode::RemoteClient;
    app
}

fn pending_todo(id: &str) -> crate::todo::TodoItem {
    crate::todo::TodoItem {
        group: None,
        id: id.to_string(),
        content: "Still open".to_string(),
        status: "pending".to_string(),
        priority: "high".to_string(),
        blocked_by: Vec::new(),
        assigned_to: None,
        confidence: None,
        completion_confidence: None,
        confidence_history: Vec::new(),
    }
}

/// A reload with nothing queued and no startup prompt: the shape
/// `apply_restored_reload_input` sees for a plain `/reload` of a session that
/// had no pending input.
fn restored_reload_input_fixture() -> crate::tui::app::state_ui::RestoredReloadInput {
    crate::tui::app::state_ui::RestoredReloadInput {
        input: String::new(),
        cursor: 0,
        pending_images: Vec::new(),
        submit_on_restore: false,
        queued_messages: Vec::new(),
        hidden_queued_system_messages: Vec::new(),
        startup_status_notice: None,
        startup_display_message: None,
        interleave_message: None,
        pending_soft_interrupts: Vec::new(),
        pending_soft_interrupt_resend: None,
        rate_limit_pending_message: None,
        rate_limit_reset: None,
        observe_mode_enabled: false,
        observe_page_markdown: String::new(),
        observe_page_updated_at_ms: 0,
        split_view_enabled: false,
        todos_view_enabled: false,
        todo_confidence_spike_challenged: false,
        last_todo_ownership_fingerprint: None,
        final_response_todo_fingerprint: None,
    }
}

#[test]
fn restored_reload_input_defers_auto_poke_rearm_until_the_plan_is_loaded() {
    with_temp_jcode_home(|| {
        let mut app = rearm_test_app();
        // An episode-scoped stop (Esc interrupt, provider guardrail, dead
        // credential, non-retryable error) disarmed auto-poke but kept the
        // session default, so the feature owes itself a re-arm.
        app.auto_poke_incomplete_todos = false;
        app.auto_poke_default_on = true;
        // The plan is empty while the client's view of the session is being
        // rebuilt, which is exactly the state a single-point re-arm judged and
        // lost.
        crate::todo::save_todos(&app.session.id, &[]).expect("save empty plan");
        assert!(
            crate::tui::app::commands::incomplete_poke_todos(&app).is_empty(),
            "fixture precondition: no plan is visible yet"
        );

        app.apply_restored_reload_input(restored_reload_input_fixture());
        assert!(
            app.auto_poke_rearm_owed,
            "a reload must record that the re-arm decision is still owed"
        );
        assert!(
            !app.auto_poke_incomplete_todos,
            "an empty plan must not arm anything by itself"
        );

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        assert!(!remote.has_loaded_history());

        // Still pre-history: the debt has to survive this pass untouched.
        rt.block_on(crate::tui::app::remote::process_remote_followups(
            &mut app,
            &mut remote,
        ));
        assert!(app.auto_poke_rearm_owed, "the debt must not be dropped");
        assert!(
            !app.auto_poke_incomplete_todos,
            "nothing may be armed while the plan is unreadable"
        );

        // The plan arrives with the session it belongs to.
        let restored_session = "ses_reloaded_with_open_plan";
        app.remote_session_id = Some(restored_session.to_string());
        crate::todo::save_todos(restored_session, &[pending_todo("todo-1")])
            .expect("save plan with an unfinished task");
        remote.mark_history_loaded();
        rt.block_on(crate::tui::app::remote::process_remote_followups(
            &mut app,
            &mut remote,
        ));

        // Deliberately NOT the reference's placement, and the divergence is
        // intentional rather than a convenience. The reference arms as soon as
        // history arrives. Here `settle_deferred_auto_poke_rearm` has exactly one
        // caller - `schedule_auto_poke_followup_if_needed` - because that is the
        // single place that decides whether to talk to the model. Arming on
        // history arrival would leave the flag armed with no poke behind it, and
        // a session that is idle after a reload has no unfinished turn for a
        // poke to be about. What matters is that the decision is not LOST: it
        // survives the window and is acted on the moment something can act.
        assert!(
            app.auto_poke_rearm_owed,
            "the debt must survive until something can actually act on it"
        );
        assert!(
            !app.auto_poke_incomplete_todos,
            "arming must not happen merely because history landed"
        );

        assert!(
            crate::tui::app::commands::settle_deferred_auto_poke_rearm(&mut app),
            "once the plan is genuinely visible the owed re-arm must settle to armed"
        );
        assert!(
            app.auto_poke_incomplete_todos,
            "a settled re-arm leaves auto-poke armed"
        );
        assert!(
            !app.auto_poke_rearm_owed,
            "the debt is settled once auto-poke is armed again"
        );
    });
}

#[test]
fn explicit_poke_off_is_never_rearmed_after_history_loads() {
    with_temp_jcode_home(|| {
        // `/poke off` before the debt is recorded: nothing may be owed at all.
        let mut app = rearm_test_app();
        app.auto_poke_incomplete_todos = true;
        app.auto_poke_default_on = true;
        crate::tui::app::commands::disable_auto_poke(&mut app);
        assert!(!app.auto_poke_default_on, "fixture precondition");

        app.apply_restored_reload_input(restored_reload_input_fixture());
        assert!(
            !app.auto_poke_rearm_owed,
            "an explicit /poke off must not even record a debt"
        );

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        // Pre-history pass, then the plan lands. Still no arm.
        rt.block_on(crate::tui::app::remote::process_remote_followups(
            &mut app,
            &mut remote,
        ));
        let session = "ses_reloaded_after_poke_off";
        app.remote_session_id = Some(session.to_string());
        crate::todo::save_todos(session, &[pending_todo("todo-1")]).expect("save plan");
        remote.mark_history_loaded();
        rt.block_on(crate::tui::app::remote::process_remote_followups(
            &mut app,
            &mut remote,
        ));

        assert!(
            !app.auto_poke_incomplete_todos,
            "/poke off must survive the deferred re-arm path"
        );
        assert!(
            !app.auto_poke_default_on,
            "/poke off must stay a whole-session decision"
        );
        assert!(!app.auto_poke_rearm_owed);

        // The dangerous ordering: the debt is already recorded and the user
        // turns the feature off before history lands. Settling must respect the
        // off, not the older debt.
        let mut app = rearm_test_app();
        app.auto_poke_incomplete_todos = false;
        app.auto_poke_default_on = true;
        crate::todo::save_todos(&app.session.id, &[]).expect("save empty plan");
        app.apply_restored_reload_input(restored_reload_input_fixture());
        assert!(app.auto_poke_rearm_owed, "fixture precondition: debt is owed");

        crate::tui::app::commands::disable_auto_poke(&mut app);
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        rt.block_on(crate::tui::app::remote::process_remote_followups(
            &mut app,
            &mut remote,
        ));
        let session = "ses_reloaded_poke_off_after_debt";
        app.remote_session_id = Some(session.to_string());
        crate::todo::save_todos(session, &[pending_todo("todo-2")]).expect("save plan");
        remote.mark_history_loaded();
        rt.block_on(crate::tui::app::remote::process_remote_followups(
            &mut app,
            &mut remote,
        ));

        assert!(
            !app.auto_poke_incomplete_todos,
            "a debt recorded before /poke off must not re-arm the feature"
        );
        assert!(!app.auto_poke_default_on);
        assert!(
            !app.auto_poke_rearm_owed,
            "the debt must be dropped once the session turned the feature off"
        );
    });
}

#[test]
fn deferred_rearm_arms_state_without_sending_a_poke() {
    with_temp_jcode_home(|| {
        // The re-arm is pure state on purpose.
        // `schedule_auto_poke_followup_if_needed` is the single thing that
        // decides to talk to the model at the end of a turn; emitting from the
        // re-arm would produce nudges where no reminder belongs.
        let mut app = rearm_test_app();
        app.auto_poke_incomplete_todos = false;
        app.auto_poke_default_on = true;
        crate::todo::save_todos(&app.session.id, &[]).expect("save empty plan");
        app.apply_restored_reload_input(restored_reload_input_fixture());
        assert!(app.auto_poke_rearm_owed, "fixture precondition: debt is owed");

        let session = "ses_reloaded_pure_state";
        app.remote_session_id = Some(session.to_string());
        crate::todo::save_todos(session, &[pending_todo("todo-1")]).expect("save plan");

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        remote.mark_history_loaded();
        rt.block_on(crate::tui::app::remote::process_remote_followups(
            &mut app,
            &mut remote,
        ));

        assert!(app.auto_poke_incomplete_todos, "the arm happened");
        assert!(
            app.queued_messages.is_empty(),
            "the re-arm must not queue a poke: the end-of-turn scheduler owns that"
        );
        assert!(
            !app.pending_turn && !app.is_processing,
            "the re-arm must not start a turn"
        );
        assert!(
            app.display_messages()
                .iter()
                .all(|message| !crate::tui::app::commands::is_poke_message(
                    &message.content
                )),
            "the re-arm must not echo a poke into the transcript"
        );
    });
}

/// The local send path never reaches `begin_remote_send`, so if the re-arm were
/// only wired there, one Esc in a local TUI would leave auto-poke disarmed for
/// the rest of the session even with the plan unfinished.
///
/// This drives the real `submit_input` rather than calling the re-arm directly,
/// so it fails if the wiring is missing, not only if the function misbehaves.
#[test]
fn local_user_turn_rearms_auto_poke_after_an_episode_stop() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        assert!(!app.is_remote, "fixture precondition: a local session");

        crate::todo::save_todos(&app.session.id, &[pending_todo("todo-local-rearm")])
            .expect("save todos");

        app.auto_poke_incomplete_todos = true;
        app.auto_poke_default_on = true;
        // What Esc does: the episode ends, the session default survives.
        crate::tui::app::commands::stop_auto_poke_episode(&mut app);
        assert!(
            !app.auto_poke_incomplete_todos,
            "fixture precondition: disarmed"
        );
        assert!(
            app.auto_poke_default_on,
            "fixture precondition: default on"
        );

        app.input = "keep going".to_string();
        app.cursor_pos = app.input.len();
        app.submit_input();

        assert!(
            app.auto_poke_incomplete_todos,
            "a local user turn must re-arm auto-poke while the plan is unfinished"
        );
        assert!(
            app.auto_poke_default_on,
            "the session default must survive: the user never turned the feature off"
        );
    });
}

/// A local session must not drop an owed re-arm just because the user typed
/// while no plan was visible yet.
///
/// `rearm_auto_poke_if_plan_unfinished` declines when `incomplete_poke_todos` is
/// empty, and "not decided yet" is not "nothing to do": the agent can create
/// todos later in that same turn. `defer_auto_poke_rearm` is the only thing that
/// turns a decline into a retry, and if it is reachable only from the remote
/// send path then a local `submit_input` calls the re-arm directly, latches
/// nothing, and the decline is dropped permanently.
#[test]
fn local_rearm_debt_survives_a_user_turn_with_no_visible_plan() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        assert!(!app.is_remote, "fixture precondition: a local session");

        // What an episode-scoped stop leaves behind: disarmed, session default on.
        app.auto_poke_incomplete_todos = false;
        app.auto_poke_default_on = true;

        // The user types at a moment when nothing is unfinished, so the re-arm
        // cannot decide.
        crate::todo::save_todos(&app.session.id, &[]).expect("save a completed plan");
        app.input = "now do the next thing".to_string();
        app.cursor_pos = app.input.len();
        app.submit_input();

        // That turn creates new work and stops with it unfinished. Simulate the
        // end of the turn so the arm flag is the only thing that can block.
        crate::todo::save_todos(&app.session.id, &[pending_todo("todo-after-the-gap")])
            .expect("save the new plan");
        app.pending_turn = false;
        app.pending_queued_dispatch = false;

        assert!(
            app.schedule_auto_poke_followup_if_needed(),
            "an owed re-arm must not be dropped by a user turn that saw no plan: \
             the work created after it would never be poked"
        );
        assert!(
            app.auto_poke_default_on,
            "the session default must survive: the user never turned the feature off"
        );
    });
}

/// The third state neither of the two original tests covers: a poke is IN FLIGHT,
/// the user interrupts it, and then sends a NEW message. The other two tests stop
/// at the flag (`auto_poke_incomplete_todos`), which is necessary but not
/// sufficient - a session can be armed and still never poke.
///
/// This asserts the user-visible outcome (a poke is actually queued) rather than
/// the internal flag, so it fails on the silent stall where auto-poke looks
/// enabled but nothing ever fires.
///
/// Discriminating power, verified by mutation: forcing
/// `auto_poke_incomplete_todos = false` after `submit_input()` - i.e. simulating
/// pre-71f05d1b1, where `submit_input` had no re-arm call - makes this test
/// FAIL with "must produce a REAL poke, not leave the session armed and silent".
/// It PASSES on current code. So it genuinely pins the `submit_input` re-arm
/// wiring rather than restating the flag.
///
/// Note: an earlier attempt to prove this via the fingerprint guard did NOT
/// discriminate. Restoring `last_auto_poke_fingerprint` after the episode stop
/// still passed, because `rearm_auto_poke_if_plan_unfinished` clears the
/// fingerprint itself on the re-arm path (`commands.rs:285`). The fingerprint
/// reset in `stop_auto_poke_episode` is therefore redundant on this path rather
/// than load-bearing.
#[test]
fn interrupted_poke_is_replaced_by_a_real_new_poke_on_the_next_user_turn() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        assert!(!app.is_remote, "fixture precondition: a local session");

        crate::todo::save_todos(&app.session.id, &[pending_todo("todo-in-flight")])
            .expect("save todos");

        // A poke cycle is running: armed, and a turn end that actually fires one.
        app.auto_poke_incomplete_todos = true;
        app.auto_poke_default_on = true;
        app.pending_turn = false;
        app.pending_queued_dispatch = false;
        assert!(
            app.schedule_auto_poke_followup_if_needed(),
            "fixture precondition: the first poke fires"
        );
        assert!(
            app.last_auto_poke_fingerprint.is_some(),
            "fixture precondition: a fired poke records a fingerprint, which is \
             what a stale one would make the next turn end decline"
        );

        // The poke is dispatched and in flight; the user interrupts it.
        app.queued_messages.clear();
        app.pending_queued_dispatch = false;
        app.is_processing = true;
        crate::tui::app::commands::stop_auto_poke_episode(&mut app);
        assert!(
            app.last_auto_poke_fingerprint.is_none(),
            "fixture precondition: the interrupted poke left no fingerprint to \
             re-trigger the unchanged-todo guard"
        );

        // The user then sends a NEW message - the re-arm point under audit.
        app.input = "keep going".to_string();
        app.cursor_pos = app.input.len();
        app.submit_input();

        // That turn ends. Nothing is in flight any more.
        app.is_processing = false;
        app.pending_turn = false;
        app.pending_queued_dispatch = false;

        // The user-visible outcome, not the internal flag: a fresh poke fires.
        assert!(
            app.schedule_auto_poke_followup_if_needed(),
            "after an interrupt a new user message must produce a REAL poke, not \
             leave the session armed and silent"
        );
        assert!(
            app.queued_messages
                .iter()
                .any(|message| message.contains("incomplete todo")),
            "the queued follow-up must be an actual poke, got: {:?}",
            app.queued_messages
        );
        assert!(
            app.auto_poke_default_on,
            "the session default must survive the whole interrupt-then-continue cycle"
        );
    });
}

/// The re-arm is wired into exactly one place: `submit_input`
/// (`input.rs:4220` and `input.rs:4240`). A message the user sends while a turn
/// is still running goes down `queue_message` (`input.rs:1594`) or
/// `stage_local_interleave`, and NEITHER calls `rearm_auto_poke_on_user_turn`.
///
/// That is reachable right after an interrupt, because `Esc` sets
/// `cancel_requested` but leaves `is_processing` true until the cancelled turn
/// actually unwinds. A user who types their next message in that window - the
/// normal thing to do after interrupting - is on the queue path, so the re-arm
/// never runs for that turn.
///
/// This test pins the CURRENT behaviour rather than asserting a fix: it records
/// that the queue path leaves auto-poke disarmed and owes nothing, which is the
/// silent-stall shape (default still on, nothing armed, no debt to settle). It
/// fails loudly if someone later wires the re-arm into `queue_message`, at which
/// point the assertion should be inverted deliberately rather than by accident.
#[test]
fn queueing_a_message_while_processing_does_not_rearm_auto_poke() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        assert!(!app.is_remote, "fixture precondition: a local session");

        crate::todo::save_todos(&app.session.id, &[pending_todo("todo-queued-rearm")])
            .expect("save todos");

        // What Esc leaves: the episode stopped, the session default still on.
        app.auto_poke_incomplete_todos = false;
        app.auto_poke_default_on = true;

        // The cancelled turn has not unwound yet, so the next message queues.
        app.is_processing = true;
        app.queue_mode = true;
        assert_eq!(
            super::input::send_action(&app, false),
            crate::tui::app::SendAction::Queue,
            "fixture precondition: a message sent mid-turn takes the queue path"
        );

        app.input = "keep going".to_string();
        app.cursor_pos = app.input.len();
        crate::tui::app::input::queue_message(&mut app);

        // The gap: no re-arm, and no owed debt, so the end-of-turn scheduler has
        // nothing to settle. If this ever starts re-arming, invert this test.
        assert!(
            !app.auto_poke_incomplete_todos,
            "the queue path currently does NOT re-arm; see the doc comment - \
             if this now passes the other way, the re-arm was wired here and \
             this test needs inverting on purpose"
        );
        assert!(
            !app.auto_poke_rearm_owed,
            "and it latches no owed re-arm either, so this turn is a silent stall"
        );
        assert!(
            app.auto_poke_default_on,
            "while the session default still reads as enabled - the confusing part"
        );
    });
}