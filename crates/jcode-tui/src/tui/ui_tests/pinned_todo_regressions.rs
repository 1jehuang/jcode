use super::*;
use ratatui::{Terminal, backend::TestBackend};

/// Keep pinning deterministic without modifying the developer's config.
///
/// Takes the render-state lock here, inside the env lock, so callers cannot
/// invert the canonical env-before-render order and ABBA-deadlock against
/// idle-animation tests holding the env lock.
fn with_pinned_todos_config(f: impl FnOnce()) {
    // Always acquire environment before render state. App construction under
    // with_temp_jcode_home takes these locks in this same order.
    let _env_lock = crate::storage::lock_test_env();
    let _render_lock = viewport_snapshot_test_lock();
    let home = tempfile::tempdir().expect("temporary test home");
    struct RestoreHome(Option<std::ffi::OsString>);
    impl Drop for RestoreHome {
        fn drop(&mut self) {
            if let Some(home) = self.0.take() {
                crate::env::set_var("JCODE_HOME", home);
            } else {
                crate::env::remove_var("JCODE_HOME");
            }
            crate::config::invalidate_config_cache();
        }
    }
    let _restore = RestoreHome(std::env::var_os("JCODE_HOME"));
    crate::env::set_var("JCODE_HOME", home.path());
    std::fs::write(
        home.path().join("config.toml"),
        "[display]\npin_todos = true\n",
    )
    .expect("write isolated config");
    crate::config::invalidate_config_cache();
    assert!(crate::config::config().display.pin_todos);
    f();
}

fn todo_payload() -> String {
    serde_json::json!({"todos": [{
        "id": "regression", "content": "Pinned regression task",
        "status": "in_progress", "priority": "high"
    }]})
    .to_string()
}

