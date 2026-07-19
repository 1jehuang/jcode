// Deterministic anchor-stability (smoothness) benchmark over a simulated
// streaming turn.
//
// Drives a scripted reasoning -> answer -> commit turn through full
// `ui::draw` frames on a TestBackend, feeding each rendered messages-area
// frame into a local AnchorStabilityRecorder, then asserts budgets on the
// jarring-event counts. This is the regression gate for transcript
// smoothness: changes that make committed blocks pop, content reposition, or
// the screen reflow will fail here before anyone sees them live.

/// Render one frame and feed the messages area into the recorder.
fn observe_smoothness_frame(
    app: &App,
    terminal: &mut ratatui::Terminal<ratatui::backend::TestBackend>,
    recorder: &mut jcode_tui_core::anchor_stability::AnchorStabilityRecorder,
) {
    terminal
        .draw(|f| crate::tui::ui::draw(f, app))
        .expect("draw");
    let layout = crate::tui::ui::last_layout_snapshot().expect("layout snapshot");
    let frame = crate::tui::ui::smoothness_frame_from_buffer(
        terminal.backend().buffer(),
        layout.messages_area,
        app.scroll_offset,
        !app.auto_scroll_paused,
    )
    .expect("messages area frame");
    recorder.observe(frame);
}

/// Debug variant: also return the rendered text so failures can be diagnosed.
#[allow(dead_code)]
fn observe_smoothness_frame_text(
    app: &App,
    terminal: &mut ratatui::Terminal<ratatui::backend::TestBackend>,
    recorder: &mut jcode_tui_core::anchor_stability::AnchorStabilityRecorder,
) -> String {
    observe_smoothness_frame(app, terminal, recorder);
    let buf = terminal.backend().buffer();
    let mut out = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

#[test]
fn smoothness_benchmark_simulated_streaming_turn_stays_within_budget() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.session.short_name = Some("test".to_string());
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    let backend = ratatui::backend::TestBackend::new(100, 32);
    let mut terminal = ratatui::Terminal::new(backend).expect("terminal");
    let mut recorder = jcode_tui_core::anchor_stability::AnchorStabilityRecorder::new();

    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    observe_smoothness_frame(&app, &mut terminal, &mut recorder);

    // Scripted turn: bursty reasoning, then an answer, then a commit, then a
    // second short reasoning+answer round. Drain the paced buffer between
    // event batches the way the redraw tick does, drawing after every step.
    let reasoning_chunks = [
        "Considering the request and weighing several options carefully.\n",
        "The first option needs less work but covers fewer cases overall.\n",
        "The second option is more robust; checking its constraints now.\n",
    ];
    for chunk in reasoning_chunks {
        app.handle_server_event(
            crate::protocol::ServerEvent::ReasoningDelta {
                text: chunk.to_string(),
            },
            &mut remote,
        );
        // Drain with multiple paced frames per burst, drawing each.
        for _ in 0..4 {
            let ops = app.stream_buffer.flush_smooth_frame();
            app.apply_stream_ops(ops);
            observe_smoothness_frame(&app, &mut terminal, &mut recorder);
        }
    }
    app.handle_server_event(
        crate::protocol::ServerEvent::ReasoningDone {
            duration_secs: None,
        },
        &mut remote,
    );

    let answer_chunks = [
        "Here is the plan: ",
        "first do the setup, ",
        "then run the checks, ",
        "and finally report the results in a table.",
    ];
    for chunk in answer_chunks {
        app.handle_server_event(
            crate::protocol::ServerEvent::TextDelta {
                text: chunk.to_string(),
            },
            &mut remote,
        );
        for _ in 0..4 {
            let ops = app.stream_buffer.flush_smooth_frame();
            app.apply_stream_ops(ops);
            observe_smoothness_frame(&app, &mut terminal, &mut recorder);
        }
    }
    // Force-drain anything left so the commit is deterministic.
    let ops = app.stream_buffer.flush();
    app.apply_stream_ops(ops);
    observe_smoothness_frame(&app, &mut terminal, &mut recorder);

    // Commit (as a tool call boundary would) and keep drawing.
    app.commit_pending_streaming_assistant_message();
    observe_smoothness_frame(&app, &mut terminal, &mut recorder);

    // Second round: more reasoning then a short answer.
    app.handle_server_event(
        crate::protocol::ServerEvent::ReasoningDelta {
            text: "Re-checking the output before finishing.\n".to_string(),
        },
        &mut remote,
    );
    for _ in 0..4 {
        let ops = app.stream_buffer.flush_smooth_frame();
        app.apply_stream_ops(ops);
        observe_smoothness_frame(&app, &mut terminal, &mut recorder);
    }
    app.handle_server_event(
        crate::protocol::ServerEvent::ReasoningDone {
            duration_secs: None,
        },
        &mut remote,
    );
    app.handle_server_event(
        crate::protocol::ServerEvent::TextDelta {
            text: "All done.".to_string(),
        },
        &mut remote,
    );
    let ops = app.stream_buffer.flush();
    app.apply_stream_ops(ops);
    observe_smoothness_frame(&app, &mut terminal, &mut recorder);

    let report = recorder.report();
    assert!(
        report.frames_compared >= 20,
        "benchmark must observe a realistic number of frames, got {}",
        report.frames_compared
    );
    // Budgets: a paced streaming turn must not reflow the screen and nothing
    // should blink. Commits may pop (bounded). One small reposition is
    // permitted at the answer-commit boundary: the retained reasoning trace
    // (kept above the streaming answer so it stays readable) releases when
    // the answer commits, shifting the answer up by the trace height once.
    assert!(
        report.reposition_events <= 1,
        "at most the answer-commit trace release may reposition: {report:?}"
    );
    assert!(
        report.reposition_rows_total <= 4,
        "the trace-release shift must stay small: {report:?}"
    );
    assert_eq!(
        report.mass_reflow_events, 0,
        "no whole-screen reflows during a streaming turn: {report:?}"
    );
    assert_eq!(
        report.blink_events, 0,
        "no rows may blink out and back: {report:?}"
    );
    assert!(
        report.big_pop_events <= 2,
        "at most the commit boundaries may pop, got {}: {report:?}",
        report.big_pop_events
    );
}

