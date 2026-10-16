use crate::tui::app::PendingDecision;
use crossterm::event::{KeyCode, KeyModifiers};

struct DecisionMockProvider;

#[async_trait::async_trait]
impl crate::provider::Provider for DecisionMockProvider {
    async fn complete(
        &self,
        _messages: &[crate::message::Message],
        _tools: &[crate::message::ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> anyhow::Result<crate::provider::EventStream> {
        Err(anyhow::anyhow!(
            "mock provider should never stream in decision tests"
        ))
    }

    fn name(&self) -> &str {
        "mock"
    }

    fn fork(&self) -> std::sync::Arc<dyn crate::provider::Provider> {
        std::sync::Arc::new(Self)
    }
}

fn create_test_app_for_decision() -> crate::tui::app::App {
    let provider: std::sync::Arc<dyn crate::provider::Provider> =
        std::sync::Arc::new(DecisionMockProvider);
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let registry = rt.block_on(crate::tool::Registry::new(provider.clone()));
    crate::tui::app::App::new_for_test_harness(provider, registry)
}

fn make_decision() -> PendingDecision {
    PendingDecision {
        request_id: "decision-call-1".to_string(),
        question: "Deploy now or stage first?".to_string(),
        options: vec![
            ("Deploy".to_string(), None),
            ("Stage".to_string(), Some("stage first".to_string())),
        ],
        selected: 0,
        answer_draft: String::new(),
    }
}

/// Render the full TUI frame with the test backend and return its text.
fn render_frame_text(app: &crate::tui::app::App, width: u16, height: u16) -> String {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| crate::tui::ui::draw(f, app))
        .expect("frame draw should succeed");
    let buf = terminal.backend().buffer();
    let mut lines = Vec::with_capacity(height as usize);
    for y in 0..height {
        let mut line = String::new();
        for x in 0..width {
            line.push_str(buf[(x, y)].symbol());
        }
        lines.push(line.trim_end().to_string());
    }
    lines.join("\n")
}

/// Read one newline-delimited JSON request off the dummy peer.
async fn read_peer_request(peer: crate::transport::Stream) -> crate::protocol::Request {
    let (reader, _writer) = peer.into_split();
    let mut reader = tokio::io::BufReader::new(reader);
    let mut line = String::new();
    tokio::io::AsyncBufReadExt::read_line(&mut reader, &mut line)
        .await
        .expect("peer should receive a request line");
    serde_json::from_str(&line).expect("peer request should deserialize")
}

#[test]
fn esc_dismissal_sends_dismissed_choice() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app_for_decision();
    rt.block_on(async {
        app.pending_decision = Some(make_decision());
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        let peer = remote
            .take_dummy_peer()
            .expect("dummy remote should retain peer stream");

        let consumed = app
            .handle_decision_key(KeyCode::Esc, KeyModifiers::NONE, &mut remote)
            .await;
        assert!(consumed, "Esc should be consumed by the chooser");
        assert!(app.pending_decision.is_none(), "chooser should clear");

        let request = read_peer_request(peer).await;
        match request {
            crate::protocol::Request::DecisionResponse {
                request_id, choice, ..
            } => {
                assert_eq!(request_id, "decision-call-1");
                assert!(matches!(choice, crate::protocol::DecisionChoice::Dismissed));
            }
            other => panic!("expected DecisionResponse, got {:?}", other),
        }
    });
}

#[test]
fn enter_on_option_confirms_highlighted_option() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app_for_decision();
    rt.block_on(async {
        let mut decision = make_decision();
        decision.selected = 1;
        app.pending_decision = Some(decision);
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        let peer = remote.take_dummy_peer().expect("peer stream");

        let consumed = app
            .handle_decision_key(KeyCode::Enter, KeyModifiers::NONE, &mut remote)
            .await;
        assert!(consumed);

        let request = read_peer_request(peer).await;
        match request {
            crate::protocol::Request::DecisionResponse { choice, .. } => {
                assert!(matches!(
                    choice,
                    crate::protocol::DecisionChoice::Option { index: 2 }
                ));
            }
            other => panic!("expected DecisionResponse, got {:?}", other),
        }
    });
}

#[test]
fn digit_key_picks_option_directly() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app_for_decision();
    rt.block_on(async {
        app.pending_decision = Some(make_decision());
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        let peer = remote.take_dummy_peer().expect("peer stream");

        let consumed = app
            .handle_decision_key(KeyCode::Char('2'), KeyModifiers::NONE, &mut remote)
            .await;
        assert!(consumed);

        let request = read_peer_request(peer).await;
        match request {
            crate::protocol::Request::DecisionResponse { choice, .. } => {
                assert!(matches!(
                    choice,
                    crate::protocol::DecisionChoice::Option { index: 2 }
                ));
            }
            other => panic!("expected DecisionResponse, got {:?}", other),
        }
    });
}

