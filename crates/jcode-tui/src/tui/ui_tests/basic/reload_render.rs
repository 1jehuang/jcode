// `use super::*` is inherited from the including module (basic.rs), which
// already brings the shared test helpers and UI symbols into scope.
#[test]
fn reload_overflow_preflight_is_conservative() {
    let empty = DisplayMessage::assistant("");
    let prompt = DisplayMessage::user("");
    assert!(!transcript_definitely_overflows(&[], 0));
    assert!(!transcript_definitely_overflows(
        std::slice::from_ref(&empty),
        0
    ));
    assert!(!transcript_definitely_overflows(
        std::slice::from_ref(&prompt),
        1
    ));
    assert!(transcript_definitely_overflows(
        std::slice::from_ref(&prompt),
        0
    ));
    // Even an empty assistant contributes a separator after a rendered prompt.
    assert!(transcript_definitely_overflows(&[prompt.clone(), empty], 1));
    let tools: Vec<_> = (0..100)
        .map(|_| DisplayMessage {
            role: "tool".into(),
            ..DisplayMessage::assistant("")
        })
        .collect();
    assert!(!transcript_definitely_overflows(&tools, 1));

    let mut messages = vec![prompt];
    messages.extend(tools);
    messages.push(DisplayMessage::user("next"));
    assert!(!transcript_definitely_overflows(&messages, 3));
    assert!(transcript_definitely_overflows(&messages, 2));
}

#[test]
fn reload_overflow_preflight_never_exceeds_rendered_body() {
    let _lock = viewport_snapshot_test_lock();
    let messages = [
        DisplayMessage::assistant(""),
        DisplayMessage::user("one\n\ntwo"),
        DisplayMessage::assistant("paragraph\n\n```rust\nfn main() {}\n```"),
        DisplayMessage {
            role: "tool".into(),
            ..DisplayMessage::assistant("")
        },
        DisplayMessage {
            role: "meta".into(),
            ..DisplayMessage::assistant("meta")
        },
        DisplayMessage {
            role: "spacer".into(),
            ..DisplayMessage::assistant("4")
        },
        DisplayMessage::user(""),
    ];
    for len in 0..=messages.len() {
        let state = TestState {
            display_messages: messages[..len].to_vec(),
            ..Default::default()
        };
        for width in [20, 80, 160] {
            let body = prepare::prepare_body(&state, width, false);
            assert!(!transcript_definitely_overflows(
                &state.display_messages,
                body.wrapped_lines.len()
            ));
        }
    }
}

#[test]
fn reload_long_transcript_prepares_once_and_preserves_frame() {
    let _lock = viewport_snapshot_test_lock();
    let state = TestState {
        display_messages: (0..80)
            .map(|i| {
                if i % 2 == 0 {
                    DisplayMessage::user(format!("prompt {i}"))
                } else {
                    DisplayMessage::assistant(format!("response {i}"))
                }
            })
            .collect(),
        messages_version: 1,
        chat_native_scrollbar: true,
        ..Default::default()
    };
    let backend = ratatui::backend::TestBackend::new(100, 30);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    set_last_chat_scrollbar_visible(false);
    terminal.draw(|frame| draw(frame, &state)).unwrap();
    let cold = terminal.backend().buffer().clone();
    assert!(last_chat_scrollbar_visible());
    assert_eq!(
        frame_metrics::frame_perf_stats_snapshot().prep_prepare_at_calls,
        1
    );
    terminal.draw(|frame| draw(frame, &state)).unwrap();
    assert_eq!(&cold, terminal.backend().buffer());
    assert_eq!(
        frame_metrics::frame_perf_stats_snapshot().prep_prepare_at_calls,
        1
    );
}