#[test]
fn smoothness_plain_text_commit_preserves_the_live_viewport() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.session.short_name = Some("test".to_string());
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.push_display_message(DisplayMessage::user(
        "Explain why smooth terminal streaming matters.".to_string(),
    ));
    let response = (1..=36)
        .map(|i| {
            format!(
                "{i}. Smooth terminal streaming keeps long responses readable and visually stable."
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    app.replace_streaming_text(response);
    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    app.current_message_id = Some(7);

    let backend = ratatui::backend::TestBackend::new(120, 40);
    let mut terminal = ratatui::Terminal::new(backend).expect("terminal");
    let mut recorder = jcode_tui_core::anchor_stability::AnchorStabilityRecorder::new();

    // Settle the fully revealed live view before isolating the completion
    // transition. The committed assistant message should render identically;
    // only the compact turn footer is new.
    for _ in 0..3 {
        observe_smoothness_frame(&app, &mut terminal, &mut recorder);
    }
    app.handle_server_event(crate::protocol::ServerEvent::MessageEnd, &mut remote);
    app.handle_server_event(crate::protocol::ServerEvent::Done { id: 7 }, &mut remote);
    for _ in 0..3 {
        observe_smoothness_frame(&app, &mut terminal, &mut recorder);
    }

    let report = recorder.report();
    assert_eq!(report.reposition_events, 0, "no rows may jump: {report:?}");
    assert_eq!(report.blink_events, 0, "no rows may blink: {report:?}");
    assert_eq!(
        report.mass_reflow_events, 0,
        "committing plain text must not reflow the viewport: {report:?}"
    );
    assert_eq!(
        report.big_pop_events, 0,
        "the footer must not turn a stable response into a large pop: {report:?}"
    );
}

#[test]
fn smoothness_benchmark_mid_transcript_growth_settles_quickly() {
    // An in-place message replacement (todo-table update) grows the transcript
    // mid-document while following the tail. The viewport cannot keep both the
    // grown block and the bottom anchored, so some motion is expected; this
    // benchmark bounds it: the disturbance must settle within a few frames and
    // must not blink, reposition, or mass-reflow.
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.session.short_name = Some("test".to_string());

    // A transcript tall enough to scroll, with a todo-like block in the middle.
    for i in 0..6 {
        app.display_messages.push(DisplayMessage::assistant(format!(
            "Message {i} line one.\nMessage {i} line two.\nMessage {i} line three."
        )));
    }
    let todo_idx = app.display_messages.len();
    app.display_messages.push(DisplayMessage::assistant(
        "todo: item 1\ntodo: item 2".to_string(),
    ));
    for i in 6..10 {
        app.display_messages.push(DisplayMessage::assistant(format!(
            "Message {i} line one.\nMessage {i} line two.\nMessage {i} line three."
        )));
    }
    app.bump_display_messages_version();
    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;

    let backend = ratatui::backend::TestBackend::new(100, 28);
    let mut terminal = ratatui::Terminal::new(backend).expect("terminal");
    let mut recorder = jcode_tui_core::anchor_stability::AnchorStabilityRecorder::new();

    // Settle the initial view.
    for _ in 0..3 {
        observe_smoothness_frame(&app, &mut terminal, &mut recorder);
    }

    // Grow the mid-transcript block by many rows at once (todo update).
    let grown = (1..=10)
        .map(|i| format!("todo: item {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    app.display_messages[todo_idx].content = grown;
    app.bump_display_messages_version();

    // Render until motion settles (tail catch-up slide runs at frame cadence).
    for _ in 0..20 {
        observe_smoothness_frame(&app, &mut terminal, &mut recorder);
    }

    let report = recorder.report();
    assert_eq!(report.blink_events, 0, "no blinks: {report:?}");
    // Known limitation (budget ratchet): a mid-transcript block growing while
    // the viewport is bottom-anchored moves content above and below it in
    // opposite directions, so the single update frame reads as one reflow +
    // one pop (and, depending on status-row timing, a tiny reposition).
    // Per-message height-diff easing would remove this; until then the budget
    // pins the disturbance to exactly one frame so regressions (flicker
    // loops, repeated reflows) still fail.
    assert!(
        report.reposition_events <= 1 && report.reposition_rows_total <= 2,
        "at most a tiny one-frame reposition during growth: {report:?}"
    );
    assert!(
        report.mass_reflow_events <= 1,
        "at most the one growth frame may reflow: {report:?}"
    );
    // The tail catch-up slide spreads the disturbance over a couple frames,
    // so up to two frames may individually exceed the pop threshold.
    assert!(
        report.big_pop_events <= 2,
        "growth disturbance must stay within two frames: {report:?}"
    );
    assert!(
        report.frames_with_changes <= 4,
        "mid-transcript growth must settle in a frame or two, {} frames changed: {report:?}",
        report.frames_with_changes
    );
}