#[test]
fn arrows_navigate_options_and_answer_row() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app_for_decision();
    rt.block_on(async {
        app.pending_decision = Some(make_decision());
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        // 0 -> 1 -> answer row
        let consumed = app
            .handle_decision_key(KeyCode::Down, KeyModifiers::NONE, &mut remote)
            .await;
        assert!(consumed);
        assert_eq!(app.pending_decision.as_ref().unwrap().selected, 1);

        let consumed = app
            .handle_decision_key(KeyCode::Down, KeyModifiers::NONE, &mut remote)
            .await;
        assert!(consumed);
        assert!(
            app.pending_decision
                .as_ref()
                .unwrap()
                .selected_is_answer_row(),
            "down past the last option lands on the Your answer row"
        );

        // Answer row is the last row; down clamps there.
        let consumed = app
            .handle_decision_key(KeyCode::Down, KeyModifiers::NONE, &mut remote)
            .await;
        assert!(consumed);
        assert!(
            app.pending_decision
                .as_ref()
                .unwrap()
                .selected_is_answer_row()
        );

        // Up returns to the last numbered option, then 0, then clamps.
        let consumed = app
            .handle_decision_key(KeyCode::Up, KeyModifiers::NONE, &mut remote)
            .await;
        assert!(consumed);
        assert_eq!(app.pending_decision.as_ref().unwrap().selected, 1);

        let consumed = app
            .handle_decision_key(KeyCode::Up, KeyModifiers::NONE, &mut remote)
            .await;
        assert!(consumed);
        assert_eq!(app.pending_decision.as_ref().unwrap().selected, 0);

        let consumed = app
            .handle_decision_key(KeyCode::Up, KeyModifiers::NONE, &mut remote)
            .await;
        assert!(consumed);
        assert_eq!(
            app.pending_decision.as_ref().unwrap().selected,
            0,
            "up at the start must clamp"
        );
    });
}

#[test]
fn typing_on_answer_row_builds_draft_and_enter_sends_free_form() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app_for_decision();
    rt.block_on(async {
        let mut decision = make_decision();
        decision.selected = PendingDecision::ANSWER_ROW;
        app.pending_decision = Some(decision);
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        let peer = remote.take_dummy_peer().expect("peer stream");

        // Letters typed while the answer row is selected edit the draft.
        for c in "kotlin not node".chars() {
            let consumed = app
                .handle_decision_key(KeyCode::Char(c), KeyModifiers::NONE, &mut remote)
                .await;
            assert!(consumed, "char {c} should edit the answer draft");
        }
        assert_eq!(
            app.pending_decision.as_ref().unwrap().answer_draft,
            "kotlin not node"
        );
        assert!(
            app.input.is_empty(),
            "typing must never leak into the main input box"
        );

        // Enter sends the draft as a free-form answer.
        let consumed = app
            .handle_decision_key(KeyCode::Enter, KeyModifiers::NONE, &mut remote)
            .await;
        assert!(consumed);
        assert!(app.pending_decision.is_none(), "chooser should clear");

        let request = read_peer_request(peer).await;
        match request {
            crate::protocol::Request::DecisionResponse { choice, .. } => {
                assert!(matches!(
                    choice,
                    crate::protocol::DecisionChoice::Text { ref text }
                        if text == "kotlin not node"
                ));
            }
            other => panic!("expected DecisionResponse, got {:?}", other),
        }
    });
}

#[test]
fn backspace_edits_answer_draft() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app_for_decision();
    rt.block_on(async {
        let mut decision = make_decision();
        decision.selected = PendingDecision::ANSWER_ROW;
        decision.answer_draft = "kotli".to_string();
        app.pending_decision = Some(decision);
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        let consumed = app
            .handle_decision_key(KeyCode::Backspace, KeyModifiers::NONE, &mut remote)
            .await;
        assert!(consumed);
        assert_eq!(app.pending_decision.as_ref().unwrap().answer_draft, "kotl");
    });
}

#[test]
fn enter_on_empty_answer_draft_does_not_answer() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app_for_decision();
    rt.block_on(async {
        let mut decision = make_decision();
        decision.selected = PendingDecision::ANSWER_ROW;
        app.pending_decision = Some(decision);
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        let consumed = app
            .handle_decision_key(KeyCode::Enter, KeyModifiers::NONE, &mut remote)
            .await;
        assert!(consumed, "Enter is still owned by the chooser");
        assert!(
            app.pending_decision.is_some(),
            "an empty answer draft must not send anything"
        );
    });
}

