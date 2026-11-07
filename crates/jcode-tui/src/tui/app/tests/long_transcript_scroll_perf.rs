// Scroll-frame cost over a very long transcript. Run with
// `-- long_transcript_scroll_perf --ignored --nocapture` to print timings.

fn long_transcript_app(messages: usize) -> crate::tui::app::App {
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::None;
    app.diagram_pane_enabled = false;
    let mut msgs = Vec::with_capacity(messages);
    for i in 0..messages {
        msgs.push(match i % 4 {
            0 => DisplayMessage::user(format!("question {i}: {}", "why ".repeat(10))),
            1 => DisplayMessage::assistant(format!(
                "## Answer {i}\n\nSome **bold** text and `code`.\n\n```rust\nfn f{i}() {{ println!(\"x\"); }}\n```\n\n- item a\n- item b\n\n{}",
                "lorem ipsum dolor sit amet ".repeat(30)
            )),
            2 => DisplayMessage::system(format!("system note {i}")),
            _ => DisplayMessage::assistant(format!("short reply {i} {}", "word ".repeat(40))),
        });
    }
    app.display_messages = msgs;
    app.bump_display_messages_version();
    app.status = ProcessingStatus::Idle;
    app.session.short_name = Some("test".to_string());
    app
}

#[test]
#[ignore]
fn long_transcript_scroll_perf() {
    let _lock = scroll_render_test_lock();
    crate::perf::pin_full_profile_for_tests();
    crate::tui::ui::prepare::set_prep_cache_enabled_for_tests(true);
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            crate::tui::ui::prepare::set_prep_cache_enabled_for_tests(false);
        }
    }
    let _reset = Reset;
    let mut app = long_transcript_app(4000);
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(140, 50)).unwrap();
    let t = std::time::Instant::now();
    terminal.draw(|f| crate::tui::ui::draw(f, &app)).unwrap();
    eprintln!("cold first frame: {:?}", t.elapsed());
    let t = std::time::Instant::now();
    terminal.draw(|f| crate::tui::ui::draw(f, &app)).unwrap();
    eprintln!("warm identical frame: {:?}", t.elapsed());

    let mut times = Vec::new();
    for _ in 0..300 {
        app.scroll_up(3);
        let t = std::time::Instant::now();
        terminal.draw(|f| crate::tui::ui::draw(f, &app)).unwrap();
        times.push(t.elapsed());
    }
    times.sort();
    eprintln!(
        "scroll frames: p50 {:?} p90 {:?} max {:?} total_lines {}",
        times[times.len() / 2],
        times[times.len() * 9 / 10],
        times[times.len() - 1],
        crate::tui::ui::last_total_wrapped_lines()
    );
}

fn compacted_marker(remaining: usize, visible: usize, total: usize, hidden: usize) -> DisplayMessage {
    DisplayMessage::system(format!(
        "Earlier conversation compacted - {remaining} older historical messages hidden. Showing {visible} of {total} compacted messages. Scroll to the top to load more. Hidden user prompts: {hidden}."
    ))
}

#[test]
#[ignore]
fn long_transcript_history_prepend_perf() {
    let _lock = scroll_render_test_lock();
    crate::perf::pin_full_profile_for_tests();
    crate::tui::ui::prepare::set_prep_cache_enabled_for_tests(true);
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            crate::tui::ui::prepare::set_prep_cache_enabled_for_tests(false);
        }
    }
    let _reset = Reset;
    let all = long_transcript_app(2000).display_messages;
    let prompts_in = |msgs: &[DisplayMessage]| msgs.iter().filter(|m| m.role == "user").count();
    let mut app = long_transcript_app(0);
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(140, 50)).unwrap();

    let mut visible = 200usize;
    let window = |visible: usize| {
        let start = all.len() - visible;
        let mut msgs = vec![compacted_marker(start, visible, all.len(), prompts_in(&all[..start]))];
        msgs.extend(all[start..].iter().cloned());
        (msgs, start)
    };
    let (msgs, start) = window(visible);
    app.apply_compacted_history_window(msgs, Vec::new(), all.len(), visible, start, prompts_in(&all[..start]));
    terminal.draw(|f| crate::tui::ui::draw(f, &app)).unwrap();
    while visible < all.len() {
        visible = (visible + 200).min(all.len());
        let (msgs, start) = window(visible);
        app.capture_history_anchor(0);
        app.apply_compacted_history_window(msgs, Vec::new(), all.len(), visible, start, prompts_in(&all[..start]));
        let t = std::time::Instant::now();
        terminal.draw(|f| crate::tui::ui::draw(f, &app)).unwrap();
        let (path, base) = crate::tui::ui::body_last_path_for_tests();
        eprintln!("prepend to {visible}: {:?} path={path} base={base:?}", t.elapsed());
        app.reconcile_history_anchor();
    }
}

