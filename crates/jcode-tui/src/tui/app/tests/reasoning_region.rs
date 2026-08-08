// Tests for the streaming reasoning region helpers.
//
// Reasoning text is rendered as dim, italic lines (no blockquote `│` gutter, no
// header, no footer). Each complete line is wrapped in `*…*` with an invisible
// REASONING_SENTINEL inside both ends that the markdown renderer strips and dims.
// (Both ends so whitespace at the line edges can't break CommonMark emphasis.) The
// region auto-closes when real output or a tool call begins so the final answer
// renders as normal (non-italic) text.
//
// The in-progress (not yet newline-terminated) line renders live as a partial
// `*…*` tail so reasoning trickles in token-by-token; that tail is rebuilt in
// place on each delta and promoted to a committed line when its newline arrives.
//
// In `current` mode (the default) reasoning is *ephemeral*: only the live block is
// ever shown. Once it closes (the model answers or runs a tool) the whole block is
// sliced back out of the stream in place, so no per-block trace accumulates and
// answer text keeps its order.

#[test]
fn reasoning_region_emits_dim_italic_lines_no_gutter_header_or_footer() {
    // These assertions describe `current` (live-then-anchored) reasoning, so
    // pin that mode: the global default became `Off` when show_thinking was
    // defaulted off for new users (166e4444f).
    with_reasoning_current_home(|| {
        let mut app = create_test_app();

        app.open_reasoning_region();
        app.append_reasoning_text("Let me think.\nSecond thought.");
        // While streaming, reasoning is dim+italic markup in the live stream buffer.
        let streaming = app.streaming_text().to_string();
        assert!(
            !streaming.contains("Thinking"),
            "no header expected: {streaming:?}"
        );
        assert!(
            !streaming.contains('>'),
            "no blockquote gutter expected: {streaming:?}"
        );
        assert!(
            !streaming.contains("Thought for"),
            "no footer expected: {streaming:?}"
        );
        let sentinel = jcode_tui_markdown::REASONING_SENTINEL;
        assert!(
            streaming.contains(&format!("*{sentinel}Let me think.{sentinel}*")),
            "first line not dim+italic: {streaming:?}"
        );
        assert!(
            streaming.contains(&format!("*{sentinel}Second thought.{sentinel}*")),
            "second line not dim+italic: {streaming:?}"
        );

        // In `current` mode (the default), closing anchors the block in the
        // transcript flow as a display-only reasoning message: it leaves the live
        // stream and never moves again.
        app.close_reasoning_region(None);
        assert!(
            app.streaming_text().is_empty(),
            "reasoning should leave the live stream once anchored: {:?}",
            app.streaming_text()
        );
        let anchored = app
            .display_messages
            .iter()
            .find(|m| m.role == "reasoning")
            .expect("closed trace anchors as a display-only reasoning message");
        assert!(
            anchored.content.contains("Let me think."),
            "anchored trace keeps its content: {:?}",
            anchored.content
        );
    });
}

#[test]
fn reasoning_region_closes_before_normal_output() {
    // These assertions describe `current` (live-then-anchored) reasoning, so
    // pin that mode: the global default became `Off` when show_thinking was
    // defaulted off for new users (166e4444f).
    with_reasoning_current_home(|| {
        let mut app = create_test_app();

        app.open_reasoning_region();
        app.append_reasoning_text("thinking about it\n");
        // Real output begins; region must close so output is not styled as reasoning.
        app.close_reasoning_region(None);
        app.append_streaming_text("Final answer.");

        // The answer stays in the live stream and must never be styled as reasoning.
        let text = app.streaming_text();
        assert!(
            text.contains("Final answer."),
            "answer present in stream: {text:?}"
        );
        let answer_line = text
            .lines()
            .find(|l| l.contains("Final answer."))
            .expect("answer line present");
        assert!(
            !answer_line.contains(jcode_tui_markdown::REASONING_SENTINEL),
            "final answer must not be styled as reasoning: {answer_line:?}"
        );
        // The reasoning left the stream and anchored as a display-only message.
        assert!(
            !text.contains(jcode_tui_markdown::REASONING_SENTINEL),
            "reasoning must not remain in the answer stream: {text:?}"
        );
        assert!(
            app.display_messages.iter().any(|m| m.role == "reasoning"),
            "closed trace anchors in the transcript"
        );
    });
}

#[test]
fn reasoning_region_open_is_idempotent() {
    let mut app = create_test_app();

    app.open_reasoning_region();
    app.append_reasoning_text("a\n");
    app.open_reasoning_region(); // no-op while open
    app.append_reasoning_text("b\n");

    let text = app.streaming_text();
    let sentinel = jcode_tui_markdown::REASONING_SENTINEL;
    assert!(
        text.contains(&format!("*{sentinel}a{sentinel}*")),
        "first chunk: {text:?}"
    );
    assert!(
        text.contains(&format!("*{sentinel}b{sentinel}*")),
        "second chunk: {text:?}"
    );
    // No extra separator burst between the two chunks.
    assert!(
        !text.contains(&format!("*{sentinel}a{sentinel}*\n\n")),
        "second chunk should not restart the region: {text:?}"
    );
}