#[test]
fn letters_on_option_rows_do_not_edit_the_main_input() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app_for_decision();
    rt.block_on(async {
        app.pending_decision = Some(make_decision());
        app.input.clear();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        // 'j' with an option selected falls through (consumed = false) and
        // must never land in the main input box either; the caller discards
        // unconsumed keys because the input box is disabled while the
        // chooser is visible.
        let consumed = app
            .handle_decision_key(KeyCode::Char('j'), KeyModifiers::NONE, &mut remote)
            .await;
        assert!(
            !consumed,
            "letters outside the answer row must fall through"
        );
        assert!(
            app.pending_decision.is_some(),
            "chooser must survive the fall-through"
        );
    });
}

#[test]
fn digit_beyond_options_falls_through_without_answering() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app_for_decision();
    rt.block_on(async {
        app.pending_decision = Some(make_decision());
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        let consumed = app
            .handle_decision_key(KeyCode::Char('5'), KeyModifiers::NONE, &mut remote)
            .await;
        assert!(!consumed, "5 with 2 options must fall through");
        assert!(app.pending_decision.is_some());
    });
}

#[test]
fn chooser_renders_your_answer_row_and_locked_input() {
    let mut app = create_test_app_for_decision();
    app.pending_decision = Some(make_decision());
    app.input = "stale draft that must not show".to_string();
    app.cursor_pos = app.input.len();

    let frame = render_frame_text(&app, 100, 30);
    assert!(
        frame.contains("Deploy now or stage first?"),
        "question missing from frame:\n{}",
        frame
    );
    assert!(frame.contains("1. Deploy"));
    assert!(frame.contains("2. Stage"));
    assert!(frame.contains("3. Your answer"));

    // The main input box is disabled: no stale composer text leaks through
    // and the locked hint renders instead.
    assert!(
        !frame.contains("stale draft"),
        "disabled input box must not render the stale draft"
    );
    assert!(frame.contains("input locked while a decision is pending"));
}

#[test]
fn chooser_renders_answer_draft_when_answer_row_selected() {
    let mut app = create_test_app_for_decision();
    let mut decision = make_decision();
    decision.selected = PendingDecision::ANSWER_ROW;
    decision.answer_draft = "kotlin not node".to_string();
    app.pending_decision = Some(decision);

    let frame = render_frame_text(&app, 100, 30);

    assert!(
        frame.contains("3. Your answer: kotlin not node"),
        "the answer draft must render in the chooser, got: {}",
        frame
    );
}

#[test]
fn remote_ctrl_c_interrupts_even_with_pending_decision() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app_for_decision();
    rt.block_on(async {
        app.pending_decision = Some(make_decision());
        app.is_processing = false;
        app.input.clear();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        // Ctrl+C must fall through the chooser gate so the user can always
        // interrupt the blocked turn (here: request quit while idle).
        crate::tui::app::remote::handle_remote_key(
            &mut app,
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
            &mut remote,
        )
        .await
        .expect("ctrl+c should be handled");
        assert!(
            app.quit_pending.is_some(),
            "Ctrl+C must reach the quit/interrupt path while the chooser is visible"
        );
        assert!(
            app.pending_decision.is_some(),
            "the chooser itself must survive Ctrl+C"
        );
    });
}

#[test]
fn remote_printable_keys_never_reach_the_main_input_during_decision() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app_for_decision();
    rt.block_on(async {
        app.pending_decision = Some(make_decision());
        app.input.clear();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        crate::tui::app::remote::handle_remote_key(
            &mut app,
            KeyCode::Char('x'),
            KeyModifiers::NONE,
            &mut remote,
        )
        .await
        .expect("plain char should be handled");
        assert!(
            app.input.is_empty(),
            "the disabled main input box must not receive typing"
        );
        assert!(app.pending_decision.is_some());
    });
}

#[test]
fn digits_on_answer_row_type_into_the_draft_not_pick_options() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app_for_decision();
    rt.block_on(async {
        let mut decision = make_decision();
        decision.selected = PendingDecision::ANSWER_ROW;
        app.pending_decision = Some(decision);
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        // Typing "2pm" on the answer row must not pick option 2.
        for c in "2pm".chars() {
            app.handle_decision_key(KeyCode::Char(c), KeyModifiers::NONE, &mut remote)
                .await;
        }
        let decision = app.pending_decision.as_ref().unwrap();
        assert_eq!(decision.answer_draft, "2pm");
        assert!(decision.selected_is_answer_row());
    });
}

#[test]
fn unicode_chars_edit_the_answer_draft_by_codepoint() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app_for_decision();
    rt.block_on(async {
        let mut decision = make_decision();
        decision.selected = PendingDecision::ANSWER_ROW;
        app.pending_decision = Some(decision);
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        for c in "cámara ño".chars() {
            app.handle_decision_key(KeyCode::Char(c), KeyModifiers::NONE, &mut remote)
                .await;
        }
        assert_eq!(
            app.pending_decision.as_ref().unwrap().answer_draft,
            "cámara ño"
        );

        // Backspace pops one codepoint, not one byte.
        app.handle_decision_key(KeyCode::Backspace, KeyModifiers::NONE, &mut remote)
            .await;
        assert_eq!(
            app.pending_decision.as_ref().unwrap().answer_draft,
            "cámara ñ"
        );
    });
}

