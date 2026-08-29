use super::*;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

fn jcode_home_test_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct ScopedJcodeHome {
    path: PathBuf,
    previous: Option<OsString>,
    _guard: MutexGuard<'static, ()>,
}

impl ScopedJcodeHome {
    fn new(label: &str) -> Self {
        let guard = jcode_home_test_lock();
        let previous = std::env::var_os("JCODE_HOME");
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "jcode-harness-api-{label}-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("create isolated JCODE_HOME");
        // SAFETY: all tests in this module that mutate JCODE_HOME share `LOCK`,
        // and this guard restores the prior value before it is released.
        unsafe { std::env::set_var("JCODE_HOME", &path) };
        Self {
            path,
            previous,
            _guard: guard,
        }
    }
}

impl Drop for ScopedJcodeHome {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(value) => unsafe { std::env::set_var("JCODE_HOME", value) },
            None => unsafe { std::env::remove_var("JCODE_HOME") },
        }
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn write_session_record(home: &Path, session_id: &str, working_dir: &Path) -> PathBuf {
    write_session_record_with_titles(home, session_id, working_dir, None, None)
}

fn write_session_record_with_titles(
    home: &Path,
    session_id: &str,
    working_dir: &Path,
    title: Option<&str>,
    custom_title: Option<&str>,
) -> PathBuf {
    let sessions = home.join("sessions");
    std::fs::create_dir_all(&sessions).expect("create sessions directory");
    let path = sessions.join(format!("{session_id}.json"));
    std::fs::write(
        &path,
        json!({
            "working_dir": working_dir,
            "title": title,
            "custom_title": custom_title,
            "messages": [{"role": "user", "content": "hello"}],
        })
        .to_string(),
    )
    .expect("write session record");
    path
}

#[test]
fn persisted_metadata_reads_large_transcripts_from_bounded_windows() {
    let home = ScopedJcodeHome::new("bounded-metadata");
    let sessions = home.path.join("sessions");
    std::fs::create_dir_all(&sessions).expect("create sessions directory");
    let path = sessions.join("session_large.json");
    let mut file = std::fs::File::create(&path).expect("create large session");
    write!(
        file,
        "{{\"id\":\"session_large\",\"title\":\"Generated title\",\"messages\":[\""
    )
    .unwrap();
    for _ in 0..(2 * 1024) {
        file.write_all(&[b'x'; 1024]).unwrap();
    }
    write!(
        file,
        "\"],\"working_dir\":\"/workspace/large\",\"custom_title\":\"Pinned title\"}}"
    )
    .unwrap();
    drop(file);

    let metadata = BridgeState::resolve_session_metadata("session_large").expect("metadata");
    assert_eq!(metadata.working_dir.as_deref(), Some("/workspace/large"));
    assert_eq!(metadata.title.as_deref(), Some("Generated title"));
    assert_eq!(metadata.custom_title.as_deref(), Some("Pinned title"));
    assert_eq!(metadata.display_title().as_deref(), Some("Pinned title"));
}

fn only_reply_event(outbound: Vec<Outbound>) -> ApiEvent {
    assert_eq!(outbound.len(), 1, "expected exactly one reply");
    match outbound.into_iter().next().expect("one outbound") {
        Outbound::Reply(frame) => frame.event,
        other => panic!("expected API reply, got {other:?}"),
    }
}

fn state_with_session() -> BridgeState {
    BridgeState {
        session_id: Some("s1".into()),
        ..Default::default()
    }
}

#[test]
fn connection_phase_is_forwarded_to_api_clients() {
    let mut state = state_with_session();
    let frames = state.legacy_event_to_api(&json!({
        "type": "connection_phase",
        "phase": "sending request",
    }));

    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].reply_to, None);
    assert_eq!(
        frames[0].event,
        ApiEvent::ConnectionPhase {
            session_id: "s1".into(),
            phase: "sending request".into(),
        }
    );
}

#[test]
fn wake_request_is_forwarded_with_explicit_session_and_payload() {
    let mut state = state_with_session();
    let frames = state.legacy_event_to_api(&json!({
        "type": "wake_requested",
        "session_id": "target",
        "reason": "background_task_completed",
        "notification": "finished",
    }));
    assert_eq!(frames.len(), 1);
    assert_eq!(
        frames[0].event,
        ApiEvent::WakeRequested {
            session_id: "target".into(),
            reason: "background_task_completed".into(),
            notification: "finished".into(),
        }
    );
}

#[test]
fn create_session_maps_to_subscribe() {
    let mut state = BridgeState::default();
    let out = state.api_request_to_legacy(&json!({"req": "create_session", "id": 1}));
    let Outbound::Legacy(value) = &out[0] else {
        panic!("expected legacy outbound");
    };
    assert_eq!(value["type"], "subscribe");
    assert!(value["working_dir"].is_string());
}

#[test]
fn desktop_owned_session_requests_crash_on_disconnect() {
    let mut state = BridgeState::with_crash_on_disconnect(true);
    let out = state.api_request_to_legacy(&json!({"req": "create_session", "id": 1}));
    let Outbound::Legacy(value) = &out[0] else {
        panic!("expected legacy outbound");
    };
    assert_eq!(value["crash_on_disconnect"], true);
}

#[test]
fn detach_disarms_crash_on_disconnect() {
    let mut state = BridgeState::with_crash_on_disconnect(true);
    let out = state.api_request_to_legacy(&json!({
        "req": "detach_session",
        "id": 2,
        "session_id": "abc",
    }));
    let Outbound::Legacy(value) = &out[0] else {
        panic!("expected legacy outbound");
    };
    assert_eq!(value["type"], "prepare_disconnect");
}

#[test]
fn state_event_answers_pending_attach() {
    let home = ScopedJcodeHome::new("attach-title");
    let project = home.path.join("project");
    std::fs::create_dir_all(&project).unwrap();
    write_session_record_with_titles(
        &home.path,
        "abc",
        &project,
        Some("Generated attach title"),
        Some("Persisted attach rename"),
    );
    let mut state = BridgeState::default();
    let out = state.api_request_to_legacy(&json!({"req": "create_session", "id": 5}));
    assert_eq!(
        out.len(),
        3,
        "subscribe + state chase + model catalog probe"
    );
    let Outbound::Legacy(state_req) = &out[1] else {
        panic!("expected legacy state request");
    };
    assert_eq!(state_req["type"], "state");
    let state_id = state_req["id"].as_u64().unwrap();

    // A subscribe `done` must not leak a turn_done.
    let done = state.legacy_event_to_api(&json!({"type": "done", "id": 1}));
    assert!(done.is_empty());

    let frames = state.legacy_event_to_api(&json!({
        "type": "state", "id": state_id, "session_id": "abc",
        "message_count": 0, "is_processing": false,
    }));
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].reply_to, Some(5));
    match &frames[0].event {
        ApiEvent::Attached { session } => {
            assert_eq!(session.session_id, "abc");
            assert_eq!(session.title.as_deref(), Some("Persisted attach rename"));
            assert_eq!(session.working_dir.as_deref(), project.to_str());
        }
        other => panic!("unexpected: {other:?}"),
    }
    assert_eq!(state.session_id.as_deref(), Some("abc"));
}