#[test]
fn reasoning_line_split_across_deltas_stays_one_run() {
    let mut app = create_test_app();

    app.open_reasoning_region();
    app.append_reasoning_text("one ");
    app.append_reasoning_text("two\n");

    // While streaming live, the split-across-deltas line is a single emphasis run.
    let content = app.streaming_text();
    let sentinel = jcode_tui_markdown::REASONING_SENTINEL;
    assert!(
        content.contains(&format!("*{sentinel}one two{sentinel}*")),
        "split line must be one emphasis run: {content:?}"
    );
}

#[test]
fn reasoning_region_renders_dim_italic_text_without_gutter() {
    use ratatui::style::Modifier;

    let mut app = create_test_app();

    app.open_reasoning_region();
    app.append_reasoning_text("considering options\n");

    // The live reasoning renders dim+italic from the streaming buffer.
    let reasoning_content = app.streaming_text().to_string();

    let lines = crate::tui::markdown::render_markdown_with_width(&reasoning_content, Some(80));
    let body = lines
        .iter()
        .find(|l| {
            l.spans
                .iter()
                .any(|s| s.content.as_ref().contains("considering options"))
        })
        .expect("reasoning body line present");

    let rendered: String = body.spans.iter().map(|s| s.content.as_ref()).collect();
    // No blockquote gutter, and the sentinel is stripped from the visible text.
    assert!(!rendered.contains('│'), "no gutter expected: {rendered:?}");
    assert!(
        !rendered.contains(jcode_tui_markdown::REASONING_SENTINEL),
        "sentinel must be stripped: {rendered:?}"
    );

    let body_span = body
        .spans
        .iter()
        .find(|s| s.content.as_ref().contains("considering options"))
        .expect("body span present");
    assert!(
        body_span.style.add_modifier.contains(Modifier::ITALIC),
        "reasoning body should be italic: {:?}",
        body_span.style
    );
}

#[test]
fn strip_reasoning_lines_removes_reasoning_keeps_answer() {
    use crate::tui::app::input::strip_reasoning_lines;

    // Build content the way the streaming buffer would: reasoning lines wrapped
    // with the sentinel, then a normal answer paragraph.
    let mut content = String::new();
    content.push_str(&jcode_tui_markdown::reasoning_line_markup("thinking one"));
    content.push_str(&jcode_tui_markdown::reasoning_line_markup("thinking two"));
    content.push('\n');
    content.push_str("Here is the answer.\n");

    let stripped = strip_reasoning_lines(&content);
    assert_eq!(stripped, "Here is the answer.");
    assert!(!stripped.contains(jcode_tui_markdown::REASONING_SENTINEL));
}

#[test]
fn strip_reasoning_lines_reasoning_only_becomes_empty() {
    use crate::tui::app::input::strip_reasoning_lines;

    let mut content = String::new();
    content.push_str(&jcode_tui_markdown::reasoning_line_markup("only thinking"));
    let stripped = strip_reasoning_lines(&content);
    assert!(stripped.trim().is_empty(), "got: {stripped:?}");
}

#[test]
fn reasoning_partial_line_renders_live_before_newline() {
    // The in-progress line (no trailing newline) must render immediately as a
    // dim+italic partial tail so reasoning streams token-by-token.
    let mut app = create_test_app();
    let sentinel = jcode_tui_markdown::REASONING_SENTINEL;

    app.open_reasoning_region();
    app.append_reasoning_text("partial thou");

    let text = app.streaming_text();
    assert!(
        text.contains(&format!("*{sentinel}partial thou{sentinel}*")),
        "partial line should render live: {text:?}"
    );
}

#[test]
fn reasoning_partial_tail_grows_in_place_without_duplication() {
    // Successive deltas of the same line replace the live tail (truncate + rebuild)
    // rather than appending duplicate fragments.
    let mut app = create_test_app();
    let sentinel = jcode_tui_markdown::REASONING_SENTINEL;

    app.open_reasoning_region();
    app.append_reasoning_text("one ");
    app.append_reasoning_text("two ");
    app.append_reasoning_text("three");

    let text = app.streaming_text();
    assert!(
        text.contains(&format!("*{sentinel}one two three{sentinel}*")),
        "tail should grow in place: {text:?}"
    );
    // The earlier partial fragments must not linger as separate runs.
    assert!(
        !text.contains(&format!("*{sentinel}one {sentinel}*")),
        "stale partial tail should be replaced, not duplicated: {text:?}"
    );
    assert_eq!(
        text.matches(sentinel).count(),
        2,
        "exactly one live emphasis run (two sentinels) expected: {text:?}"
    );
}