/// A subprocess bounds failure even if both worker threads deadlock while
/// holding shared test locks. A timed channel alone would leave the rest of
/// the test binary wedged behind those locks after the assertion failed.
#[test]
fn pinned_config_and_app_reset_lock_order_does_not_deadlock() {
    use std::time::{Duration, Instant};

    const CHILD: &str = "JCODE_TEST_PINNED_CONFIG_LOCK_ORDER_CHILD";
    if std::env::var_os(CHILD).is_some() {
        for _ in 0..8 {
            let env = crate::storage::lock_test_env();
            let (started_tx, started_rx) = std::sync::mpsc::channel();
            let pinned = std::thread::spawn(move || {
                started_tx.send(()).unwrap();
                // Exercise the actual rendering test's lock acquisition path,
                // not a duplicate of the helper's intended implementation.
                justrust_status_is_below_todos_and_never_wraps();
            });
            started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            // Give the pinned test a chance to block on the held environment.
            // Previously it first held render state, creating the ABBA cycle
            // when the environment owner reset render state below.
            std::thread::sleep(Duration::from_millis(25));
            crate::tui::ui::clear_test_render_state_for_tests();
            drop(env);
            pinned.join().unwrap();
        }
        return;
    }

    let name = format!(
        "{}::pinned_config_and_app_reset_lock_order_does_not_deadlock",
        module_path!()
    );
    // libtest names omit the crate prefix from module_path!().
    let name = name.split_once("::").unwrap().1;
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--nocapture"])
        .env(CHILD, "1")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn isolated lock-order regression");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait().expect("poll regression child") {
            let output = child.wait_with_output().expect("collect regression output");
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                status.success(),
                "lock-order regression failed: {status}\n{stdout}\n{stderr}"
            );
            assert!(
                stdout.contains("1 passed"),
                "child did not run the regression: {stdout}"
            );
            break;
        }
        if Instant::now() >= deadline {
            child.kill().expect("kill deadlocked regression child");
            child.wait().expect("reap regression child");
            panic!("environment/render lock acquisition deadlocked (10s deadline)");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn justrust_status_is_below_todos_and_never_wraps() {
    with_pinned_todos_config(|| {
        // Release commit titles may contain the product name too. The status
        // assertion must distinguish UI chrome from unrelated changelog prose.
        let _changelog = header::scoped_unseen_changelog_entries_override_for_tests(vec![
            "justrust release update".to_string(),
        ]);
        let mut state = TestState {
            pinned_todos_payload: Some(todo_payload()),
            justrust_status: Some(crate::tui::justrust_status::StatusRow {
                text: "justrust [remote/hosted] test 编译器 · 51s · 7.8/96 cores".into(),
                tone: crate::tui::justrust_status::Tone::Active,
                own_session: false,
            }),
            ..Default::default()
        };
        for width in [1, 8, 16, 40, 80, 160] {
            let (lines, _) = viewport::pinned_todo_band_lines(&state, width, 24);
            assert!(lines.iter().all(|line| line.width() <= width as usize));
            if width >= 80 {
                assert!(lines.last().unwrap().to_string().contains("justrust"));
                assert!(
                    lines[..lines.len() - 1]
                        .iter()
                        .any(|line| line.to_string().contains("Pinned regression task"))
                );
            }
        }
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal
            .draw(|frame| crate::tui::ui::draw(frame, &state))
            .unwrap();
        let text = terminal_text(&terminal);
        assert!(text.contains("justrust release update"), "{text}");
        let status_rows: Vec<_> = text
            .lines()
            .enumerate()
            .filter(|(_, line)| line.trim_start().starts_with("justrust [remote/hosted]"))
            .collect();
        assert_eq!(status_rows.len(), 1, "{text}");
        let todo_row = text
            .lines()
            .position(|line| line.contains("Pinned regression task"))
            .unwrap();
        assert!(todo_row < status_rows[0].0, "{text}");

        state.pinned_todos_payload = None;
        let (lines, _) = viewport::pinned_todo_band_lines(&state, 100, 24);
        assert_eq!(lines.len(), 1, "a build row does not require todos");
        for (own_session, expected) in [(false, Color::Magenta), (true, Color::Cyan)] {
            let status = state.justrust_status.as_mut().unwrap();
            status.own_session = own_session;
            status.tone = crate::tui::justrust_status::Tone::Failure;
            let (lines, _) = viewport::pinned_todo_band_lines(&state, 100, 24);
            let label = lines[0]
                .spans
                .iter()
                .find(|span| span.content == "justrust")
                .unwrap();
            assert_eq!(label.style.fg, Some(expected));
            assert_eq!(lines[0].spans.last().unwrap().style.fg, Some(Color::Red));
        }
        state.justrust_status = None;
        assert!(
            viewport::pinned_todo_band_lines(&state, 100, 24)
                .0
                .is_empty()
        );
    });
}

#[test]
fn pinned_todos_do_not_duplicate_passive_swarm_gallery_at_any_width() {
    with_pinned_todos_config(|| {
        let member = crate::protocol::SwarmMemberStatus {
            session_id: "test-worker".into(),
            friendly_name: Some("placement-worker".into()),
            status: "running".into(),
            detail: Some("Checking layout".into()),
            task_label: None,
            role: None,
            is_headless: Some(true),
            live_attachments: None,
            status_age_secs: Some(1),
            output_tail: None,
            report_back_to_session_id: None,
            todo_progress: None,
            todo_items: Vec::new(),
            runtime: Default::default(),
        };
        let state = TestState {
            pinned_todos_payload: Some(todo_payload()),
            swarm_members: vec![member],
            ..Default::default()
        };
        for width in [80, 119, 120, 160, 240] {
            let (lines, _) = viewport::pinned_todo_band_lines(&state, width, 36);
            let text = lines
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n");
            assert!(!text.contains("placement-worker"), "{width}: {text}");
            assert!(lines.iter().all(|line| line.width() <= width as usize));
            assert!(text.contains("Pinned regression task"), "{width}: {text}");
        }
        clear_flicker_frame_history_for_tests();
        let mut terminal = Terminal::new(TestBackend::new(160, 36)).unwrap();
        terminal
            .draw(|frame| crate::tui::ui::draw(frame, &state))
            .unwrap();
        let text = terminal_text(&terminal);
        assert!(!text.contains("placement-worker"), "{text}");
        assert_eq!(text.matches("Pinned regression task").count(), 1, "{text}");
    });
}

fn hidden_todo(payload: &str) -> DisplayMessage {
    DisplayMessage {
        role: "tool".into(),
        content: payload.into(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: Some(ToolCall {
            id: "pinned-regression-todo".into(),
            name: "todo".into(),
            input: serde_json::json!({}),
            intent: None,
            thought_signature: None,
        }),
    }
}

fn terminal_text(terminal: &Terminal<TestBackend>) -> String {
    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn pinned_todo_regression_packed_fresh_session_keeps_prompt_when_band_appears() {
    with_pinned_todos_config(|| {
        let _changelog = header::scoped_unseen_changelog_entries_override_for_tests(Vec::new());
        reset_prompt_viewport_state_for_test();
        struct RestoreDebug(bool);
        impl Drop for RestoreDebug {
            fn drop(&mut self) {
                if !self.0 {
                    visual_debug::disable();
                }
            }
        }
        let _debug = RestoreDebug(visual_debug::is_enabled());
        visual_debug::enable();
        let mut state = TestState {
            display_messages: vec![DisplayMessage::user("Fresh session prompt remains visible")],
            messages_version: 1,
            suppress_info_widgets: true,
            ..Default::default()
        };
        let mut terminal = Terminal::new(TestBackend::new(80, 30)).expect("test terminal");
        terminal
            .draw(|frame| draw(frame, &state))
            .expect("initial draw");
        let before = visual_debug::latest_frame().expect("initial capture");
        assert!(before.layout.use_packed);
        assert!(terminal_text(&terminal).contains("Fresh session prompt remains visible"));
        assert!(!terminal_text(&terminal).contains("Pinned regression task"));

        let payload = todo_payload();
        state.display_messages.push(hidden_todo(&payload));
        state.pinned_todos_payload = Some(payload);
        state.messages_version += 1;
        terminal
            .draw(|frame| draw(frame, &state))
            .expect("first todo update draw");
        let after = visual_debug::latest_frame().expect("todo update capture");
        let text = terminal_text(&terminal);
        assert!(
            after.layout.use_packed,
            "fresh session must exercise packed layout"
        );
        assert!(
            text.contains("Pinned regression task"),
            "band missing:\n{text}"
        );
        assert!(
            text.contains("Fresh session prompt remains visible"),
            "prompt lost:\n{text}"
        );
        // The old fresh-session sizing reserved no band rows, leaving the same
        // message area as before and allowing the band to consume the prompt.
        assert!(after.layout.estimated_content_height > before.layout.estimated_content_height);
        assert!(
            after.layout.messages_area.as_ref().unwrap().height
                > before.layout.messages_area.as_ref().unwrap().height
        );
    });
}

#[test]
fn pinned_todo_regression_hidden_message_has_zero_length_cache_boundary() {
    with_pinned_todos_config(|| {
        let mut state = TestState {
            display_messages: vec![
                DisplayMessage::user("Preserve the original prompt"),
                hidden_todo(&todo_payload()),
                hidden_todo("second hidden update"),
                DisplayMessage::assistant("Visible answer before tail edit"),
            ],
            messages_version: 1,
            ..Default::default()
        };
        let base = Arc::new(prepare::prepare_body(&state, 64, false));
        assert_eq!(base.message_boundaries.len(), state.display_messages.len());
        for index in [1, 2] {
            let boundary = &base.message_boundaries[index];
            let previous = &base.message_boundaries[index - 1];
            assert_eq!(
                boundary.msg_hash,
                state.display_messages[index].stable_cache_hash()
            );
            assert_eq!(boundary.wrapped_len, previous.wrapped_len);
            assert_eq!(boundary.raw_len, previous.raw_len);
            assert_eq!(boundary.user_prompt_len, previous.user_prompt_len);
        }
        assert!(
            !base
                .wrapped_plain_lines
                .iter()
                .any(|line| line.contains("Pinned regression task"))
        );

        // Editing the visible tail must retain all three preceding message
        // positions, even though two of them contributed zero transcript rows.
        state.display_messages[3] =
            DisplayMessage::assistant("Visible completed answer after tail edit");
        state.messages_version += 1;
        let keep = prepare::matching_prefix_len(&base, &state.display_messages);
        assert_eq!(keep, 3);
        let mut reused = base;
        prepare::truncate_prepared_to_boundary(Arc::make_mut(&mut reused), keep);
        let reused = prepare::prepare_body_incremental(&state, 64, reused, keep);
        let full = prepare::prepare_body(&state, 64, false);
        assert_eq!(reused.wrapped_plain_lines, full.wrapped_plain_lines);
        assert_eq!(reused.raw_plain_lines, full.raw_plain_lines);
        assert_eq!(reused.user_prompt_texts, full.user_prompt_texts);
        let boundaries = |body: &PreparedMessages| {
            body.message_boundaries
                .iter()
                .map(|b| (b.msg_hash, b.wrapped_len, b.raw_len, b.user_prompt_len))
                .collect::<Vec<_>>()
        };
        assert_eq!(boundaries(&reused), boundaries(&full));
        assert!(
            reused
                .wrapped_plain_lines
                .iter()
                .any(|line| line.contains("Preserve the original prompt"))
        );
        assert!(
            reused
                .wrapped_plain_lines
                .iter()
                .any(|line| line.contains("Visible completed answer"))
        );
    });
}
