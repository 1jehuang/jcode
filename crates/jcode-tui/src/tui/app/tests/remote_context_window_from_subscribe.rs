// Regression coverage for the launcher context window, client half.
//
// 48de3e706 made the server report the route on subscribe
// (crates/jcode-app-core/src/server/client_session.rs:898-922). The client
// handler (crates/jcode-tui/src/tui/app/remote/server_events.rs:2299-2351) is
// the other half of that contract: it must apply the server's `context_window`
// to the panel (`App.context_limit`) and to the compaction trigger budget.
//
// A remote client's own provider is `InertRuntimeProvider`, which carries no
// model catalog, so `context_window()` on it answers the generic
// DEFAULT_CONTEXT_LIMIT of 200_000. The server is the only side that knows the
// real number, so if the value does not cross the wire the panel shows 200K for
// a route that is actually 1M. Measured on stealth/space-bunny-alpha@Stealth.
//
// These tests feed the exact event a launcher session receives and read the
// resulting App state, so they assert the behaviour the fix claims rather than
// re-stating the fix.

/// The model name deliberately carries a `@Provider` pin, the form the launcher
/// actually reports for a routed model. It is not a plain catalog id, so the
/// client's local resolver cannot recover the real window for it - only the
/// server-reported value can.
const PINNED_MODEL: &str = "stealth/space-bunny-alpha@Stealth";

fn compaction_budget(app: &App) -> usize {
    app.registry
        .compaction()
        .try_read()
        .expect("compaction budget should be readable right after the event")
        .token_budget()
}

/// A launcher session's subscribe-time `ModelChanged` must land the
/// server-resolved window in the panel and in the compaction budget.
#[test]
fn subscribe_model_changed_applies_server_context_window_to_app() {
    with_temp_jcode_home(|| {
        // `None` resume id = the launcher starting a brand-new session, the
        // exact path that regressed.
        let mut app = App::new_for_remote(None);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        // Precondition: the client's own provider is the inert placeholder that
        // reports the generic default. If it happened to know the real window,
        // this test could no longer tell the server's value from a local guess.
        assert!(
            app.is_remote,
            "the launcher path must exercise the remote client"
        );
        assert_eq!(
            app.provider.context_window(),
            200_000,
            "precondition: a remote client's own provider is inert and reports the 200K default"
        );

        app.handle_server_event(
            crate::protocol::ServerEvent::ModelChanged {
                id: 1,
                model: PINNED_MODEL.to_string(),
                provider_name: Some("Stealth".to_string()),
                context_window: Some(1_000_000),
                error: None,
                resolved_credential: None,
                reasoning_effort: Some("high".to_string()),
            },
            &mut remote,
        );

        assert_eq!(
            app.context_limit, 1_000_000,
            "the server-resolved window must reach the panel; falling back to the client's inert \
             provider is what showed 200K for a 1M route"
        );
        assert_eq!(
            compaction_budget(&app),
            1_000_000,
            "the compaction trigger must use the same window, or it fires about five times too \
             early on a 1M route"
        );
    });
}

/// The whole launch sequence, in the order the server emits it, must leave the
/// panel on the server's number. A `Done` that finalised subscribe state before
/// `ModelChanged` arrived would still flash the default.
#[test]
fn subscribe_sequence_session_id_model_changed_done_keeps_server_window() {
    with_temp_jcode_home(|| {
        let mut app = App::new_for_remote(None);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        app.handle_server_event(
            crate::protocol::ServerEvent::SessionId {
                session_id: "ses_launcher_window_01".to_string(),
            },
            &mut remote,
        );
        app.handle_server_event(
            crate::protocol::ServerEvent::ModelChanged {
                id: 9,
                model: PINNED_MODEL.to_string(),
                provider_name: Some("Stealth".to_string()),
                context_window: Some(1_000_000),
                error: None,
                resolved_credential: None,
                reasoning_effort: None,
            },
            &mut remote,
        );
        app.handle_server_event(crate::protocol::ServerEvent::Done { id: 9 }, &mut remote);

        assert_eq!(
            app.context_limit, 1_000_000,
            "the launcher's full subscribe sequence must leave the server-resolved window in place"
        );
        assert_eq!(
            compaction_budget(&app),
            1_000_000,
            "Done must not leave the compaction budget on the stale default"
        );
    });
}

/// A later `ModelChanged` that omits the window must not downgrade a value the
/// server already reported. `None` means "unknown", and for a remote client the
/// only local answer available is the inert 200K default, so treating it as
/// authoritative would silently walk a 1M route back down to 200K.
#[test]
fn model_changed_without_a_window_does_not_downgrade_a_reported_window() {
    with_temp_jcode_home(|| {
        let mut app = App::new_for_remote(None);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        app.handle_server_event(
            crate::protocol::ServerEvent::ModelChanged {
                id: 1,
                model: PINNED_MODEL.to_string(),
                provider_name: Some("Stealth".to_string()),
                context_window: Some(1_000_000),
                error: None,
                resolved_credential: None,
                reasoning_effort: None,
            },
            &mut remote,
        );
        assert_eq!(app.context_limit, 1_000_000);

        // Same route, window omitted (an older peer, or a peer that does not
        // know it). The reported value must survive.
        app.handle_server_event(
            crate::protocol::ServerEvent::ModelChanged {
                id: 2,
                model: PINNED_MODEL.to_string(),
                provider_name: Some("Stealth".to_string()),
                context_window: None,
                error: None,
                resolved_credential: None,
                reasoning_effort: None,
            },
            &mut remote,
        );

        assert_eq!(
            app.context_limit, 1_000_000,
            "an omitted window means unknown, not 200K; downgrading here is how a 1M route went \
             back to displaying 200000"
        );
        assert_eq!(
            compaction_budget(&app),
            1_000_000,
            "the compaction budget must not be downgraded either"
        );
    });
}