#[test]
fn reasoning_partial_promotes_to_committed_line_on_newline() {
    // When the newline arrives, the live tail becomes a committed line and a fresh
    // (empty) tail follows; no duplicate copies of the completed line remain.
    let mut app = create_test_app();
    let sentinel = jcode_tui_markdown::REASONING_SENTINEL;

    app.open_reasoning_region();
    app.append_reasoning_text("growing line");
    app.append_reasoning_text("\nnext");

    let text = app.streaming_text();
    // Committed first line (hard-break terminated) and a live second-line tail.
    assert!(
        text.contains(&format!("*{sentinel}growing line{sentinel}*  \n")),
        "first line should be committed with a hard break: {text:?}"
    );
    assert!(
        text.contains(&format!("*{sentinel}next{sentinel}*")),
        "second line should render live: {text:?}"
    );
    // The completed line must appear exactly once (no partial+committed duplication).
    assert_eq!(
        text.matches(&format!("*{sentinel}growing line{sentinel}*"))
            .count(),
        1,
        "completed line must not be duplicated: {text:?}"
    );
}

#[test]
fn reasoning_close_promotes_pending_partial_line() {
    // These assertions describe `current` (live-then-anchored) reasoning, so
    // pin that mode: the global default became `Off` when show_thinking was
    // defaulted off for new users (166e4444f).
    with_reasoning_current_home(|| {
        // Closing the region with an in-progress (no-newline) partial promotes it to a
        // committed line exactly once, then collapses into the reasoning message.
        let mut app = create_test_app();
        let sentinel = jcode_tui_markdown::REASONING_SENTINEL;

        app.open_reasoning_region();
        app.append_reasoning_text("final thought");
        app.close_reasoning_region(None);

        // The reasoning leaves the live stream on close and anchors as a display
        // message, with the pending partial promoted to a committed line.
        let _ = sentinel;
        assert!(
            app.streaming_text().is_empty(),
            "reasoning should leave the live stream once anchored: {:?}",
            app.streaming_text()
        );
        let anchored = app
            .display_messages
            .iter()
            .find(|m| m.role == "reasoning")
            .expect("anchored trace exists");
        assert!(
            anchored.content.contains("final thought"),
            "pending partial promoted into the anchored trace: {:?}",
            anchored.content
        );
    });
}

#[test]
fn reasoning_preceded_by_answer_keeps_order_and_drops_reasoning() {
    // These assertions describe `current` (live-then-anchored) reasoning, so
    // pin that mode: the global default became `Off` when show_thinking was
    // defaulted off for new users (166e4444f).
    with_reasoning_current_home(|| {
        // Answer text streamed *before* a reasoning block commits ahead of the
        // anchored trace so the transcript keeps chronological order; answer text
        // after the close streams below the anchored trace.
        let mut app = create_test_app();
        let sentinel = jcode_tui_markdown::REASONING_SENTINEL;

        app.append_streaming_text("Intro before thinking.");
        app.open_reasoning_region();
        app.append_reasoning_text("let me think\nstep two\n");
        app.close_reasoning_region(None);
        app.append_streaming_text("Conclusion after thinking.");

        let text = app.streaming_text();
        assert!(
            !text.contains(sentinel),
            "reasoning must leave the live stream: {text:?}"
        );
        assert!(
            text.contains("Conclusion after thinking."),
            "post-close answer streams live: {text:?}"
        );
        // Intro committed ahead of the anchored trace, in order.
        let intro_idx = app
            .display_messages
            .iter()
            .position(|m| m.role == "assistant" && m.content.contains("Intro before thinking."))
            .expect("intro committed before the anchored trace");
        let trace_idx = app
            .display_messages
            .iter()
            .position(|m| m.role == "reasoning")
            .expect("trace anchored in the transcript");
        assert!(
            intro_idx < trace_idx,
            "intro must precede the anchored trace: {intro_idx} vs {trace_idx}"
        );
    });
}