#[test]
fn whitespace_only_answer_draft_does_not_answer() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app_for_decision();
    rt.block_on(async {
        let mut decision = make_decision();
        decision.selected = PendingDecision::ANSWER_ROW;
        decision.answer_draft = "   ".to_string();
        app.pending_decision = Some(decision);
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        let consumed = app
            .handle_decision_key(KeyCode::Enter, KeyModifiers::NONE, &mut remote)
            .await;
        assert!(consumed);
        assert!(
            app.pending_decision.is_some(),
            "a whitespace-only draft must trim to empty and not answer"
        );
    });
}

#[test]
fn digit_picks_still_work_when_an_option_row_is_selected() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app_for_decision();
    rt.block_on(async {
        app.pending_decision = Some(make_decision());
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        let peer = remote.take_dummy_peer().expect("peer stream");

        let consumed = app
            .handle_decision_key(KeyCode::Char('1'), KeyModifiers::NONE, &mut remote)
            .await;
        assert!(consumed);
        assert!(app.pending_decision.is_none());

        let request = read_peer_request(peer).await;
        match request {
            crate::protocol::Request::DecisionResponse { choice, .. } => {
                assert!(matches!(
                    choice,
                    crate::protocol::DecisionChoice::Option { index: 1 }
                ));
            }
            other => panic!("expected DecisionResponse, got {:?}", other),
        }
    });
}

#[test]
fn shift_typing_uppercase_and_symbols_reaches_the_answer_draft() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app_for_decision();
    rt.block_on(async {
        let mut decision = make_decision();
        decision.selected = PendingDecision::ANSWER_ROW;
        app.pending_decision = Some(decision);
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        app.handle_decision_key(KeyCode::Char('a'), KeyModifiers::SHIFT, &mut remote)
            .await;
        app.handle_decision_key(KeyCode::Char('1'), KeyModifiers::SHIFT, &mut remote)
            .await;
        app.handle_decision_key(KeyCode::Char('?'), KeyModifiers::NONE, &mut remote)
            .await;
        assert_eq!(app.pending_decision.as_ref().unwrap().answer_draft, "A1?");
    });
}

#[test]
fn altgr_layout_symbols_reach_the_answer_draft() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app_for_decision();
    rt.block_on(async {
        let mut decision = make_decision();
        decision.selected = PendingDecision::ANSWER_ROW;
        app.pending_decision = Some(decision);
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        // AltGr characters arrive as Ctrl+Alt + the final printable symbol
        // on layouts like es-ES ('{' on the digit-1 key) or de ('@').
        app.handle_decision_key(
            KeyCode::Char('{'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
            &mut remote,
        )
        .await;
        assert_eq!(app.pending_decision.as_ref().unwrap().answer_draft, "{");
    });
}

#[test]
fn ctrl_chords_do_not_leak_into_the_answer_draft() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app_for_decision();
    rt.block_on(async {
        let mut decision = make_decision();
        decision.selected = PendingDecision::ANSWER_ROW;
        app.pending_decision = Some(decision);
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        app.handle_decision_key(KeyCode::Char('u'), KeyModifiers::CONTROL, &mut remote)
            .await;
        assert_eq!(app.pending_decision.as_ref().unwrap().answer_draft, "");
    });
}

#[test]
fn shift_backspace_deletes_from_the_answer_draft() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app_for_decision();
    rt.block_on(async {
        let mut decision = make_decision();
        decision.selected = PendingDecision::ANSWER_ROW;
        decision.answer_draft = "abc".to_string();
        app.pending_decision = Some(decision);
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        app.handle_decision_key(KeyCode::Backspace, KeyModifiers::SHIFT, &mut remote)
            .await;
        assert_eq!(app.pending_decision.as_ref().unwrap().answer_draft, "ab");
    });
}