#[test]
#[ignore]
fn long_transcript_streaming_while_scrolled_perf() {
    let _lock = scroll_render_test_lock();
    crate::perf::pin_full_profile_for_tests();
    crate::tui::ui::prepare::set_prep_cache_enabled_for_tests(true);
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            crate::tui::ui::prepare::set_prep_cache_enabled_for_tests(false);
        }
    }
    let _reset = Reset;
    let mut app = long_transcript_app(4000);
    app.remote_side_pane_images = vec![crate::session::RenderedImage {
        media_type: "image/png".to_string(),
        data: "x".repeat(200_000),
        label: Some("img".to_string()),
        source: crate::session::RenderedImageSource::UserInput,
        anchor: None,
        history_message_index: None,
    }];
    app.invalidate_side_pane_images_signature();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(140, 50)).unwrap();
    terminal.draw(|f| crate::tui::ui::draw(f, &app)).unwrap();
    app.scroll_up(500);
    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    let mut times = Vec::new();
    let mut text = String::new();
    for i in 0..100 {
        text.push_str(&format!("token{i} "));
        app.replace_streaming_text(text.clone());
        if i % 10 == 0 {
            app.scroll_up(1);
        }
        let t = std::time::Instant::now();
        terminal.draw(|f| crate::tui::ui::draw(f, &app)).unwrap();
        times.push(t.elapsed());
    }
    // A tool message lands while the reader is parked in history.
    app.replace_streaming_text(String::new());
    app.display_messages.push(DisplayMessage::assistant("done ".repeat(50)));
    app.bump_display_messages_version();
    let t = std::time::Instant::now();
    terminal.draw(|f| crate::tui::ui::draw(f, &app)).unwrap();
    let append = t.elapsed();
    times.sort();
    eprintln!(
        "streaming frames: p50 {:?} p90 {:?} max {:?}; append frame {:?} path={:?}",
        times[times.len() / 2],
        times[times.len() * 9 / 10],
        times[times.len() - 1],
        append,
        crate::tui::ui::body_last_path_for_tests()
    );
}

/// A compacted-history page arrives without tool durations (the History
/// protocol does not carry them), while the already-loaded tail had durations
/// filled in live. Loading older history must still reuse the prepared tail
/// instead of re-rendering the entire transcript on the scroll frame.
#[test]
fn history_prepend_reuses_tail_when_page_lacks_tool_durations() {
    let _lock = scroll_render_test_lock();
    crate::perf::pin_full_profile_for_tests();
    crate::tui::ui::prepare::set_prep_cache_enabled_for_tests(true);
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            crate::tui::ui::prepare::set_prep_cache_enabled_for_tests(false);
        }
    }
    let _reset = Reset;
    let tool_msg = |i: usize| {
        DisplayMessage::tool(
            format!("output {i}"),
            crate::message::ToolCall {
                id: format!("call_{i}"),
                name: "bash".to_string(),
                input: serde_json::json!({ "command": format!("echo {i}") }),
                intent: None,
                ..Default::default()
            },
        )
    };
    let all: Vec<DisplayMessage> = (0..120)
        .map(|i| match i % 3 {
            0 => DisplayMessage::user(format!("prompt {i}")),
            1 => tool_msg(i),
            _ => DisplayMessage::assistant(format!("reply {i} {}", "word ".repeat(30))),
        })
        .collect();
    let prompts_in = |msgs: &[DisplayMessage]| msgs.iter().filter(|m| m.role == "user").count();
    let window = |visible: usize| {
        let start = all.len() - visible;
        let mut msgs = vec![compacted_marker(start, visible, all.len(), prompts_in(&all[..start]))];
        msgs.extend(all[start..].iter().cloned());
        (msgs, start)
    };

    let mut app = long_transcript_app(0);
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).unwrap();
    let (mut msgs, start) = window(60);
    // The live tail learned durations from tool-finish events.
    for msg in msgs.iter_mut().filter(|m| m.role == "tool") {
        msg.duration_secs = Some(1.5);
    }
    app.apply_compacted_history_window(msgs, Vec::new(), all.len(), 60, start, prompts_in(&all[..start]));
    terminal.draw(|f| crate::tui::ui::draw(f, &app)).unwrap();

    let (msgs, start) = window(120);
    app.capture_history_anchor(0);
    app.apply_compacted_history_window(msgs, Vec::new(), all.len(), 120, start, prompts_in(&all[..start]));
    terminal.draw(|f| crate::tui::ui::draw(f, &app)).unwrap();
    let (path, reused) = crate::tui::ui::body_last_path_for_tests();
    assert_eq!(path, "suffix_reuse", "prepend fell back to a full transcript rebuild");
    assert!(
        reused.unwrap_or(0) >= 60,
        "only {reused:?} tail messages were reused; the loaded tail must be kept"
    );
    let tool = app
        .display_messages
        .iter()
        .rev()
        .find(|m| m.role == "tool")
        .unwrap();
    assert_eq!(tool.duration_secs, Some(1.5), "known tool durations must survive the reload");
}