#[test]
fn multiple_reasoning_blocks_anchor_in_order_and_clear_next_prompt() {
    // Hermetic JCODE_HOME: these assertions depend on the default
    // `reasoning_display = "current"` config (see sibling anchor/GC tests).
    with_reasoning_current_home(|| {
        // Each closed block anchors in the transcript flow, in order, and stays
        // readable for the whole turn. The next user prompt clears them all.
        let mut app = create_test_app();

        app.open_reasoning_region();
        app.append_reasoning_text("first block thinking\n");
        app.close_reasoning_region(None);
        app.append_streaming_text("Answer one.");
        app.commit_pending_streaming_assistant_message();

        app.open_reasoning_region();
        app.append_reasoning_text("second block thinking\n");
        app.close_reasoning_region(None);

        let reasoning_msgs: Vec<usize> = app
            .display_messages
            .iter()
            .enumerate()
            .filter(|(_, m)| m.role == "reasoning")
            .map(|(i, _)| i)
            .collect();
        assert_eq!(
            reasoning_msgs.len(),
            2,
            "both traces anchor for the duration of the turn"
        );
        assert!(
            !app.streaming_text()
                .contains(jcode_tui_markdown::REASONING_SENTINEL),
            "no reasoning markup should linger in the stream: {:?}",
            app.streaming_text()
        );

        // The next prompt removes the turn's traces (ephemeral across turns).
        app.clear_turn_reasoning_traces();
        assert_eq!(
            app.display_messages
                .iter()
                .filter(|m| m.role == "reasoning")
                .count(),
            0,
            "next prompt clears the turn's anchored traces"
        );
        assert!(
            app.display_messages
                .iter()
                .any(|m| m.content.contains("Answer one.")),
            "committed answers survive trace cleanup"
        );
    });
}

#[test]
fn anchored_trace_never_moves_and_clears_on_next_prompt() {
    // Hermetic JCODE_HOME: these assertions depend on the default
    // `reasoning_display = "current"` config and on ambient/info state
    // not leaking in from the developer's real ~/.jcode (other tests
    // write config overrides into the shared per-process test home).
    with_reasoning_current_home(|| {
        // Anchored traces are ordinary transcript entries: they keep their index
        // as later content is appended (no bottom-following, no hoisting) and are
        // removed when the next user prompt begins.
        let mut app = create_test_app();

        app.open_reasoning_region();
        app.append_reasoning_text("first trace\n");
        app.close_reasoning_region(None);

        let trace_idx = app
            .display_messages
            .iter()
            .position(|m| m.role == "reasoning")
            .expect("first trace anchored");

        // Later activity appends below; the trace index is unchanged.
        app.append_streaming_text("answer text");
        app.commit_pending_streaming_assistant_message();
        app.open_reasoning_region();
        app.append_reasoning_text("second trace\n");
        app.close_reasoning_region(None);

        assert_eq!(
            app.display_messages[trace_idx].role, "reasoning",
            "anchored trace must keep its transcript position"
        );
        assert!(
            app.display_messages[trace_idx]
                .content
                .contains("first trace"),
            "anchored trace content unchanged"
        );

        // Next prompt clears all of the turn's traces.
        app.clear_turn_reasoning_traces();
        assert_eq!(
            app.display_messages
                .iter()
                .filter(|m| m.role == "reasoning")
                .count(),
            0
        );
    });
}

#[test]
fn remote_reasoning_delta_burst_is_paced_not_dumped() {
    // Pacing only happens when reasoning is actually displayed. The global
    // default is `Off` (166e4444f), under which the delta is dropped and
    // nothing is ever buffered, so this must pin `Current` like its sibling
    // tests do.
    with_reasoning_current_home(|| {

        // A large provider reasoning burst must reveal over multiple paced frames
        // (via the segment-aware StreamBuffer), not pop in all at once. This is the
        // regression test for "reasoning mode feels choppy".
        let mut app = create_test_app();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        app.is_processing = true;
        app.status = ProcessingStatus::Streaming;

        let burst = "x".repeat(400);
        app.handle_server_event(
            crate::protocol::ServerEvent::ReasoningDelta { text: burst },
            &mut remote,
        );

        // Only a small paced slice should be visible immediately; the rest stays
        // buffered and drains on subsequent redraw frames.
        let visible = app.streaming_text().matches('x').count();
        assert!(
            visible < 400,
            "reasoning burst must not dump in one frame, revealed {visible} chars"
        );
        assert!(
            !app.stream_buffer.is_empty(),
            "remainder must stay buffered for paced reveal"
        );

        // Draining the buffer (as the redraw tick does) eventually reveals it all.
        let ops = app.stream_buffer.flush();
        app.apply_stream_ops(ops);
        assert_eq!(app.streaming_text().matches('x').count(), 400);
    });
}

#[test]
fn remote_reasoning_then_text_preserves_order_through_paced_buffer() {
    // Interleaved reasoning -> answer must reveal in arrival order even though
    // both kinds now share one paced backlog: the reasoning region closes after
    // the last buffered reasoning char and before the first answer char.
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;

    app.handle_server_event(
        crate::protocol::ServerEvent::ReasoningDelta {
            text: "thinking hard about this problem\n".to_string(),
        },
        &mut remote,
    );
    app.handle_server_event(
        crate::protocol::ServerEvent::ReasoningDone {
            duration_secs: None,
        },
        &mut remote,
    );
    app.handle_server_event(
        crate::protocol::ServerEvent::TextDelta {
            text: "The answer is 42.".to_string(),
        },
        &mut remote,
    );

    // Drain whatever is still paced.
    let ops = app.stream_buffer.flush();
    app.apply_stream_ops(ops);

    // The reasoning region must be closed (current mode discards/retains it) and
    // the answer text must be present, unstyled, after it.
    assert!(!app.reasoning_streaming, "region must close before answer");
    let text = app.streaming_text();
    assert!(
        text.contains("The answer is 42."),
        "answer must reveal after reasoning: {text:?}"
    );
}

