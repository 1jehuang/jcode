//! Wrapper-level tests for the exported `ask_form` functions.
//!
//! These exercise the exact signatures lane C calls, using a real `App` built
//! by the test harness. Most behaviour is covered through the internal core in
//! `tests/mod.rs`; here we only prove the plumbing and session gating.

use std::sync::Arc;

use super::*;
use crate::protocol::ServerEvent;
use crate::provider::Provider;
use crate::tui::TuiState;
use crate::tui::app::App;
use crate::tui::ask_form::{
    KeyOutcome, handle_key, handle_paste, on_connection_replaced, on_event, on_remote_loop_exit,
    on_remote_loop_start, on_replay_complete, record_response_id, send_failed,
};
use anyhow::Result;

struct MockProvider;

#[async_trait::async_trait]
impl Provider for MockProvider {
    async fn complete(
        &self,
        _messages: &[crate::message::Message],
        _tools: &[crate::message::ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        Err(anyhow::anyhow!("mock provider"))
    }

    fn name(&self) -> &str {
        "mock"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self)
    }
}

fn ensure_test_jcode_home() {
    use std::sync::OnceLock;
    static TEST_HOME: OnceLock<std::path::PathBuf> = OnceLock::new();
    if std::env::var_os("JCODE_HOME").is_some() {
        return;
    }
    let path = TEST_HOME.get_or_init(|| {
        let path = std::env::temp_dir().join(format!("jcode-ask-form-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&path);
        path
    });
    crate::env::set_var("JCODE_HOME", path);
}

fn test_app() -> App {
    ensure_test_jcode_home();
    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");
    let registry = runtime.block_on(crate::tool::Registry::new(provider.clone()));
    let mut app = App::new_for_test_harness(provider, registry);
    // Keep the onboarding welcome from outranking the form on hosts with no
    // stored credentials.
    app.push_display_message(crate::tui::DisplayMessage::system("test"));
    app
}

fn session_of(app: &App) -> String {
    app.current_session_id().expect("test session")
}

#[test]
fn public_key_and_event_plumbing_sends_and_records_the_request_id() {
    reset();
    let mut app = test_app();
    let session = session_of(&app);
    on_remote_loop_start(&mut app);

    let request = form("e1", &session, single_schema());
    let event = ServerEvent::ElicitationRequest(request);
    assert!(on_event(&mut app, &event));
    assert_eq!(form_count(), 1);

    // Digit confirms, Enter advances to Submit, Enter sends.
    assert_eq!(
        handle_key(&mut app, KeyCode::Char('1'), KeyModifiers::NONE, None),
        KeyOutcome::Consumed
    );
    assert_eq!(
        handle_key(&mut app, KeyCode::Enter, KeyModifiers::NONE, None),
        KeyOutcome::Consumed
    );
    match handle_key(&mut app, KeyCode::Enter, KeyModifiers::NONE, None) {
        KeyOutcome::Send {
            session_id,
            elicitation_id,
            response,
        } => {
            assert_eq!(session_id, session);
            assert_eq!(elicitation_id, "e1");
            assert!(matches!(
                response,
                crate::protocol::elicitation::ElicitationResponse::Accept { .. }
            ));
        }
        other => panic!("expected send, got {other:?}"),
    }

    record_response_id(&mut app, 77);
    assert_eq!(response_ids(), vec![77]);
    assert!(on_event(&mut app, &ServerEvent::Done { id: 77 }));
    assert!(response_ids().is_empty());
}

#[test]
fn public_paste_render_and_reconnect_hooks() {
    reset();
    let mut app = test_app();
    let session = session_of(&app);
    on_remote_loop_start(&mut app);
    assert!(on_event(
        &mut app,
        &ServerEvent::ElicitationRequest(form("e1", &session, schema_json()))
    ));

    // Open notes and paste into the focused editor.
    assert_eq!(
        handle_key(&mut app, KeyCode::Char('n'), KeyModifiers::NONE, None),
        KeyOutcome::Consumed
    );
    assert!(handle_paste(&mut app, "note text"));
    active(|form| assert_eq!(form.questions[0].notes, "note text"));

    // Render the real App through the exported `render` signature.
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| crate::tui::ask_form::render(frame, &app))
        .expect("render");
    let buffer = terminal.backend().buffer().clone();
    assert!(first_nonblank_row(&buffer).is_some());

    // Freeze, replay and complete.
    on_connection_replaced(&mut app);
    assert!(on_event(
        &mut app,
        &ServerEvent::ElicitationRequest(form("e1", &session, schema_json()))
    ));
    on_replay_complete(&mut app, &session);
    assert_eq!(form_count(), 1);

    on_remote_loop_exit();
    assert_eq!(form_count(), 0);
}

#[test]
fn public_send_failure_restores_editing() {
    reset();
    let mut app = test_app();
    let session = session_of(&app);
    on_remote_loop_start(&mut app);
    assert!(on_event(
        &mut app,
        &ServerEvent::ElicitationRequest(form("e1", &session, single_schema()))
    ));
    assert_eq!(
        handle_key(&mut app, KeyCode::Char('1'), KeyModifiers::NONE, None),
        KeyOutcome::Consumed
    );
    assert_eq!(
        handle_key(&mut app, KeyCode::Enter, KeyModifiers::NONE, None),
        KeyOutcome::Consumed
    );
    assert!(matches!(
        handle_key(&mut app, KeyCode::Enter, KeyModifiers::NONE, None),
        KeyOutcome::Send { .. }
    ));
    send_failed(&mut app, "offline");
    active(|form| assert!(matches!(form.submit, SubmitState::Rejected(_))));
}
