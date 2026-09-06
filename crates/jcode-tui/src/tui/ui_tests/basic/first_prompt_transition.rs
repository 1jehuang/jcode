fn startup_input_y() -> u16 {
    crate::tui::ui::last_layout_snapshot()
        .unwrap()
        .input_area
        .unwrap()
        .y
}

#[test]
fn first_prompt_keeps_composer_and_fills_from_top() {
    let _lock = viewport_snapshot_test_lock();
    for (width, height) in [(80, 24), (100, 40), (60, 50)] {
        for centered in [false, true] {
            crate::tui::ui::clear_test_render_state_for_tests();
            let mut state = TestState {
                input: "FIRST PROMPT".into(),
                centered_mode: centered,
                chat_native_scrollbar: true,
                suggestions: vec![("One".into(), "one".into()), ("Two".into(), "two".into())],
                suppress_info_widgets: true,
                ..Default::default()
            };
            let _ = render_full(&state, width, height);
            let initial_y = startup_input_y();
            state.input.clear();
            state.suggestions.clear();
            state.status = ProcessingStatus::Thinking(std::time::Instant::now());
            let _ = render_full(&state, width, height);
            assert_eq!(
                startup_input_y(),
                initial_y,
                "composer moved while submitting"
            );
            state
                .display_messages
                .push(DisplayMessage::user("FIRST PROMPT"));
            state.messages_version += 1;
            let terminal = render_full(&state, width, height);
            assert_eq!(
                startup_input_y(),
                initial_y,
                "composer moved on submit at {width}x{height}"
            );
            let text = buffer_to_text(&terminal);
            let prompt_y = text
                .lines()
                .position(|line| line.contains("FIRST PROMPT"))
                .unwrap();
            assert_eq!(
                text.lines().position(|line| !line.trim().is_empty()),
                Some(0),
                "the conversation must not retain welcome padding: {text}"
            );
            assert!(
                prompt_y < initial_y as usize,
                "prompt is above the input: {text}"
            );
            assert_eq!(crate::tui::ui::last_resolved_chat_scroll(), 0);

            let mut previous_y = initial_y;
            for rows in 1..=height {
                state.streaming_text = (0..rows).map(|i| format!("response row {i}\n\n")).collect();
                let _ = render_full(&state, width, height);
                let y = startup_input_y();
                assert!(y >= previous_y, "growing output moved input upward");
                assert!(y < height);
                if rows == 1 {
                    assert_eq!(y, initial_y, "first response row should use empty space");
                }
                previous_y = y;
            }
            assert_eq!(
                previous_y,
                height - 1,
                "growing content should fill the viewport"
            );
        }
    }
}

#[test]
fn first_prompt_stays_at_its_beginning_until_output_arrives() {
    let _lock = viewport_snapshot_test_lock();
    crate::tui::ui::clear_test_render_state_for_tests();
    let prompt = format!("FIRST LINE\n{}\nLAST LINE", "middle line\n".repeat(60));
    let mut state = TestState {
        input: prompt.clone(),
        suppress_info_widgets: true,
        ..Default::default()
    };
    let _ = render_full(&state, 80, 24);
    state.input.clear();
    state.display_messages.push(DisplayMessage::user(prompt));
    state.messages_version += 1;
    state.status = ProcessingStatus::Thinking(std::time::Instant::now());
    for _ in 0..3 {
        let terminal = render_full(&state, 80, 24);
        let text = buffer_to_text(&terminal);
        assert!(
            text.contains("FIRST LINE"),
            "missing start of long prompt: {text}"
        );
        assert!(
            !text.contains("LAST LINE"),
            "landed at end of long prompt: {text}"
        );
    }
    state.streaming_text = "RESPONSE ARRIVED".into();
    let mut text = String::new();
    for _ in 0..30 {
        text = buffer_to_text(&render_full(&state, 80, 24));
    }
    assert!(
        text.contains("RESPONSE ARRIVED"),
        "tail following did not resume: {text}"
    );
}

#[test]
fn first_prompt_with_idle_animation_keeps_composer_when_animation_disappears() {
    let _idle_animation = IdleAnimationEnvGuard::enable();
    let _lock = viewport_snapshot_test_lock();
    pin_full_tier();
    crate::tui::ui::clear_test_render_state_for_tests();
    let mut state = TestState {
        input: "hello".into(),
        ..Default::default()
    };
    let _ = render_full(&state, 100, 40);
    let initial_y = startup_input_y();
    assert!(crate::tui::ui::last_idle_animation_area().is_some());
    state.input.clear();
    state.display_messages.push(DisplayMessage::user("hello"));
    state.messages_version += 1;
    state.status = ProcessingStatus::Thinking(std::time::Instant::now());
    let _ = render_full(&state, 100, 40);
    assert_eq!(startup_input_y(), initial_y);
    assert!(crate::tui::ui::last_idle_animation_area().is_none());
}

#[test]
fn first_prompt_floor_does_not_leak_to_another_session_or_resized_view() {
    let _lock = viewport_snapshot_test_lock();
    for resize in [false, true] {
        crate::tui::ui::clear_test_render_state_for_tests();
        let mut state = TestState {
            session_id: Some("welcome-session".into()),
            suppress_info_widgets: true,
            ..Default::default()
        };
        let _ = render_full(&state, 100, 50);
        let initial_y = startup_input_y();
        state
            .display_messages
            .push(DisplayMessage::user("short prompt"));
        state.messages_version += 1;
        if !resize {
            state.session_id = Some("other-session".into());
        }
        let _ = render_full(&state, 100, if resize { 30 } else { 50 });
        assert!(
            startup_input_y() < initial_y,
            "stale welcome anchor leaked into different view"
        );
    }
}