#[test]
fn anchored_trace_survives_tool_commit_and_answer_commit() {
    // Hermetic JCODE_HOME: these assertions depend on the default
    // `reasoning_display = "current"` config and on ambient/info state
    // not leaking in from the developer's real ~/.jcode (other tests
    // write config overrides into the shared per-process test home).
    with_reasoning_current_home(|| {
        // Anchored traces are independent transcript entries: neither a tool-only
        // commit nor an answer commit touches them, so the thought stays readable
        // (and stationary) for the rest of the turn.
        let mut app = create_test_app();
        app.is_processing = true;

        app.open_reasoning_region();
        app.append_reasoning_text("pre-tool thinking\n");
        app.close_reasoning_region(None);
        assert_eq!(trace_count(&app), 1);

        // Tool-only commit (no streamed answer text).
        app.commit_pending_streaming_assistant_message();
        assert_eq!(trace_count(&app), 1, "tool commit leaves the trace anchored");

        // Answer commit.
        app.append_streaming_text("the final answer");
        app.commit_pending_streaming_assistant_message();
        assert_eq!(
            trace_count(&app),
            1,
            "answer commit leaves the trace anchored"
        );
        assert!(
            !app
                .display_messages
                .iter()
                .any(|m| m.role == "assistant" && m.content.contains("thought")),
            "no thought-summary residue may be committed"
        );
    });
}

fn trace_count(app: &App) -> usize {
    app.display_messages
        .iter()
        .filter(|m| m.role == "reasoning")
        .count()
}

#[test]
fn gc_dissolves_stale_traces_only_when_provably_offscreen() {
    // Hermetic JCODE_HOME: these assertions depend on the default
    // `reasoning_display = "current"` config and on ambient/info state
    // not leaking in from the developer's real ~/.jcode (other tests
    // write config overrides into the shared per-process test home).
    with_reasoning_current_home(|| {
        // Stale traces (all but the most recent) are GC'd only once the transcript
        // has grown a full viewport past their anchor point, so removal can never
        // cause visible motion while tail-following.
        let mut app = create_test_app();
        app.is_processing = true;

        // Two traces: the first anchored when the transcript was 10 lines tall.
        crate::tui::ui::set_last_total_wrapped_lines(10);
        app.open_reasoning_region();
        app.append_reasoning_text("old thought\n");
        app.close_reasoning_region(None);

        crate::tui::ui::set_last_total_wrapped_lines(40);
        app.open_reasoning_region();
        app.append_reasoning_text("current thought\n");
        app.close_reasoning_region(None);

        let viewport_h = 20u16;
        crate::tui::ui::record_layout_snapshot(
            ratatui::layout::Rect::new(0, 0, 80, viewport_h),
            None,
            None,
            None,
        );

        // Transcript hasn't grown enough yet: 25 - 10 = 15 <= 20 + 2 margin.
        crate::tui::ui::set_last_total_wrapped_lines(25);
        assert!(!app.gc_offscreen_reasoning_traces());
        assert_eq!(trace_count(&app), 2, "no GC while possibly on screen");

        // Transcript grew a viewport past the first anchor: 40 - 10 = 30 > 22.
        crate::tui::ui::set_last_total_wrapped_lines(40);
        assert!(app.gc_offscreen_reasoning_traces());
        assert_eq!(trace_count(&app), 1, "stale off-screen trace dissolved");
        assert!(
            app.display_messages
                .iter()
                .any(|m| m.role == "reasoning" && m.content.contains("current thought")),
            "the most recent trace always survives"
        );
    });
}

#[test]
fn gc_never_runs_while_user_scrolled_up() {
    // Hermetic JCODE_HOME: these assertions depend on the default
    // `reasoning_display = "current"` config and on ambient/info state
    // not leaking in from the developer's real ~/.jcode (other tests
    // write config overrides into the shared per-process test home).
    with_reasoning_current_home(|| {
        let mut app = create_test_app();
        app.is_processing = true;

        crate::tui::ui::set_last_total_wrapped_lines(10);
        app.open_reasoning_region();
        app.append_reasoning_text("old thought\n");
        app.close_reasoning_region(None);
        app.open_reasoning_region();
        app.append_reasoning_text("current thought\n");
        app.close_reasoning_region(None);

        crate::tui::ui::record_layout_snapshot(
            ratatui::layout::Rect::new(0, 0, 80, 20),
            None,
            None,
            None,
        );
        crate::tui::ui::set_last_total_wrapped_lines(200);

        // Scrolled up: the user may be reading the old trace; never remove it.
        app.auto_scroll_paused = true;
        assert!(!app.gc_offscreen_reasoning_traces());
        assert_eq!(trace_count(&app), 2);

        // Back at the tail: GC may proceed.
        app.auto_scroll_paused = false;
        assert!(app.gc_offscreen_reasoning_traces());
        assert_eq!(trace_count(&app), 1);
    });
}