/// Shift+Enter and Alt+Enter both insert an explicit newline in the answer
/// draft, mirroring the composer's enter_inserts_newline. Plain Enter keeps
/// submitting. The chord must be consumed even on an option row (the key
/// would otherwise leak to the disabled composer), but only edits the draft
/// when the answer row is selected.
#[test]
fn shift_or_alt_enter_inserts_newline_in_answer_draft() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app_for_decision();
    rt.block_on(async {
        let mut decision = make_decision();
        decision.selected = PendingDecision::ANSWER_ROW;
        decision.answer_draft = "first".to_string();
        app.pending_decision = Some(decision);
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        let consumed = app
            .handle_decision_key(KeyCode::Enter, KeyModifiers::SHIFT, &mut remote)
            .await;
        assert!(consumed, "Shift+Enter must be consumed by the chooser");
        let consumed = app
            .handle_decision_key(KeyCode::Enter, KeyModifiers::ALT, &mut remote)
            .await;
        assert!(consumed, "Alt+Enter must be consumed by the chooser");
        assert_eq!(
            app.pending_decision.as_ref().unwrap().answer_draft,
            "first\n\n"
        );

        // On an option row the chord is consumed but must not touch the
        // draft or send anything.
        let mut decision = make_decision();
        decision.selected = 0;
        decision.answer_draft = "untouched".to_string();
        app.pending_decision = Some(decision);
        app.handle_decision_key(KeyCode::Enter, KeyModifiers::SHIFT, &mut remote)
            .await;
        assert_eq!(
            app.pending_decision.as_ref().unwrap().answer_draft,
            "untouched"
        );
    });
}

/// A multiline draft submits as one free-form answer: the newlines the user
/// typed survive into the sent text, only trimmed at the edges.
#[test]
fn multiline_draft_submits_with_explicit_newlines() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app_for_decision();
    rt.block_on(async {
        let mut decision = make_decision();
        decision.selected = PendingDecision::ANSWER_ROW;
        decision.answer_draft = "line one\nline two\n".to_string();
        app.pending_decision = Some(decision);
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        let peer = remote.take_dummy_peer().expect("peer stream");

        let consumed = app
            .handle_decision_key(KeyCode::Enter, KeyModifiers::NONE, &mut remote)
            .await;
        assert!(consumed);

        let request = read_peer_request(peer).await;
        let text = match request {
            crate::protocol::Request::DecisionResponse { choice, .. } => match choice {
                crate::protocol::DecisionChoice::Text { text } => text,
                other => panic!("expected a free-form text answer, got {other:?}"),
            },
            other => panic!("expected DecisionResponse, got {other:?}"),
        };
        assert_eq!(text, "line one\nline two");
    });
}

#[test]
fn paste_during_decision_lands_in_answer_draft_not_main_input() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app_for_decision();
    rt.block_on(async {
        app.pending_decision = Some(make_decision());
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        // Select the answer row, then paste multiline text.
        app.handle_decision_key(KeyCode::Down, KeyModifiers::NONE, &mut remote)
            .await;
        app.handle_paste("yes, do it\nbut carefully".to_string());

        let decision = app.pending_decision.as_ref().unwrap();
        assert_eq!(decision.answer_draft, "yes, do it\nbut carefully");
        // The disabled composer must stay untouched.
        assert!(app.input.is_empty());
    });
}

#[test]
fn paste_while_an_option_row_is_selected_jumps_to_the_answer_draft() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app_for_decision();
    rt.block_on(async {
        let mut decision = make_decision();
        decision.selected = 0; // an option row, not the answer row
        app.pending_decision = Some(decision);

        app.handle_paste("deploy to prod".to_string());

        let decision = app.pending_decision.as_ref().unwrap();
        assert_eq!(decision.answer_draft, "deploy to prod");
        assert!(decision.selected_is_answer_row());
        assert!(app.input.is_empty());
    });
}

#[test]
fn paste_without_decision_unchanged_behavior() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app_for_decision();
    rt.block_on(async {
        assert!(app.pending_decision.is_none());
        app.handle_paste("hello".to_string());
        // Normal paste still lands in the composer.
        assert!(app.input.contains("hello"));
    });
}

#[test]
fn narrow_terminal_wraps_draft_without_panicking() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app_for_decision();
    rt.block_on(async {
        let mut decision = make_decision();
        decision.selected = PendingDecision::ANSWER_ROW;
        decision.answer_draft = "a very long typed answer that cannot fit".to_string();
        app.pending_decision = Some(decision);

        // 24 cols leaves a ~3-column wrap budget and the layout gives the
        // chooser only a few rows, so tail rows clip. The guarantees at this
        // width: no panic, no row wider than the terminal, and no ellipsis
        // on the answer rows (the title may still truncate). Whole-word
        // visibility is covered by the wider-terminal wrap tests.
        let frame = render_frame_text(&app, 24, 24);
        assert!(frame.contains("Your answer"));
        for line in frame.split('\n') {
            if line.chars().any(|c| char_width(c) > 1) {
                continue;
            }
            assert!(
                line.chars().count() <= 24,
                "row exceeds terminal width: {line:?}"
            );
        }
        for line in frame.split('\n') {
            if line.contains("Your answer") || line.trim_start().starts_with("     ") {
                assert!(
                    !line.contains('…'),
                    "answer row must not ellipsize: {line:?}"
                );
            }
        }
    });
}

fn char_width(c: char) -> usize {
    unicode_width::UnicodeWidthChar::width(c).unwrap_or(0)
}