#[test]
fn send_message_then_done_becomes_turn_done() {
    let mut state = state_with_session();
    let out = state.api_request_to_legacy(
        &json!({"req": "send_message", "id": 2, "session_id": "s1", "content": "hi"}),
    );
    let Outbound::Legacy(message) = &out[0] else {
        panic!("expected legacy outbound");
    };
    assert_eq!(message["type"], "message");
    let legacy_id = message["id"].as_u64().unwrap();

    let deltas = state.legacy_event_to_api(&json!({"type": "text_delta", "text": "yo"}));
    assert!(matches!(
        &deltas[0].event,
        ApiEvent::TextDelta { session_id, text } if session_id == "s1" && text == "yo"
    ));

    let done = state.legacy_event_to_api(&json!({"type": "done", "id": legacy_id}));
    assert!(matches!(
        &done[0].event,
        ApiEvent::TurnDone { session_id } if session_id == "s1"
    ));
}

/// The daemon acking the in-flight message is the only signal that the agent
/// took delivery, so it must surface as its own event rather than being
/// swallowed as a bookkeeping ack. A client that shows "sent" until the first
/// token of the reply is showing a lie for as long as the model thinks.
#[test]
fn acking_the_pending_message_reports_acceptance() {
    let mut state = state_with_session();
    let out = state.api_request_to_legacy(
        &json!({"req": "send_message", "id": 2, "session_id": "s1", "content": "hi"}),
    );
    let Outbound::Legacy(message) = &out[0] else {
        panic!("expected legacy outbound");
    };
    let legacy_id = message["id"].as_u64().unwrap();

    let accepted = state.legacy_event_to_api(&json!({"type": "ack", "id": legacy_id}));
    assert!(matches!(
        &accepted[0].event,
        ApiEvent::MessageAccepted { session_id } if session_id == "s1"
    ));
    // The turn must still end normally: the acceptance event must not consume
    // the pending id the `done` boundary depends on.
    let done = state.legacy_event_to_api(&json!({"type": "done", "id": legacy_id}));
    assert!(matches!(&done[0].event, ApiEvent::TurnDone { .. }));
}

#[test]
fn context_only_message_waits_for_persistence_event_and_replies_ok() {
    let mut state = state_with_session();
    let out = state.api_request_to_legacy(&json!({
        "req": "send_message", "id": 27, "session_id": "s1",
        "content": "context", "no_reply": true
    }));
    let Outbound::Legacy(message) = &out[0] else {
        panic!("expected legacy outbound");
    };
    assert_eq!(message["type"], "message");
    assert_eq!(message["no_reply"], true);
    let legacy_id = message["id"].as_u64().unwrap();

    assert!(
        state
            .legacy_event_to_api(&json!({"type": "ack", "id": legacy_id}))
            .is_empty(),
        "the daemon's early ack does not prove persistence"
    );
    let frames =
        state.legacy_event_to_api(&json!({"type": "context_message_added", "id": legacy_id}));
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].reply_to, Some(27));
    assert!(matches!(frames[0].event, ApiEvent::Ok));
    assert!(
        state
            .legacy_event_to_api(&json!({"type": "done", "id": legacy_id}))
            .is_empty(),
        "context-only messages never create turn boundaries"
    );
}

#[test]
fn context_only_message_error_is_correlated_to_the_request() {
    let mut state = state_with_session();
    let out = state.api_request_to_legacy(&json!({
        "req": "send_message", "id": 28, "session_id": "s1",
        "content": "context", "no_reply": true
    }));
    let Outbound::Legacy(message) = &out[0] else {
        panic!("expected legacy outbound");
    };
    let legacy_id = message["id"].as_u64().unwrap();
    let frames = state
        .legacy_event_to_api(&json!({"type": "error", "id": legacy_id, "message": "save failed"}));
    assert_eq!(frames[0].reply_to, Some(28));
    assert!(matches!(
        &frames[0].event,
        ApiEvent::Error { message, .. } if message == "save failed"
    ));
    assert!(
        state
            .legacy_event_to_api(&json!({"type": "context_message_added", "id": legacy_id}))
            .is_empty()
    );
}

/// An ack for anything else (a ping, a clear) is still a plain request reply:
/// promoting those to acceptance would wiggle a message that nobody sent.
#[test]
fn acking_an_unrelated_request_stays_a_reply() {
    let mut state = state_with_session();
    let out = state.api_request_to_legacy(&json!({"req": "clear", "id": 9, "session_id": "s1"}));
    let Outbound::Legacy(clear) = &out[0] else {
        panic!("expected legacy outbound");
    };
    let legacy_id = clear["id"].as_u64().unwrap();
    let frames = state.legacy_event_to_api(&json!({"type": "ack", "id": legacy_id}));
    assert_eq!(frames[0].reply_to, Some(9));
    assert!(matches!(&frames[0].event, ApiEvent::Ok));
}

#[test]
fn ping_pong_roundtrip() {
    let mut state = state_with_session();
    let out = state.api_request_to_legacy(&json!({"req": "ping", "id": 9}));
    let Outbound::Legacy(ping) = &out[0] else {
        panic!("expected legacy outbound");
    };
    let legacy_id = ping["id"].as_u64().unwrap();
    let frames = state.legacy_event_to_api(&json!({"type": "pong", "id": legacy_id}));
    assert_eq!(frames[0].reply_to, Some(9));
    assert!(matches!(frames[0].event, ApiEvent::Pong));
}