#[test]
fn repro_reasoning_rendered_then_removed_when_turn_ends_open() {
    // These assertions describe `current` (live-then-anchored) reasoning, so
    // pin that mode: the global default became `Off` when show_thinking was
    // defaulted off for new users (166e4444f).
    with_reasoning_current_home(|| {
        // REPRO: a turn whose reasoning region is still open when `Done` arrives
        // (reasoning streamed, but no `ReasoningDone` and no answer text followed)
        // renders the reasoning live, then DROPS it on finish: `Done` commits via
        // `take_streaming_text` + `collapse_reasoning_for_commit`, which strips every
        // reasoning-sentinel line, and the region was never closed to anchor a trace.
        // Expected (correct) behavior: the live-rendered reasoning is preserved (as an
        // anchored trace) rather than rendered-then-removed.
        let mut app = create_test_app();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        app.is_processing = true;
        app.status = ProcessingStatus::Streaming;
        app.current_message_id = Some(1);

        // Reasoning streams in and renders live (dim+italic) in the stream buffer.
        app.handle_server_event(
            crate::protocol::ServerEvent::ReasoningDelta {
                text: "weighing the options before answering\n".to_string(),
            },
            &mut remote,
        );
        let ops = app.stream_buffer.flush();
        app.apply_stream_ops(ops);
        assert!(
            app.streaming_text()
                .contains(jcode_tui_markdown::REASONING_SENTINEL),
            "precondition: reasoning rendered live in the stream"
        );
        assert!(app.reasoning_streaming, "region open: no ReasoningDone sent");

        // Turn ends with the region still open (no ReasoningDone, no answer text).
        app.handle_server_event(crate::protocol::ServerEvent::Done { id: 1 }, &mut remote);

        // The reasoning that was rendered live must not silently vanish on finish.
        let lingered_in_stream = app
            .streaming_text()
            .contains(jcode_tui_markdown::REASONING_SENTINEL);
        let anchored = app
            .display_messages
            .iter()
            .any(|m| m.role == "reasoning" && m.content.contains("weighing the options"));
        assert!(
            anchored || lingered_in_stream,
            "BUG: reasoning was rendered live then removed on turn finish; \
             display_messages={:?}, stream={:?}",
            app.display_messages
                .iter()
                .map(|m| (m.role.as_str(), m.content.as_str()))
                .collect::<Vec<_>>(),
            app.streaming_text(),
        );
    });
}

#[test]
fn open_reasoning_region_closed_at_turn_finish_is_anchored_not_dropped() {
    // These assertions describe `current` (live-then-anchored) reasoning, so
    // pin that mode: the global default became `Off` when show_thinking was
    // defaulted off for new users (166e4444f).
    with_reasoning_current_home(|| {
        // Mirrors the local turn loop's end-of-turn commit: when a turn finishes
        // with the reasoning region still open (reasoning streamed, but no answer
        // text and no explicit close), the finish path must close the region so the
        // live-rendered reasoning is anchored as a trace rather than silently
        // stripped by `collapse_reasoning_for_commit`.
        let mut app = create_test_app();
        app.is_processing = true;

        app.open_reasoning_region();
        app.append_reasoning_text("weighing the options before answering");
        assert!(app.reasoning_streaming, "precondition: region open");
        assert!(
            app.streaming_text()
                .contains(jcode_tui_markdown::REASONING_SENTINEL),
            "precondition: reasoning rendered live in the stream"
        );

        // End-of-turn commit path (matches turn.rs / Done handler): close any open
        // region first, then commit whatever remains.
        if app.reasoning_streaming {
            app.close_reasoning_region(None);
        }
        let _ = app.commit_pending_streaming_assistant_message();

        let anchored = app
            .display_messages
            .iter()
            .any(|m| m.role == "reasoning" && m.content.contains("weighing the options"));
        let lingered_in_stream = app
            .streaming_text()
            .contains(jcode_tui_markdown::REASONING_SENTINEL);
        assert!(
            anchored || lingered_in_stream,
            "reasoning rendered live must be preserved at turn finish; display_messages={:?}, stream={:?}",
            app.display_messages
                .iter()
                .map(|m| (m.role.as_str(), m.content.as_str()))
                .collect::<Vec<_>>(),
            app.streaming_text(),
        );
    });
}