#[test]
fn long_question_and_draft_render_within_bounds() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app_for_decision();
    rt.block_on(async {
        let mut decision = make_decision();
        decision.question = "This is an extremely long question header that \
                             absolutely cannot fit in the chooser width \
                             and must be truncated gracefully"
            .to_string();
        decision.selected = PendingDecision::ANSWER_ROW;
        decision.answer_draft =
            "and an equally long typed free-form answer to match it character for character"
                .to_string();
        app.pending_decision = Some(decision);

        let frame = render_frame_text(&app, 60, 14);
        assert!(frame.contains("Your answer"));
        // The title still truncates with an ellipsis; the draft wraps
        // instead, so its full text stays visible across its rows.
        assert!(frame.contains('…'), "the long title must truncate");
        for fragment in [
            "and an equally long typed free-form",
            "answer to match it character for",
            "character",
        ] {
            assert!(
                frame.contains(fragment),
                "wrapped draft keeps every word visible, missing {fragment:?}:\n{frame}"
            );
        }
    });
}

#[test]
fn chooser_title_emoji_measures_consistently_for_truncation() {
    // unicode-width 0.2 classifies U+2753 (❓) as width 2, matching how
    // terminals render emoji. draw_inline_decision sizes/truncates the
    // chooser border title with this measure, so a narrow terminal keeps
    // every row inside the width. Pin the measure so a unicode-width
    // upgrade that changes emoji classification is caught deliberately.
    assert_eq!(unicode_width::UnicodeWidthChar::width('❓'), Some(2));
}

#[test]
fn picked_option_is_recorded_with_label_and_value() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app_for_decision();
    rt.block_on(async {
        let mut decision = make_decision();
        decision.selected = 1; // Stage (stage first)
        app.pending_decision = Some(decision);
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        let peer = remote.take_dummy_peer().expect("peer stream");

        let consumed = app
            .handle_decision_key(KeyCode::Enter, KeyModifiers::NONE, &mut remote)
            .await;
        assert!(consumed);

        let _ = read_peer_request(peer).await;
        let last = app
            .display_messages()
            .last()
            .expect("answer should be recorded");
        assert_eq!(last.role, "system");
        assert!(
            last.content
                .contains("❓ Deploy now or stage first? — you picked: Stage (stage first)"),
            "got: {}",
            last.content
        );
    });
}

#[test]
fn digit_pick_records_the_digit_picked_option_not_the_highlighted_one() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app_for_decision();
    rt.block_on(async {
        // Highlight row 0, then pick option 2 with a digit key.
        app.pending_decision = Some(make_decision());
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        let peer = remote.take_dummy_peer().expect("peer stream");

        let consumed = app
            .handle_decision_key(KeyCode::Char('2'), KeyModifiers::NONE, &mut remote)
            .await;
        assert!(consumed);

        let _ = read_peer_request(peer).await;
        let last = app
            .display_messages()
            .last()
            .expect("answer should be recorded");
        assert!(
            last.content.contains("you picked: Stage (stage first)"),
            "digit pick must record the digit-picked option, got: {}",
            last.content
        );
        assert!(
            !last.content.contains("Deploy —"),
            "must not name the highlighted option instead of the picked one, got: {}",
            last.content
        );
    });
}

#[test]
fn free_form_answer_is_recorded_with_the_written_text() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app_for_decision();
    rt.block_on(async {
        let mut decision = make_decision();
        decision.selected = PendingDecision::ANSWER_ROW;
        decision.answer_draft = "why a database at all?".to_string();
        app.pending_decision = Some(decision);
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        let peer = remote.take_dummy_peer().expect("peer stream");

        let consumed = app
            .handle_decision_key(KeyCode::Enter, KeyModifiers::NONE, &mut remote)
            .await;
        assert!(consumed);

        let _ = read_peer_request(peer).await;
        let last = app
            .display_messages()
            .last()
            .expect("answer should be recorded");
        assert!(
            last.content
                .contains("❓ Deploy now or stage first? — you answered: why a database at all?"),
            "got: {}",
            last.content
        );
    });
}

#[test]
fn recorded_answer_message_survives_frame_rendering() {
    let mut app = create_test_app_for_decision();
    // Simulate the recorded message exactly as record_decision_answer emits
    // it after an option pick, then render in a 80-column frame. Single-line
    // system notices are one row by policy (upstream's
    // render_single_line_system_notice), so the full recorded answer needs
    // adequate width; the message must appear (never dropped) and every
    // rendered line must respect the frame width.
    app.push_display_message(crate::tui::DisplayMessage::system(
        "❓ Deploy now or stage first? — you picked: Stage (stage first)",
    ));

    let frame = render_frame_text(&app, 80, 20);
    assert!(
        frame.contains("picked:") && frame.contains("Stage"),
        "recorded answer must survive rendering, got: {}",
        frame
    );
    for line in frame.split('\n') {
        if line.chars().any(|c| char_width(c) > 1) {
            continue;
        }
        assert!(
            line.chars().count() <= 80,
            "row exceeds terminal width: {line:?}"
        );
    }
}