#[test]
fn history_reply_is_mapped() {
    let mut state = state_with_session();
    let out = state.api_request_to_legacy(&json!({"req": "get_history", "id": 4}));
    let Outbound::Legacy(get) = &out[0] else {
        panic!("expected legacy outbound");
    };
    let legacy_id = get["id"].as_u64().unwrap();
    let frames = state.legacy_event_to_api(&json!({
        "type": "history",
        "id": legacy_id,
        "session_id": "s1",
        "messages": [{"role": "user", "content": "hi"}],
    }));
    match &frames[0].event {
        ApiEvent::History { messages, .. } => {
            assert_eq!(messages.len(), 1);
            assert_eq!(messages[0].role, "user");
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn unknown_legacy_events_are_dropped() {
    let mut state = state_with_session();
    let frames = state.legacy_event_to_api(&json!({"type": "swarm_event", "data": {}}));
    assert!(frames.is_empty());
}

#[test]
fn unknown_api_request_gets_error_reply() {
    let mut state = BridgeState::default();
    let out = state.api_request_to_legacy(&json!({"req": "frobnicate", "id": 3}));
    let Outbound::Reply(frame) = &out[0] else {
        panic!("expected direct reply");
    };
    assert_eq!(frame.reply_to, Some(3));
    assert!(matches!(
        frame.event,
        ApiEvent::Error {
            code: ErrorCode::UnknownRequest,
            ..
        }
    ));
}

#[test]
fn error_routes_to_pending_request() {
    let mut state = state_with_session();
    let out = state.api_request_to_legacy(&json!({"req": "clear", "id": 7}));
    let Outbound::Legacy(clear) = &out[0] else {
        panic!("expected legacy outbound");
    };
    let legacy_id = clear["id"].as_u64().unwrap();
    let frames =
        state.legacy_event_to_api(&json!({"type": "error", "id": legacy_id, "message": "nope"}));
    assert_eq!(frames[0].reply_to, Some(7));
}

/// Attaching must volunteer the model identity: a client that has to know to
/// ask would show "unknown model" forever, which is what this fixes.
#[test]
fn attaching_probes_and_reports_the_model() {
    let mut state = BridgeState::default();
    let out = state.api_request_to_legacy(&json!({"req": "create_session", "id": 7}));
    let Outbound::Legacy(catalog) = &out[2] else {
        panic!("expected a legacy catalog probe");
    };
    assert_eq!(catalog["type"], "get_model_catalog");
    let catalog_id = catalog["id"].as_u64().unwrap();

    // The daemon answers the probe with a `history`-shaped reply carrying no
    // messages. That must become an unsolicited model_info event, not a reply
    // to some client request that never asked for history.
    let frames = state.legacy_event_to_api(&json!({
        "type": "history", "id": catalog_id, "messages": [],
        "provider_name": "anthropic", "provider_model": "claude-sonnet-4-5",
    }));
    assert_eq!(frames.len(), 1);
    assert_eq!(
        frames[0].reply_to, None,
        "the probe was not client-initiated"
    );
    match &frames[0].event {
        ApiEvent::ModelInfo {
            provider, model, ..
        } => {
            assert_eq!(provider.as_deref(), Some("anthropic"));
            assert_eq!(model.as_deref(), Some("claude-sonnet-4-5"));
        }
        other => panic!("unexpected: {other:?}"),
    }
}

/// A real `get_history` reply must still be a history reply after the probe has
/// been consumed, or the probe would swallow the client's own request.
#[test]
fn a_client_history_request_is_untouched_by_the_probe() {
    let mut state = BridgeState::default();
    let out = state.api_request_to_legacy(&json!({"req": "create_session", "id": 1}));
    let Outbound::Legacy(catalog) = &out[2] else {
        panic!("expected a catalog probe");
    };
    let catalog_id = catalog["id"].as_u64().unwrap();
    state.legacy_event_to_api(&json!({"type": "history", "id": catalog_id, "messages": []}));

    let out = state.api_request_to_legacy(&json!({"req": "get_history", "id": 9}));
    let Outbound::Legacy(request) = &out[0] else {
        panic!("expected a legacy history request");
    };
    let history_id = request["id"].as_u64().unwrap();
    let frames = state.legacy_event_to_api(&json!({
        "type": "history", "id": history_id,
        "messages": [{"role": "user", "content": "hi"}],
    }));
    assert_eq!(frames[0].reply_to, Some(9));
    assert!(matches!(frames[0].event, ApiEvent::History { .. }));
}

/// Switching model mid-session must reach the client, or the caption goes stale
/// and confidently lies about which model answered.
#[test]
fn a_model_change_is_forwarded() {
    let mut state = state_with_session();
    let frames = state.legacy_event_to_api(&json!({
        "type": "model_changed", "id": 3,
        "model": "gpt-5.6", "provider_name": "openai",
    }));
    match &frames[0].event {
        ApiEvent::ModelInfo {
            provider, model, ..
        } => {
            assert_eq!(provider.as_deref(), Some("openai"));
            assert_eq!(model.as_deref(), Some("gpt-5.6"));
        }
        other => panic!("unexpected: {other:?}"),
    }
}

/// A failed model change must not be reported as the active model.
#[test]
fn a_failed_model_change_is_not_reported() {
    let mut state = state_with_session();
    let frames = state.legacy_event_to_api(&json!({
        "type": "model_changed", "id": 3, "model": "nope", "error": "no such model",
    }));
    assert!(frames.is_empty());
}

/// An auth change re-resolves the route, so the push must update the caption.
#[test]
fn an_available_models_push_updates_the_model() {
    let mut state = state_with_session();
    let frames = state.legacy_event_to_api(&json!({
        "type": "available_models_updated",
        "provider_name": "anthropic", "provider_model": "claude-opus-4-5",
        "available_models": ["claude-opus-4-5"],
    }));
    match &frames[0].event {
        ApiEvent::ModelInfo {
            session_id, model, ..
        } => {
            assert_eq!(session_id, "s1");
            assert_eq!(model.as_deref(), Some("claude-opus-4-5"));
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn create_session_in_a_jcode_checkout_requests_selfdev() {
    // Regression: external client opens its own crate, and without the `selfdev`
    // flag the daemon hands back an agent with no self-dev tools or prompt.
    let mut state = BridgeState::default();
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("workspace root")
        .join("crates/jcode-tui");
    let out = state.api_request_to_legacy(&json!({
        "req": "create_session",
        "id": 1,
        "working_dir": repo.display().to_string(),
    }));
    let Outbound::Legacy(value) = &out[0] else {
        panic!("expected legacy outbound");
    };
    assert_eq!(value["selfdev"], json!(true));
}

#[test]
fn create_session_outside_a_checkout_leaves_selfdev_unset() {
    let mut state = BridgeState::default();
    let out = state.api_request_to_legacy(&json!({
        "req": "create_session",
        "id": 1,
        "working_dir": "/",
    }));
    let Outbound::Legacy(value) = &out[0] else {
        panic!("expected legacy outbound");
    };
    assert!(value.get("selfdev").is_none(), "got {value}");
}

/// A turn that fails ends with `error` instead of `done`. The bridge must let
/// go of the pending message, or a later unrelated `done` reusing that legacy
/// id would be reported to the client as this turn finally finishing, and a
/// client that trusts `turn_done` would unblock on a turn that never ran.
#[test]
fn a_failed_turn_clears_the_pending_message() {
    let mut state = state_with_session();
    let out = state.api_request_to_legacy(&json!({
        "req": "send_message", "id": 11, "content": "hi",
    }));
    let Outbound::Legacy(message) = &out[0] else {
        panic!("expected a legacy message");
    };
    let legacy_id = message["id"].as_u64().expect("a legacy id");

    let frames = state.legacy_event_to_api(&json!({
        "type": "error", "id": legacy_id, "message": "dns error",
    }));
    assert!(
        frames
            .iter()
            .any(|frame| matches!(frame.event, ApiEvent::Error { .. })),
        "the failure was not forwarded"
    );

    // The same id arriving as `done` afterwards is no longer this turn.
    let frames = state.legacy_event_to_api(&json!({"type": "done", "id": legacy_id}));
    assert!(
        !frames
            .iter()
            .any(|frame| matches!(frame.event, ApiEvent::TurnDone { .. })),
        "a failed turn reported a second, phantom completion"
    );
}

#[test]
fn background_notifications_become_progress_events() {
    let mut state = state_with_session();
    let frames = state.legacy_event_to_api(&json!({
        "type": "notification",
        "from_session": "background_task",
        "message": "**Background task progress** `t9` · `bash`\n\n[#####-----] 50% · Running tests (reported)",
    }));
    assert_eq!(frames.len(), 1);
    match &frames[0].event {
        ApiEvent::BackgroundProgress {
            session_id,
            task_id,
            percent,
            done,
            ..
        } => {
            assert_eq!(session_id, "s1");
            assert_eq!(task_id, "t9");
            assert_eq!(*percent, Some(50.0));
            assert!(!done);
        }
        other => panic!("unexpected background event: {other:?}"),
    }
}

/// A DM or a shared-context push is not progress, and inventing a bar for it
/// would put a phantom task on every client's screen.
#[test]
fn unrelated_notifications_are_dropped() {
    let mut state = state_with_session();
    let frames = state.legacy_event_to_api(&json!({
        "type": "notification",
        "from_session": "fox",
        "message": "hello from another agent",
    }));
    assert!(frames.is_empty());
}

/// The daemon answers a `ping` that arrives as the first frame on a connection
/// and then closes it, because it classifies ping as a one-shot lightweight
/// control request. Forwarding an unattached ping therefore destroys the
/// client's connection before it ever gets a session, which is the opposite of
/// what a liveness probe should do.
#[test]
fn ping_before_attach_is_answered_locally() {
    let mut state = BridgeState::default();
    let out = state.api_request_to_legacy(&json!({"req": "ping", "id": 4}));
    match out.as_slice() {
        [Outbound::Reply(frame)] => {
            assert_eq!(frame.reply_to, Some(4));
            assert_eq!(frame.event, ApiEvent::Pong);
        }
        other => panic!("ping must not reach the daemon before attach: {other:?}"),
    }
}

/// Once attached the connection is a normal session connection, so ping is a
/// genuine round trip and should measure the daemon, not the bridge.
#[test]
fn ping_after_attach_reaches_the_daemon() {
    let mut state = state_with_session();
    let out = state.api_request_to_legacy(&json!({"req": "ping", "id": 5}));
    match out.as_slice() {
        [Outbound::Legacy(value)] => assert_eq!(value["type"], "ping"),
        other => panic!("expected a forwarded ping: {other:?}"),
    }
}

/// The daemon closes the connection on a stateful request that arrives before
/// a subscribe. Forwarding one therefore does not just fail the request: it
/// destroys the client's whole connection, taking every other in-flight
/// request with it, and the SDK sees a bare EPIPE. Answer locally.
#[test]
fn stateful_requests_before_attach_are_refused_locally() {
    for req in [
        "send_message",
        "cancel",
        "soft_interrupt",
        "clear",
        "rewind",
        "get_history",
    ] {
        let mut state = BridgeState::default();
        let out = state.api_request_to_legacy(&json!({
            "req": req,
            "id": 7,
            "session_id": "session_does_not_exist",
        }));
        assert_eq!(out.len(), 1, "{req} should produce exactly one reply");
        let Outbound::Reply(frame) = &out[0] else {
            panic!("{req} was forwarded to the daemon, which will close the connection");
        };
        assert_eq!(frame.reply_to, Some(7));
        match &frame.event {
            ApiEvent::Error { code, message } => {
                assert_eq!(*code, ErrorCode::UnknownSession, "{req}");
                assert!(
                    message.contains("session_does_not_exist"),
                    "{req} error should name the session: {message}"
                );
            }
            other => panic!("{req} expected an error frame, got {other:?}"),
        }
    }
}

/// The legacy protocol has no session field, so a request naming a *different*
/// session than the attached one would be applied to the attached one. A
/// `clear` or `rewind` aimed at the wrong id would then destroy a transcript
/// the caller never named.
#[test]
fn requests_for_another_session_do_not_hit_the_attached_one() {
    let mut state = state_with_session();
    let out = state.api_request_to_legacy(&json!({
        "req": "clear",
        "id": 9,
        "session_id": "some_other_session",
    }));
    let Outbound::Reply(frame) = &out[0] else {
        panic!("clear for another session must not reach the daemon");
    };
    match &frame.event {
        ApiEvent::Error { code, message } => {
            assert_eq!(*code, ErrorCode::UnknownSession);
            assert!(message.contains("s1") && message.contains("some_other_session"));
        }
        other => panic!("expected an error frame, got {other:?}"),
    }
}

/// The guard must not break the normal path: the attached session's own id,
/// and an omitted id, both still reach the daemon.
#[test]
fn attached_requests_still_reach_the_daemon() {
    let mut state = state_with_session();
    let named = state.api_request_to_legacy(&json!({
        "req": "get_history", "id": 1, "session_id": "s1",
    }));
    assert!(matches!(named[0], Outbound::Legacy(_)), "explicit id");

    let bare = state.api_request_to_legacy(&json!({"req": "get_history", "id": 2}));
    assert!(matches!(bare[0], Outbound::Legacy(_)), "omitted id");
}

/// Reading around without attaching is the entire point of `peek_session` and
/// `list_sessions`, so the attach guard must leave them alone.
#[test]
fn browsing_requests_work_without_attaching() {
    let _home = ScopedJcodeHome::new("browsing-without-attach");
    let mut state = BridgeState::default();
    for req in ["list_sessions", "peek_session", "ping"] {
        let out = state.api_request_to_legacy(&json!({
            "req": req, "id": 1, "session_id": "whatever",
        }));
        let Outbound::Reply(frame) = &out[0] else {
            panic!("{req} should be answered locally");
        };
        assert!(
            !matches!(frame.event, ApiEvent::Error { .. }),
            "{req} must not be refused by the attach guard: {:?}",
            frame.event
        );
    }
}

/// A client may pipeline: `create_session` then `send_message` without
/// awaiting the attach. The subscribe is already on the wire, so the daemon
/// will have a session by the time the message lands. Refusing here would
/// break the SDK's own `run()` path.
#[test]
fn a_message_pipelined_behind_create_session_is_forwarded() {
    let mut state = BridgeState::default();
    state.api_request_to_legacy(&json!({"req": "create_session", "id": 1}));
    let out = state.api_request_to_legacy(&json!({
        "req": "send_message", "id": 2, "content": "hi",
    }));
    let Outbound::Legacy(value) = &out[0] else {
        panic!("a pipelined message must reach the daemon, not be refused");
    };
    assert_eq!(value["type"], "message");
}

// --- Capabilities added to close the API coverage gaps --------------------

/// The catalog arrives on attach, so a picker must open without a round trip.
#[test]
fn list_models_is_answered_from_the_cached_catalog() {
    let mut state = state_with_session();
    state.legacy_event_to_api(&json!({
        "type": "available_models_updated",
        "provider_model": "claude-opus-5",
        "available_models": ["claude-opus-5", "claude-fable-5"],
    }));

    let out = state.api_request_to_legacy(&json!({"id": 9, "req": "list_models"}));
    match &out[..] {
        [Outbound::Reply(frame)] => match &frame.event {
            ApiEvent::Models {
                models, current, ..
            } => {
                assert_eq!(models, &["claude-opus-5", "claude-fable-5"]);
                assert_eq!(current.as_deref(), Some("claude-opus-5"));
            }
            other => panic!("unexpected: {other:?}"),
        },
        other => panic!("expected one local reply, got {other:?}"),
    }
}

/// A client can ask before the catalog lands. Answering "no models" then would
/// be a lie that empties its picker, so the request waits for the real answer.
#[test]
fn list_models_before_the_catalog_asks_the_daemon() {
    let mut state = state_with_session();
    let out = state.api_request_to_legacy(&json!({"id": 9, "req": "list_models"}));
    match &out[..] {
        [Outbound::Legacy(value)] => assert_eq!(value["type"], "get_model_catalog"),
        other => panic!("expected a daemon round trip, got {other:?}"),
    }

    let legacy_id = match &out[0] {
        Outbound::Legacy(value) => value["id"].as_u64().unwrap(),
        _ => unreachable!(),
    };
    let frames = state.legacy_event_to_api(&json!({
        "type": "history", "id": legacy_id, "session_id": "s1",
        "available_models": ["a", "b"], "provider_model": "a",
    }));
    match &frames[0].event {
        ApiEvent::Models { models, .. } => assert_eq!(models, &["a", "b"]),
        other => panic!("unexpected: {other:?}"),
    }
    assert_eq!(frames[0].reply_to, Some(9));
}

/// A switch must resolve the caller's request *and* tell every other client
/// watching the session that the model moved under them.
#[test]
fn a_requested_model_change_replies_and_broadcasts() {
    let mut state = state_with_session();
    let out = state.api_request_to_legacy(&json!({
        "id": 4, "req": "set_model", "model": "claude-fable-5",
    }));
    let legacy_id = match &out[..] {
        [Outbound::Legacy(value)] => {
            assert_eq!(value["type"], "set_model");
            assert_eq!(value["model"], "claude-fable-5");
            value["id"].as_u64().unwrap()
        }
        other => panic!("expected a daemon request, got {other:?}"),
    };

    let frames = state.legacy_event_to_api(&json!({
        "type": "model_changed", "id": legacy_id,
        "model": "claude-fable-5", "provider_name": "anthropic",
    }));
    assert_eq!(frames.len(), 2, "expected a reply and a broadcast");
    assert_eq!(frames[0].reply_to, Some(4));
    assert!(matches!(frames[0].event, ApiEvent::Ok));
    assert_eq!(frames[1].reply_to, None);
    assert!(matches!(frames[1].event, ApiEvent::ModelInfo { .. }));
    // The cache must follow, or a picker reopened after the switch is wrong.
    assert_eq!(state.current_model.as_deref(), Some("claude-fable-5"));
}

/// The daemon reports a rejected switch in-band, on a success-shaped event.
/// Reporting success there would leave the client's picker showing a model
/// the session is not using.
#[test]
fn a_rejected_model_change_fails_the_request() {
    let mut state = state_with_session();
    let out = state.api_request_to_legacy(&json!({
        "id": 4, "req": "set_model", "model": "nope",
    }));
    let legacy_id = match &out[0] {
        Outbound::Legacy(value) => value["id"].as_u64().unwrap(),
        _ => unreachable!(),
    };
    let frames = state.legacy_event_to_api(&json!({
        "type": "model_changed", "id": legacy_id,
        "model": "nope", "error": "unknown model",
    }));
    match &frames[..] {
        [frame] => {
            assert_eq!(frame.reply_to, Some(4));
            match &frame.event {
                ApiEvent::Error { code, message } => {
                    assert_eq!(*code, ErrorCode::InvalidRequest);
                    assert_eq!(message, "unknown model");
                }
                other => panic!("unexpected: {other:?}"),
            }
        }
        other => panic!("expected one error reply, got {other:?}"),
    }
    assert_eq!(
        state.current_model, None,
        "a failed switch must not be cached"
    );
}

#[test]
fn an_empty_model_is_refused_locally() {
    let mut state = state_with_session();
    let out = state.api_request_to_legacy(&json!({"id": 4, "req": "set_model", "model": ""}));
    match &out[..] {
        [Outbound::Reply(frame)] => {
            assert!(matches!(frame.event, ApiEvent::Error { .. }));
        }
        other => panic!("expected a local rejection, got {other:?}"),
    }
}

#[test]
fn reasoning_effort_reports_provider_refusal() {
    let mut state = state_with_session();
    let out = state.api_request_to_legacy(&json!({
        "id": 5, "req": "set_reasoning_effort", "effort": "max",
    }));
    let legacy_id = match &out[0] {
        Outbound::Legacy(value) => {
            assert_eq!(value["effort"], "max");
            value["id"].as_u64().unwrap()
        }
        _ => unreachable!(),
    };
    let frames = state.legacy_event_to_api(&json!({
        "type": "reasoning_effort_changed", "id": legacy_id,
        "error": "provider does not support reasoning effort",
    }));
    assert_eq!(frames[0].reply_to, Some(5));
    assert!(matches!(frames[0].event, ApiEvent::Error { .. }));
}

/// An effort change is identity, like a model change: every attached client
/// needs to hear it, not only the requester. A change made by another client
/// (no pending request here) must still arrive as a `model_info` broadcast,
/// and the requester's own change gets the broadcast after its `Ok`.
#[test]
fn reasoning_effort_changes_are_broadcast_as_model_info() {
    let mut state = state_with_session();

    // Unsolicited change (another client's request id): broadcast only.
    let frames = state.legacy_event_to_api(&json!({
        "type": "reasoning_effort_changed", "id": 999, "effort": "high",
    }));
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].reply_to, None);
    match &frames[0].event {
        ApiEvent::ModelInfo {
            reasoning_effort, ..
        } => assert_eq!(reasoning_effort.as_deref(), Some("high")),
        other => panic!("expected model_info, got {other:?}"),
    }

    // The same effort again is not news: no broadcast.
    let frames = state.legacy_event_to_api(&json!({
        "type": "reasoning_effort_changed", "id": 999, "effort": "high",
    }));
    assert!(frames.is_empty(), "unchanged effort must not re-broadcast");

    // This client's own change: Ok reply first, then the broadcast.
    let out = state.api_request_to_legacy(&json!({
        "id": 7, "req": "set_reasoning_effort", "effort": "low",
    }));
    let legacy_id = match &out[0] {
        Outbound::Legacy(value) => value["id"].as_u64().unwrap(),
        _ => unreachable!(),
    };
    let frames = state.legacy_event_to_api(&json!({
        "type": "reasoning_effort_changed", "id": legacy_id, "effort": "low",
    }));
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0].reply_to, Some(7));
    assert!(matches!(frames[0].event, ApiEvent::Ok));
    assert!(matches!(
        &frames[1].event,
        ApiEvent::ModelInfo { reasoning_effort, .. }
            if reasoning_effort.as_deref() == Some("low")
    ));
}

/// Compaction can be refused (nothing to compact, a turn in flight) and the
/// daemon says so with `success: false`, not an error frame. Telling the
/// client "done" would claim work that never happened.
#[test]
fn a_refused_compaction_is_an_error_not_a_success() {
    let mut state = state_with_session();
    let out = state.api_request_to_legacy(&json!({"id": 6, "req": "compact"}));
    let legacy_id = match &out[0] {
        Outbound::Legacy(value) => value["id"].as_u64().unwrap(),
        _ => unreachable!(),
    };
    let frames = state.legacy_event_to_api(&json!({
        "type": "compact_result", "id": legacy_id,
        "message": "nothing to compact", "success": false,
    }));
    match &frames[0].event {
        ApiEvent::Error { message, .. } => assert_eq!(message, "nothing to compact"),
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn a_scheduled_compaction_reports_its_status() {
    let mut state = state_with_session();
    let out = state.api_request_to_legacy(&json!({"id": 6, "req": "compact"}));
    let legacy_id = match &out[0] {
        Outbound::Legacy(value) => value["id"].as_u64().unwrap(),
        _ => unreachable!(),
    };
    let frames = state.legacy_event_to_api(&json!({
        "type": "compact_result", "id": legacy_id,
        "message": "compacting in the background", "success": true,
    }));
    match &frames[0].event {
        ApiEvent::Compacted { message, .. } => assert_eq!(message, "compacting in the background"),
        other => panic!("unexpected: {other:?}"),
    }
}

/// Clearing a title is distinct from setting an empty one, so an absent title
/// must not be sent as `""`, which the daemon would store as a real title.
#[test]
fn renaming_distinguishes_clearing_from_setting() {
    let mut state = state_with_session();
    let set = state.api_request_to_legacy(&json!({
        "id": 7, "req": "rename_session", "title": "my session",
    }));
    match &set[0] {
        Outbound::Legacy(value) => assert_eq!(value["title"], "my session"),
        other => panic!("unexpected: {other:?}"),
    }

    let clear = state.api_request_to_legacy(&json!({"id": 8, "req": "rename_session"}));
    match &clear[0] {
        Outbound::Legacy(value) => assert!(
            value.get("title").is_none(),
            "a cleared title must be absent, not empty: {value}"
        ),
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn a_rename_push_becomes_a_typed_event() {
    let mut state = state_with_session();
    let frames = state.legacy_event_to_api(&json!({
        "type": "session_renamed", "session_id": "s1",
        "title": "my session", "display_title": "my session",
    }));
    match &frames[0].event {
        ApiEvent::SessionRenamed {
            session_id,
            title,
            display_title,
        } => {
            assert_eq!(session_id, "s1");
            assert_eq!(title.as_deref(), Some("my session"));
            assert_eq!(display_title, "my session");
        }
        other => panic!("unexpected: {other:?}"),
    }
}

/// Every capability request is stateful, so none may be forwarded before the
/// connection is attached: the daemon closes the connection on those.
#[test]
fn capability_requests_need_an_attached_session() {
    for (req, extra) in [
        ("list_models", json!({})),
        ("set_model", json!({"model": "x"})),
        ("set_reasoning_effort", json!({"effort": "high"})),
        ("compact", json!({})),
        ("rename_session", json!({})),
        ("rewind_undo", json!({})),
        ("cancel_soft_interrupts", json!({})),
    ] {
        let mut state = BridgeState::default();
        let mut request = json!({"id": 1, "req": req});
        for (key, value) in extra.as_object().unwrap() {
            request[key] = value.clone();
        }
        let out = state.api_request_to_legacy(&request);
        match &out[..] {
            [Outbound::Reply(frame)] => match &frame.event {
                ApiEvent::Error { code, .. } => assert_eq!(
                    *code,
                    ErrorCode::UnknownSession,
                    "{req} should report an unattached session"
                ),
                other => panic!("{req}: unexpected {other:?}"),
            },
            other => panic!("{req} reached the daemon unattached: {other:?}"),
        }
    }
}

#[test]
fn another_sessions_broadcast_does_not_replace_the_attachment() {
    let mut state = BridgeState::default();
    let attach = state.api_request_to_legacy(&json!({
        "id": 7,
        "req": "attach_session",
        "session_id": "session_retriever_1_a",
    }));
    let state_id = match &attach[1] {
        Outbound::Legacy(value) => value["id"].as_u64().expect("state request id"),
        other => panic!("unexpected attach output: {other:?}"),
    };
    state.legacy_event_to_api(&json!({
        "type": "state",
        "id": state_id,
        "session_id": "session_retriever_1_a",
    }));

    state.legacy_event_to_api(&json!({
        "type": "session",
        "session_id": "session_pawprint_2_b",
    }));

    assert!(matches!(
        state
            .api_request_to_legacy(&json!({
                "id": 8,
                "req": "send_message",
                "session_id": "session_retriever_1_a",
                "content": "still routed to the attached session",
            }))
            .as_slice(),
        [Outbound::Legacy(_)]
    ));
}

#[test]
fn another_sessions_state_does_not_replace_the_attachment() {
    let mut state = BridgeState::default();
    let attach = state.api_request_to_legacy(&json!({
        "id": 7,
        "req": "attach_session",
        "session_id": "session_retriever_1_a",
    }));
    let state_id = match &attach[1] {
        Outbound::Legacy(value) => value["id"].as_u64().expect("state request id"),
        other => panic!("unexpected attach output: {other:?}"),
    };
    state.legacy_event_to_api(&json!({
        "type": "state",
        "id": state_id,
        "session_id": "session_retriever_1_a",
    }));

    state.legacy_event_to_api(&json!({
        "type": "state",
        "id": state_id + 100,
        "session_id": "session_pawprint_2_b",
    }));

    assert!(matches!(
        state
            .api_request_to_legacy(&json!({
                "id": 8,
                "req": "send_message",
                "session_id": "session_retriever_1_a",
                "content": "still routed to the attached session",
            }))
            .as_slice(),
        [Outbound::Legacy(_)]
    ));
}

#[test]
fn legacy_request_ids_are_unique_across_bridge_connections() {
    let mut first = BridgeState::default();
    let mut second = BridgeState::default();
    let first_attach = first.api_request_to_legacy(&json!({
        "id": 1,
        "req": "attach_session",
        "session_id": "session_first",
    }));
    let second_attach = second.api_request_to_legacy(&json!({
        "id": 1,
        "req": "attach_session",
        "session_id": "session_second",
    }));
    let request_id = |outbound: &[Outbound]| match &outbound[1] {
        Outbound::Legacy(value) => value["id"].as_u64().expect("state request id"),
        other => panic!("unexpected attach output: {other:?}"),
    };

    assert_ne!(request_id(&first_attach), request_id(&second_attach));
}

#[test]
fn colliding_state_id_for_another_target_does_not_complete_attach() {
    let mut state = BridgeState::default();
    let attach = state.api_request_to_legacy(&json!({
        "id": 7,
        "req": "attach_session",
        "session_id": "session_wanted",
    }));
    let state_id = match &attach[1] {
        Outbound::Legacy(value) => value["id"].as_u64().expect("state request id"),
        other => panic!("unexpected attach output: {other:?}"),
    };

    assert!(
        state
            .legacy_event_to_api(&json!({
                "type": "state",
                "id": state_id,
                "session_id": "session_other",
            }))
            .is_empty()
    );
    assert!(state.session_id.is_none());
}

/// A session id becomes a filesystem path, so it must be treated as untrusted.
///
/// The id arrives straight off the wire and is interpolated into
/// `<home>/sessions/<id>.json`. Without validation, a traversal id is a
/// readable path, and `peek_session` returns whatever it finds there.
#[test]
fn a_session_id_cannot_escape_the_sessions_directory() {
    for hostile in [
        "../../../etc/passwd",
        "../.ssh/id_rsa",
        "a/b",
        "a\\b",
        "..",
        "",
        "with space",
        "semi;colon",
    ] {
        assert!(
            BridgeState::session_record_path(hostile).is_none(),
            "`{hostile}` must not resolve to a session record path"
        );
    }
}

#[test]
fn a_plain_session_id_still_resolves() {
    let path = BridgeState::session_record_path("session_otter_1785728596263_80eb5ad6012a1864")
        .expect("a normal session id must resolve");
    assert!(path.ends_with("session_otter_1785728596263_80eb5ad6012a1864.json"));
    assert!(
        path.parent().is_some_and(|dir| dir.ends_with("sessions")),
        "records live in the sessions directory: {}",
        path.display()
    );
}

/// Session records must be read from the *instance's* home, not the user's.
///
/// `launch()` gives an embedded instance its own `JCODE_HOME` precisely so it
/// cannot see the user's work. Reading the user's home directly made
/// `peek_session` return the real transcripts of the jcode the user runs
/// interactively, from a client that was supposed to be sandboxed.
#[test]
fn session_records_are_read_from_the_instance_home() {
    let home = ScopedJcodeHome::new("instance-home");
    let path = BridgeState::session_record_path("session_x_1_a");
    let path = path.expect("a normal session id must resolve");
    assert!(
        path.starts_with(&home.path),
        "JCODE_HOME must scope session records, got {}",
        path.display()
    );
}

#[test]
fn unattached_list_sessions_discovers_all_persisted_records() {
    let home = ScopedJcodeHome::new("persisted-discovery");
    let first_root = home.path.join("first-project");
    let second_root = home.path.join("second-project");
    std::fs::create_dir_all(&first_root).unwrap();
    std::fs::create_dir_all(&second_root).unwrap();
    write_session_record_with_titles(
        &home.path,
        "persisted_one",
        &first_root,
        Some("  Generated first title  "),
        None,
    );
    write_session_record_with_titles(
        &home.path,
        "persisted_two",
        &second_root,
        Some("Generated second title"),
        Some("  Custom second title  "),
    );
    std::fs::write(home.path.join("sessions/not-a-session.txt"), "ignored").unwrap();

    let event = only_reply_event(
        BridgeState::default().api_request_to_legacy(&json!({"req": "list_sessions", "id": 1})),
    );
    let ApiEvent::Sessions { sessions } = event else {
        panic!("expected sessions reply, got {event:?}");
    };
    assert_eq!(
        sessions
            .iter()
            .map(|session| session.session_id.as_str())
            .collect::<Vec<_>>(),
        ["persisted_one", "persisted_two"]
    );
    assert_eq!(sessions[0].working_dir.as_deref(), first_root.to_str());
    assert_eq!(sessions[1].working_dir.as_deref(), second_root.to_str());
    assert_eq!(sessions[0].title.as_deref(), Some("Generated first title"));
    assert_eq!(sessions[1].title.as_deref(), Some("Custom second title"));
}

#[test]
fn limited_session_list_reads_compact_index_without_transcript_records() {
    let home = ScopedJcodeHome::new("metadata-index");
    assert!(BridgeState::recent_session_index_entries().is_empty());
    let mut connection = Connection::open(home.path.join("session-metadata-v1.sqlite3")).unwrap();
    let transaction = connection.transaction().unwrap();
    for index in 0..100 {
        transaction
            .execute(
                "INSERT INTO recent_sessions (
                     session_id, working_dir, todo_title, saved, updated_at_ms, last_active_at_ms
                 ) VALUES (?1, '/indexed/project', ?2, ?4, ?3, ?3)",
                params![
                    format!("indexed_{index:03}"),
                    format!("Indexed goal {index}"),
                    index,
                    index == 99,
                ],
            )
            .unwrap();
    }
    transaction.commit().unwrap();

    let event = only_reply_event(
        BridgeState::default()
            .api_request_to_legacy(&json!({"req": "list_sessions", "id": 1, "limit": 100})),
    );
    let ApiEvent::Sessions { sessions } = event else {
        panic!("expected sessions reply, got {event:?}");
    };
    assert_eq!(sessions.len(), 100);
    let newest = sessions
        .iter()
        .find(|session| session.session_id == "indexed_099")
        .expect("indexed newest session");
    assert!(newest.saved);
    assert_eq!(newest.updated_at_ms, Some(99));
    assert_eq!(newest.last_active_at_ms, Some(99));
    assert!(sessions.iter().all(|session| {
        session
            .title
            .as_deref()
            .is_some_and(|title| title.starts_with("Indexed goal "))
    }));
}

#[test]
fn runtime_info_reports_the_active_provider_and_complete_route_catalog() {
    let mut state = state_with_session();
    state.legacy_event_to_api(&json!({
        "type": "available_models_updated",
        "provider_name": "anthropic",
        "provider_model": "claude-sonnet",
        "reasoning_effort": "high",
        "available_models": ["claude-sonnet", "gemini-pro"],
        "available_model_routes": [
            {
                "model": "claude-sonnet",
                "provider": "anthropic",
                "api_method": "messages",
                "available": true,
                "detail": "ready"
            },
            {
                "model": "gemini-pro",
                "provider": "gemini",
                "api_method": "generateContent",
                "available": false,
                "detail": "credential missing"
            }
        ]
    }));

    let event = only_reply_event(state.api_request_to_legacy(&json!({
        "req": "get_runtime_info",
        "id": 4,
        "session_id": "s1"
    })));
    let ApiEvent::RuntimeInfo {
        session_id,
        provider,
        model,
        reasoning_effort,
        routes,
    } = event
    else {
        panic!("expected runtime info, got {event:?}");
    };
    assert_eq!(session_id, "s1");
    assert_eq!(provider.as_deref(), Some("anthropic"));
    assert_eq!(model.as_deref(), Some("claude-sonnet"));
    assert_eq!(reasoning_effort.as_deref(), Some("high"));
    assert_eq!(routes.len(), 2);
    assert_eq!(routes[1].provider, "gemini");
    assert!(!routes[1].available);
}

#[test]
fn archive_restore_and_retention_are_reversible_and_owner_only() {
    let home = ScopedJcodeHome::new("archive");
    let root = home.path.join("project");
    std::fs::create_dir_all(&root).unwrap();
    write_session_record(&home.path, "recent_session", &root);
    let old_record = write_session_record(&home.path, "old_session", &root);
    let old_time = SystemTime::now() - std::time::Duration::from_secs(3 * 86_400);
    std::fs::File::options()
        .write(true)
        .open(&old_record)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(old_time))
        .unwrap();

    let mut state = BridgeState::default();
    assert!(matches!(
        only_reply_event(state.api_request_to_legacy(&json!({
            "req": "archive_session",
            "id": 1,
            "session_id": "recent_session"
        }))),
        ApiEvent::Ok
    ));
    let ApiEvent::Sessions { sessions } =
        only_reply_event(state.api_request_to_legacy(&json!({"req": "list_sessions", "id": 2})))
    else {
        panic!("expected sessions");
    };
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].session_id, "old_session");

    assert!(matches!(
        only_reply_event(state.api_request_to_legacy(&json!({
            "req": "restore_session",
            "id": 3,
            "session_id": "recent_session"
        }))),
        ApiEvent::Ok
    ));
    assert!(matches!(
        only_reply_event(state.api_request_to_legacy(&json!({
            "req": "set_retention_policy",
            "id": 4,
            "archive_after_days": 1
        }))),
        ApiEvent::Ok
    ));

    let ApiEvent::Sessions { sessions } = only_reply_event(state.api_request_to_legacy(&json!({
        "req": "list_sessions",
        "id": 5,
        "include_archived": true
    }))) else {
        panic!("expected sessions");
    };
    let old = sessions
        .iter()
        .find(|session| session.session_id == "old_session")
        .expect("old session remains restorable");
    assert_eq!(old.archived, true);
    assert!(old.archived_at_ms.is_some());
    let recent = sessions
        .iter()
        .find(|session| session.session_id == "recent_session")
        .expect("restored session is listed");
    assert!(!recent.archived);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(home.path.join("sdk-archive.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        let home_mode = std::fs::metadata(&home.path).unwrap().permissions().mode() & 0o777;
        assert_eq!(home_mode, 0o700);
    }
}

#[test]
fn credential_provisioning_normalizes_gemini_and_supports_jcode() {
    let home = ScopedJcodeHome::new("credentials");
    let config = home.path.join("config/jcode");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("gemini.env"),
        "GOOGLE_API_KEY=stale\nKEEP_ME=yes\n",
    )
    .unwrap();
    let mut state = BridgeState::default();

    let outbound = state.api_request_to_legacy(&json!({
        "req": "set_api_key",
        "id": 7,
        "provider": "google-gemini",
        "api_key": "gemini-secret"
    }));
    let [Outbound::Legacy(notify)] = outbound.as_slice() else {
        panic!("credential change should notify the daemon: {outbound:?}");
    };
    assert_eq!(notify["provider"], "gemini");
    let legacy_id = notify["id"].as_u64().unwrap();
    let frames = state.legacy_event_to_api(&json!({"type": "ack", "id": legacy_id}));
    assert!(matches!(
        &frames[0].event,
        ApiEvent::CredentialUpdated { provider, configured }
            if provider == "gemini" && *configured
    ));
    let gemini = std::fs::read_to_string(config.join("gemini.env")).unwrap();
    assert!(gemini.contains("GEMINI_API_KEY=gemini-secret\n"));
    assert!(gemini.contains("KEEP_ME=yes\n"));
    assert!(!gemini.contains("GOOGLE_API_KEY"));

    let outbound = state.api_request_to_legacy(&json!({
        "req": "set_api_key",
        "id": 8,
        "provider": "subscription",
        "api_key": "jcode-secret"
    }));
    let [Outbound::Legacy(notify)] = outbound.as_slice() else {
        panic!("jcode credential should notify the daemon: {outbound:?}");
    };
    assert_eq!(notify["provider"], "jcode");
    assert_eq!(
        std::fs::read_to_string(config.join("jcode-subscription.env")).unwrap(),
        "JCODE_API_KEY=jcode-secret\n"
    );

    let event = only_reply_event(state.api_request_to_legacy(&json!({
        "req": "set_api_key",
        "id": 9,
        "provider": "gemini",
        "api_key": "line one\nline two"
    })));
    assert!(matches!(
        event,
        ApiEvent::Error {
            code: ErrorCode::InvalidRequest,
            ..
        }
    ));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&config).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(config.join("gemini.env"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[cfg(unix)]
#[test]
fn owner_only_writes_refuse_symlink_targets_and_directories() {
    use std::os::unix::fs::symlink;

    let home = ScopedJcodeHome::new("credential-symlinks");
    let outside_file = home.path.join("outside.env");
    std::fs::write(&outside_file, "unchanged\n").unwrap();
    let config = home.path.join("config/jcode");
    std::fs::create_dir_all(&config).unwrap();
    symlink(&outside_file, config.join("gemini.env")).unwrap();
    let mut state = BridgeState::default();
    let event = only_reply_event(state.api_request_to_legacy(&json!({
        "req": "set_api_key",
        "id": 1,
        "provider": "gemini",
        "api_key": "must-not-land"
    })));
    assert!(matches!(
        event,
        ApiEvent::Error {
            code: ErrorCode::Internal,
            ..
        }
    ));
    assert_eq!(
        std::fs::read_to_string(&outside_file).unwrap(),
        "unchanged\n"
    );

    std::fs::remove_file(config.join("gemini.env")).unwrap();
    std::fs::remove_dir(&config).unwrap();
    let outside_dir = home.path.join("outside-config");
    std::fs::create_dir_all(&outside_dir).unwrap();
    symlink(&outside_dir, &config).unwrap();
    let event = only_reply_event(BridgeState::default().api_request_to_legacy(&json!({
        "req": "set_api_key",
        "id": 2,
        "provider": "jcode",
        "api_key": "must-not-land"
    })));
    assert!(matches!(
        event,
        ApiEvent::Error {
            code: ErrorCode::Internal,
            ..
        }
    ));
    assert!(!outside_dir.join("jcode-subscription.env").exists());
}

#[cfg(unix)]
#[test]
fn rooted_file_operations_reject_traversal_and_symlink_escapes_and_bound_results() {
    use std::os::unix::fs::symlink;

    let home = ScopedJcodeHome::new("rooted-files");
    let root = home.path.join("project");
    let outside = home.path.join("outside");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(root.join("src/unicode.txt"), "éx secret\n").unwrap();
    for index in 0..8 {
        std::fs::write(root.join(format!("src/match-{index}.txt")), "needle\n").unwrap();
    }
    std::fs::write(outside.join("outside-secret.txt"), "outside needle\n").unwrap();
    symlink(&outside, root.join("escape")).unwrap();
    write_session_record(&home.path, "s1", &root);
    let mut state = state_with_session();

    for hostile in ["../outside/outside-secret.txt", "escape/outside-secret.txt"] {
        let event = only_reply_event(state.api_request_to_legacy(&json!({
            "req": "read_file",
            "id": 1,
            "session_id": "s1",
            "path": hostile
        })));
        assert!(matches!(
            event,
            ApiEvent::Error {
                code: ErrorCode::InvalidRequest,
                ..
            }
        ));
    }

    let event = only_reply_event(state.api_request_to_legacy(&json!({
        "req": "read_file",
        "id": 2,
        "session_id": "s1",
        "path": "src/unicode.txt",
        "max_bytes": 2
    })));
    assert!(matches!(
        event,
        ApiEvent::FileContent {
            content,
            truncated: true,
            ..
        } if content == "é"
    ));

    let event = only_reply_event(state.api_request_to_legacy(&json!({
        "req": "find_files",
        "id": 3,
        "session_id": "s1",
        "query": "outside-secret",
        "limit": 1000000
    })));
    assert!(matches!(event, ApiEvent::Files { paths, .. } if paths.is_empty()));

    let event = only_reply_event(state.api_request_to_legacy(&json!({
        "req": "search_text",
        "id": 4,
        "session_id": "s1",
        "query": "needle",
        "limit": 3
    })));
    let ApiEvent::TextMatches { matches, .. } = event else {
        panic!("expected bounded text matches, got {event:?}");
    };
    assert_eq!(matches.len(), 3);
    assert!(
        matches
            .iter()
            .all(|found| !found.path.starts_with("escape/"))
    );

    let event = only_reply_event(state.api_request_to_legacy(&json!({
        "req": "file_status",
        "id": 5,
        "session_id": "s1",
        "path": "src/missing.txt"
    })));
    assert!(matches!(
        event,
        ApiEvent::FileStatus {
            exists: false,
            ref kind,
            ..
        } if kind == "missing"
    ));
}