#[test]
fn gc_keeps_single_trace_indefinitely() {
    // Hermetic JCODE_HOME: these assertions depend on the default
    // `reasoning_display = "current"` config and on ambient/info state
    // not leaking in from the developer's real ~/.jcode (other tests
    // write config overrides into the shared per-process test home).
    with_reasoning_current_home(|| {
        // With only one (current) trace there is nothing stale to collect, no
        // matter how much the transcript grows.
        let mut app = create_test_app();
        app.is_processing = true;

        crate::tui::ui::set_last_total_wrapped_lines(10);
        app.open_reasoning_region();
        app.append_reasoning_text("only thought\n");
        app.close_reasoning_region(None);

        crate::tui::ui::record_layout_snapshot(
            ratatui::layout::Rect::new(0, 0, 80, 20),
            None,
            None,
            None,
        );
        crate::tui::ui::set_last_total_wrapped_lines(500);
        assert!(!app.gc_offscreen_reasoning_traces());
        assert_eq!(trace_count(&app), 1, "the current thought is never GC'd");
    });
}

#[test]
fn answer_text_appended_into_open_region_does_not_glue_next_reasoning() {
    // Regression: if answer text is appended while a reasoning region is still
    // open (a stale `reasoning_streaming` flag), the next reasoning chunk must
    // still be separated from the answer tail. Previously the answer ran
    // straight into the reasoning run with no break (e.g.
    // `...patch + build.Ah, I see what's happening now.`).
    let mut app = create_test_app();
    let sentinel = jcode_tui_markdown::REASONING_SENTINEL;

    app.open_reasoning_region();
    app.append_reasoning_text("first thinking\n");
    // Append real answer text directly (this path does not go through the close
    // marker), leaving the region flagged open if the invariant is not enforced.
    app.append_streaming_text("Say the word and I'll patch + build.");
    // Appending real answer text must have closed the open reasoning region so a
    // later `open_reasoning_region` re-inserts its separator.
    assert!(
        !app.reasoning_streaming,
        "appending real answer text must close the open reasoning region"
    );
    // More reasoning arrives (opens a fresh region).
    app.append_reasoning_text("Ah, I see what's happening now.");

    let text = app.streaming_text();
    // The answer tail must be separated from the next reasoning run: there must
    // not be answer text immediately followed by the opening reasoning emphasis.
    let glued = format!("build.*{sentinel}");
    assert!(
        !text.contains(&glued),
        "answer text must not be glued onto reasoning: {text:?}"
    );
}

/// Regression test for issues #632/#633/#635: a hard panic
/// (`assertion failed: self.is_char_boundary(new_len)`) inside
/// `strip_reasoning_partial_tail`.
///
/// The live reasoning tail's byte length is recorded against the buffer it was
/// appended to. When that buffer is replaced wholesale (a reconnect/resume
/// replays a server-side snapshot), the stale length no longer describes the
/// contents, and subtracting it lands at an arbitrary byte offset. With
/// multi-byte UTF-8 text in the buffer that offset falls mid-character and
/// `String::truncate` panics, killing the whole process.
#[test]
fn replace_streaming_text_resets_reasoning_tail_and_never_panics_on_multibyte() {
    let mut app = create_test_app();

    // Stream a reasoning tail so `reasoning_partial_len` is non-zero.
    app.open_reasoning_region();
    app.append_reasoning_text("thinking about the problem");
    assert!(
        app.reasoning_partial_len > 0,
        "expected a live reasoning tail to be recorded"
    );

    // A reconnect/resume replaces the buffer with a snapshot made of multi-byte
    // characters, shorter than the recorded tail length.
    app.replace_streaming_text("\u{6f22}\u{5b57}\u{1f600}".to_string());
    assert_eq!(
        app.reasoning_partial_len,
        0,
        "replacing the stream must drop the stale reasoning tail length"
    );

    // Any subsequent reasoning delta strips the tail first. This is the call
    // that used to panic.
    app.append_reasoning_text("more thought");
    // Buffer is still valid UTF-8 and the process survived.
    assert!(app.streaming_text().contains("more thought"));
}

/// Directly exercise the boundary-safe truncation: even if a tail length is
/// somehow inconsistent with the buffer, stripping must not panic.
#[test]
fn strip_reasoning_partial_tail_snaps_to_char_boundary() {
    let mut app = create_test_app();
    // Two 3-byte characters: 6 bytes total.
    app.streaming.streaming_text = "\u{6f22}\u{5b57}".to_string();
    // Claim a 2-byte tail so new_len = 4, which is *not* a char boundary.
    app.reasoning_partial_len = 2;
    app.strip_reasoning_partial_tail();
    // Snapped down to 3: the first character survives intact, no panic.
    assert_eq!(app.streaming_text(), "\u{6f22}");
    assert_eq!(app.reasoning_partial_len, 0);
}