#[test]
fn handle_decision_key_without_pending_decision_returns_false() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app_for_decision();
    rt.block_on(async {
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        let consumed = app
            .handle_decision_key(KeyCode::Enter, KeyModifiers::NONE, &mut remote)
            .await;
        assert!(
            !consumed,
            "no pending decision means the key is not consumed"
        );
        assert!(app.pending_decision.is_none());
    });
}

#[test]
fn up_from_answer_row_returns_to_last_option() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app_for_decision();
    rt.block_on(async {
        let mut decision = make_decision();
        decision.selected = PendingDecision::ANSWER_ROW;
        app.pending_decision = Some(decision);
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        let consumed = app
            .handle_decision_key(KeyCode::Up, KeyModifiers::NONE, &mut remote)
            .await;
        assert!(consumed);
        assert_eq!(
            app.pending_decision.as_ref().unwrap().selected,
            1,
            "Up from the answer row must return to the last option"
        );
    });
}

#[test]
fn backspace_on_an_option_row_falls_through() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app_for_decision();
    rt.block_on(async {
        app.pending_decision = Some(make_decision());
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        let consumed = app
            .handle_decision_key(KeyCode::Backspace, KeyModifiers::NONE, &mut remote)
            .await;
        assert!(
            !consumed,
            "Backspace on an option row edits nothing and must fall through"
        );
        assert_eq!(
            app.pending_decision.as_ref().unwrap().answer_draft,
            "",
            "the draft must be untouched"
        );
    });
}

#[test]
fn failed_decision_send_pushes_an_error_message() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app_for_decision();
    rt.block_on(async {
        app.pending_decision = Some(make_decision());
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        // Drop the peer so the write end fails: no one reads the socket and
        // the send must surface the failure instead of hanging.
        drop(remote.take_dummy_peer());

        // Esc triggers send_decision_answer; with the peer gone the send
        // fails and the error path pushes a display message.
        let consumed = app
            .handle_decision_key(KeyCode::Esc, KeyModifiers::NONE, &mut remote)
            .await;
        assert!(consumed, "Esc is still consumed by the chooser");
        let messages = app.display_messages();
        assert!(
            messages
                .iter()
                .any(|m| m.content.contains("Failed to send decision answer")),
            "send failure must push an error message, got: {:?}",
            messages.last().map(|m| m.content.as_str())
        );
    });
}

#[test]
fn down_on_last_option_row_clamps_to_answer_row_selection() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app_for_decision();
    rt.block_on(async {
        let mut decision = make_decision();
        // The answer row is already selected: Down clamps there.
        decision.selected = PendingDecision::ANSWER_ROW;
        app.pending_decision = Some(decision);
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        let consumed = app
            .handle_decision_key(KeyCode::Down, KeyModifiers::NONE, &mut remote)
            .await;
        assert!(consumed);
        assert!(
            app.pending_decision
                .as_ref()
                .unwrap()
                .selected_is_answer_row(),
            "Down on the answer row must clamp"
        );
    });
}

#[test]
fn down_with_out_of_range_selection_clamps_without_panicking() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app_for_decision();
    rt.block_on(async {
        // Defensive clamp: selected can drift past the last option only via
        // a corrupted state (server sent fewer options than the selection
        // index). Down must not panic or move further out of range.
        let mut decision = make_decision();
        decision.selected = 9;
        app.pending_decision = Some(decision);
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        let consumed = app
            .handle_decision_key(KeyCode::Down, KeyModifiers::NONE, &mut remote)
            .await;
        assert!(consumed, "Down is still consumed by the chooser");
        assert_eq!(
            app.pending_decision.as_ref().unwrap().selected,
            9,
            "the out-of-range selection must not advance further"
        );
    });
}

#[test]
fn enter_with_out_of_range_selection_records_index_fallback() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app_for_decision();
    rt.block_on(async {
        // Same corrupted state as the Down clamp test: `selected` points
        // past the option list. Enter still answers, and the recorded
        // message falls back to the raw index since no option label exists.
        let mut decision = make_decision();
        decision.selected = 9;
        app.pending_decision = Some(decision);
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        let peer = remote.take_dummy_peer().expect("peer stream");

        let consumed = app
            .handle_decision_key(KeyCode::Enter, KeyModifiers::NONE, &mut remote)
            .await;
        assert!(consumed);
        assert!(app.pending_decision.is_none(), "chooser should clear");

        let request = read_peer_request(peer).await;
        match request {
            crate::protocol::Request::DecisionResponse { choice, .. } => {
                assert!(matches!(
                    choice,
                    crate::protocol::DecisionChoice::Option { index: 10 }
                ));
            }
            other => panic!("expected DecisionResponse, got {:?}", other),
        }

        let messages = app.display_messages();
        assert!(
            messages.iter().any(|m| m.content.contains("you picked: 9")),
            "out-of-range pick must record the raw index, got: {:?}",
            messages.last().map(|m| m.content.as_str())
        );
    });
}