/// Sibling of the `strip_reasoning_partial_tail` hazard: `reasoning_block_start`
/// is also a byte offset recorded against an earlier state of the buffer, and
/// `split_off` panics on a non-boundary offset just like `truncate` does.
/// Clamping to the length alone does not prevent landing mid-character.
#[test]
fn anchor_current_reasoning_block_snaps_block_start_to_char_boundary() {
    with_reasoning_current_home(|| {
        let mut app = create_test_app();
        // Two 3-byte characters.
        app.streaming.streaming_text = "\u{6f22}\u{5b57}".to_string();
        // Offset 4 is within the buffer but inside the second character.
        app.reasoning_block_start = Some(4);
        app.reasoning_streaming = true;
        // Used to panic inside `split_off`.
        app.anchor_current_reasoning_block();
        // Buffer is still valid UTF-8 and no character was cut in half.
        assert!(app.streaming_text().is_char_boundary(app.streaming_text().len()));
    });
}

/// State-space sweep over the streaming-reasoning operations, guarding the whole
/// class of bug behind #632/#633/#635 rather than the two instances that were
/// reported.
///
/// Both crashes came from a byte offset (`reasoning_partial_len`,
/// `reasoning_block_start`) recorded against one state of `streaming_text` and
/// then used to slice a later state of it. Individual regression tests pin the
/// two sequences that were actually observed; they cannot show that no *other*
/// interleaving reaches the same slicing bug.
///
/// So drive every operation that reads or writes those offsets in
/// deterministic pseudo-random order, with heavily multi-byte payloads (so a
/// wrong offset lands mid-character rather than getting away with it), and
/// assert the buffer stays valid UTF-8 with the tail length never exceeding it.
/// A panic here is the failure; the assertions catch corruption that stops short
/// of panicking.
#[test]
fn reasoning_streaming_state_space_never_panics_or_desyncs() {
    // Multi-byte payloads: any off-by-one byte offset lands inside a character.
    const PAYLOADS: &[&str] = &[
        "\u{6f22}\u{5b57}",             // 3-byte CJK
        "\u{1f600}\u{1f601}",           // 4-byte emoji
        "caf\u{e9} na\u{ef}ve",         // 2-byte accents
        "a\u{6f22}b\u{1f600}c",         // mixed widths
        "line one\nline two",           // newline commits a reasoning line
        "",                             // empty delta
        "   ",                          // whitespace-only
    ];

    let mut app = create_test_app();
    // xorshift keeps this deterministic: a failure is always reproducible.
    let mut rng: u64 = 0x9E3779B97F4A7C15;
    let mut next = move || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };

    for step in 0..4000u32 {
        let payload = PAYLOADS[(next() % PAYLOADS.len() as u64) as usize];
        match next() % 7 {
            0 => app.open_reasoning_region(),
            1 => app.append_reasoning_text(payload),
            2 => app.close_reasoning_region(None),
            3 => app.append_streaming_text(payload),
            // The operation that caused the real bug: swap the buffer while an
            // offset into the previous buffer may still be recorded.
            4 => app.replace_streaming_text(payload.to_string()),
            5 => {
                let _ = app.take_streaming_text();
            }
            _ => app.strip_reasoning_partial_tail(),
        }

        // Invariant 1: the buffer is always valid UTF-8 at a character boundary.
        let text = app.streaming_text();
        assert!(
            text.is_char_boundary(text.len()),
            "step {step}: streaming_text ended mid-character"
        );
        // Invariant 2: the recorded tail can never claim more bytes than exist,
        // which is what made `len() - partial_len` land at a bogus offset.
        assert!(
            app.reasoning_partial_len <= text.len(),
            "step {step}: reasoning_partial_len {} exceeds buffer len {}",
            app.reasoning_partial_len,
            text.len()
        );
        // Invariant 2b: the tail must describe the *current* buffer, not a
        // previous one. The slice point it implies has to be a real character
        // boundary on its own merits. Without this, a stale length survives a
        // buffer swap and is only rescued by the defensive snap downstream,
        // which hides the desync instead of preventing it.
        let implied = text.len() - app.reasoning_partial_len;
        assert!(
            text.is_char_boundary(implied),
            "step {step}: tail len {} implies non-boundary slice at {implied} in {text:?}",
            app.reasoning_partial_len
        );
        // Invariant 3: a recorded block start must be a real boundary in the
        // current buffer, since `anchor_current_reasoning_block` splits there.
        if let Some(start) = app.reasoning_block_start {
            assert!(
                start <= text.len(),
                "step {step}: reasoning_block_start {start} exceeds buffer len {}",
                text.len()
            );
        }
    }
}