/// Retiring the visible chooser by request id clears it and promotes the next
/// queued question, exactly like a user-sent answer does (finding: a nested
/// ask_user inside batch times out server-side but never gets its own
/// ToolDone, leaving a locked input that swallows keys).
#[test]
fn retire_visible_decision_promotes_queued_question() {
    let mut app = create_test_app_for_decision();
    let mut first = make_decision();
    first.request_id = "decision-batch-subcall-1".to_string();
    let mut second = make_decision();
    second.request_id = "decision-batch-subcall-2".to_string();
    app.pending_decision = Some(first);
    app.pending_decision_queue.push_back(second.clone());

    app.retire_decision_by_request_id("decision-batch-subcall-1");

    assert!(
        app.pending_decision
            .is_some_and(|d| d.request_id == second.request_id),
        "retiring the visible chooser must promote the next queued question"
    );
    assert!(
        app.pending_decision_queue.is_empty(),
        "the promoted question must leave the queue"
    );
}

/// Retiring a queued (not visible) question only removes it from the queue;
/// the visible chooser stays untouched.
#[test]
fn retire_queued_decision_leaves_visible_chooser_alone() {
    let mut app = create_test_app_for_decision();
    let visible = make_decision();
    let mut queued = make_decision();
    queued.request_id = "decision-batch-subcall-2".to_string();
    app.pending_decision = Some(visible);
    app.pending_decision_queue.push_back(queued);

    app.retire_decision_by_request_id("decision-batch-subcall-2");

    assert!(
        app.pending_decision
            .is_some_and(|d| d.request_id == "decision-call-1"),
        "retiring a queued question must not touch the visible chooser"
    );
    assert!(
        app.pending_decision_queue.is_empty(),
        "the retired question must leave the queue"
    );
}

/// Retiring an unknown request id is a no-op: nothing visible, nothing queued.
#[test]
fn retire_unknown_request_id_is_noop() {
    let mut app = create_test_app_for_decision();
    let visible = make_decision();
    app.pending_decision = Some(visible);

    app.retire_decision_by_request_id("decision-does-not-exist");

    assert!(
        app.pending_decision
            .is_some_and(|d| d.request_id == "decision-call-1"),
        "an unknown request id must not retire the visible chooser"
    );
}

/// Retiring the last chooser clears the pending state entirely: no queue
/// promotion, and the input box unlocks.
#[test]
fn retire_last_decision_clears_pending_state() {
    let mut app = create_test_app_for_decision();
    let visible = make_decision();
    app.pending_decision = Some(visible);

    app.retire_decision_by_request_id("decision-call-1");

    assert!(
        app.pending_decision.is_none(),
        "retiring the only chooser must clear pending state"
    );
    assert!(
        app.pending_decision_queue.is_empty(),
        "the queue stays empty"
    );
}

/// A cancelled/finished/failed turn kills every question it asked: the
/// visible chooser and the whole queue go together, so a stale queued
/// question cannot resurface in a later turn and steal keyboard input
/// (finding: cancelling left pending_decision_queue intact).
#[test]
fn clear_decisions_and_unblock_drops_queue_with_chooser() {
    let mut app = create_test_app_for_decision();
    let visible = make_decision();
    let mut queued = make_decision();
    queued.request_id = "decision-batch-subcall-2".to_string();
    app.pending_decision = Some(visible);
    app.pending_decision_queue.push_back(queued);

    app.clear_decisions_and_unblock();

    assert!(
        app.pending_decision.is_none(),
        "the visible chooser must be dropped when the turn dies"
    );
    assert!(
        app.pending_decision_queue.is_empty(),
        "queued questions die with the turn, none may resurface later"
    );
}

/// clear_decisions_and_unblock resets the published OSC 9 status even when
/// no chooser is pending: the last published state may still say `blocked`
/// from an earlier question in the same turn, and herdr/herdr-webui would
/// keep showing the stopped pane as waiting for an answer.
#[test]
fn clear_decisions_and_unblock_resets_osc9_even_without_chooser() {
    let mut app = create_test_app_for_decision();

    app.clear_decisions_and_unblock();

    // No panic, no pending state: the OSC 9 reset is emitted inside; the
    // dedup static means we can only assert no-crash here, the byte-level
    // contract lives in decision_osc9's own tests.
    assert!(app.pending_decision.is_none());
    assert!(app.pending_decision_queue.is_empty());
}
