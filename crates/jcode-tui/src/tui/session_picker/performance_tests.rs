use super::*;
use std::time::{Duration, Instant};

fn session(id: usize, turns: usize) -> SessionInfo {
    let now = chrono::Utc::now();
    let messages_preview = (0..turns * 2).map(|i| PreviewMessage {
        role: if i % 2 == 0 { "user" } else { "assistant" }.into(),
        content: if i % 2 == 0 { format!("Prompt {i}: improve resume responsiveness") } else {
            format!("## Response {i}\n\n{}\n```rust\nlet answer = 42;\n```", "A paragraph with `inline code` and enough text to wrap across multiple columns.\n\n- a list entry with more text\n\n".repeat(6))
        },
        tool_calls: Vec::new(), tool_data: None, duration_secs: None, title: None, timestamp: None,
    }).collect();
    SessionInfo {
        id: format!("performance-{id}"),
        parent_id: None,
        short_name: format!("session-{id}"),
        icon: "🐒".into(),
        title: format!("Performance session {id}"),
        message_count: turns * 2,
        user_message_count: turns,
        assistant_message_count: turns,
        created_at: now,
        last_message_time: now,
        last_active_at: Some(now),
        working_dir: None,
        model: None,
        provider_key: None,
        is_canary: false,
        is_debug: false,
        saved: false,
        save_label: None,
        status: SessionStatus::Closed,
        needs_catchup: false,
        estimated_tokens: turns * 1000,
        first_user_prompt: Some("improve resume responsiveness".into()),
        messages_preview,
        search_index: String::new(),
        server_name: None,
        server_icon: None,
        source: SessionSource::Jcode,
        resume_target: ResumeTarget::JcodeSession {
            session_id: format!("performance-{id}"),
        },
        external_path: None,
    }
}

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort();
    samples[samples.len() / 2]
}

#[test]
#[ignore = "developer benchmark: stable viewport idle, loaded revisit, clone/hash attribution"]
fn benchmark_resume_stable_frames() {
    let mut picker = SessionPicker::new((0..6).map(|id| session(id, 80)).collect());
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).unwrap();
    for _ in 0..2 {
        for _ in 0..5 {
            terminal
                .draw(|f| picker.render_preview(f, f.area()))
                .unwrap();
            picker.next();
        }
        terminal
            .draw(|f| picker.render_preview(f, f.area()))
            .unwrap();
        for _ in 0..5 {
            picker.previous();
        }
    }
    let mut idle = Vec::new();
    let mut revisit = Vec::new();
    let mut clone_cost = Vec::new();
    let mut hash_cost = Vec::new();
    let mut accounting_cost = Vec::new();
    for i in 0..40 {
        let start = Instant::now();
        terminal
            .draw(|f| picker.render_preview(f, f.area()))
            .unwrap();
        idle.push(start.elapsed());
        if i % 2 == 0 {
            picker.next();
        } else {
            picker.previous();
        }
        let start = Instant::now();
        terminal
            .draw(|f| picker.render_preview(f, f.area()))
            .unwrap();
        revisit.push(start.elapsed());
        let selected = picker.selected_session().unwrap();
        let start = Instant::now();
        std::hint::black_box(selected.clone());
        clone_cost.push(start.elapsed());
        let start = Instant::now();
        std::hint::black_box(
            picker.preview_content_hash(selected, crate::config::config().display.diff_mode),
        );
        hash_cost.push(start.elapsed());
        let start = Instant::now();
        std::hint::black_box(picker.preview_cache.as_ref().unwrap().estimated_bytes());
        accounting_cost.push(start.elapsed());
    }
    eprintln!(
        "stable preview 80 turns: idle={:.0}us revisit={:.0}us clone={:.0}us full_hash={:.0}us accounting_scan={:.0}us",
        median(idle).as_secs_f64() * 1e6,
        median(revisit).as_secs_f64() * 1e6,
        median(clone_cost).as_secs_f64() * 1e6,
        median(hash_cost).as_secs_f64() * 1e6,
        median(accounting_cost).as_secs_f64() * 1e6
    );
}

fn draw_preview(
    picker: &mut SessionPicker,
    terminal: &mut ratatui::Terminal<ratatui::backend::TestBackend>,
) {
    terminal
        .draw(|frame| picker.render_preview(frame, frame.area()))
        .unwrap();
}

#[test]
fn prepared_preview_revisits_reuse_allocations_and_pixels() {
    let mut picker = SessionPicker::new(vec![session(0, 8), session(1, 8)]);
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
    draw_preview(&mut picker, &mut terminal);
    let original = terminal.backend().buffer().clone();
    let allocation = picker
        .preview_cache
        .as_ref()
        .unwrap()
        .wrapped_lines
        .as_ptr();
    let key = picker.preview_cache.as_ref().unwrap().key.clone();
    picker.next();
    draw_preview(&mut picker, &mut terminal);
    picker.previous();
    draw_preview(&mut picker, &mut terminal);
    let cached = picker.preview_cache.as_ref().unwrap();
    assert!(cached.key == key);
    assert_eq!(
        cached.wrapped_lines.as_ptr(),
        allocation,
        "revisits must not rebuild or clone wrapped lines"
    );
    assert_eq!(terminal.backend().buffer(), &original);
    draw_preview(&mut picker, &mut terminal);
    assert_eq!(
        picker
            .preview_cache
            .as_ref()
            .unwrap()
            .wrapped_lines
            .as_ptr(),
        allocation
    );
}

#[test]
fn memoized_transcript_hash_invalidates_on_load_mutation_and_reseed() {
    let mut picker = SessionPicker::new(vec![session(0, 2)]);
    let mode = crate::config::config().display.diff_mode;
    let selected = picker.selected_session_ref().unwrap();
    let original = picker.selected_preview_content_hash(selected, mode);
    assert_eq!(
        original,
        picker.preview_content_hash(picker.selected_session().unwrap(), mode)
    );
    picker
        .session_by_ref_mut(selected)
        .unwrap()
        .messages_preview[0]
        .content
        .push_str(" changed");
    let changed = picker.selected_preview_content_hash(selected, mode);
    assert_ne!(changed, original);
    assert_eq!(
        changed,
        picker.preview_content_hash(picker.selected_session().unwrap(), mode)
    );
    picker.preview_extras.insert(
        "performance-0".into(),
        PreviewExtras {
            hidden_prompts: 9,
            ..Default::default()
        },
    );
    assert_ne!(
        changed,
        picker.selected_preview_content_hash(selected, mode),
        "extras remain dynamic"
    );
    let mut preview = picker.selected_session().unwrap().messages_preview.clone();
    preview[1].content = "a loaded transcript replacement".into();
    picker.apply_session_preview("performance-0", preview);
    assert_eq!(
        picker.selected_preview_content_hash(selected, mode),
        picker.preview_content_hash(picker.selected_session().unwrap(), mode)
    );
    picker.reseed_grouped(Vec::new(), vec![session(1, 2)]);
    let selected = picker.selected_session_ref().unwrap();
    assert_eq!(
        picker.selected_preview_content_hash(selected, mode),
        picker.preview_content_hash(picker.selected_session().unwrap(), mode)
    );
    assert_ne!(
        picker.selected_preview_content_hash(selected, mode),
        original
    );
}

fn tiny_cache(id: u64, bytes: usize) -> PreviewRenderCache {
    PreviewRenderCache {
        key: PreviewCacheKey {
            content_hash: id,
            inner_width: 80,
            inner_height: 24,
            diff_mode: crate::config::DiffDisplayMode::Off,
            search_query: String::new(),
        },
        wrapped_lines: vec![Line::from("x".repeat(bytes))],
        image_regions: Vec::new(),
        user_prompt_markers: Vec::new(),
        show_scrollbar: false,
        first_match_line: None,
    }
}

#[test]
fn retained_previews_have_byte_and_entry_bounds_and_evict_lru() {
    use performance::{RETAINED_PREVIEW_BYTES, RETAINED_PREVIEW_ENTRIES};
    let mut picker = SessionPicker::new(vec![session(0, 1)]);
    for id in 0..30 {
        picker.install_preview(tiny_cache(id, 100));
        assert!(picker.preview_reuse.retained_entries() <= RETAINED_PREVIEW_ENTRIES);
        assert!(picker.preview_reuse.retained_bytes() <= RETAINED_PREVIEW_BYTES);
    }
    assert!(!picker.reuse_preview(&tiny_cache(0, 0).key));
    assert!(picker.reuse_preview(&tiny_cache(28, 0).key));
    assert!(picker.reuse_preview(&tiny_cache(29, 0).key));
    assert_eq!(
        picker.preview_reuse.retained_bytes(),
        RETAINED_PREVIEW_ENTRIES * tiny_cache(1, 100).estimated_bytes()
    );
    for id in 30..40 {
        picker.install_preview(tiny_cache(id, 5 * 1024 * 1024));
        assert!(picker.preview_reuse.retained_bytes() <= RETAINED_PREVIEW_BYTES);
    }
    assert_eq!(
        picker.preview_reuse.retained_bytes(),
        3 * tiny_cache(1, 5 * 1024 * 1024).estimated_bytes()
    );
    picker.install_preview(tiny_cache(100, RETAINED_PREVIEW_BYTES + 1));
    picker.install_preview(tiny_cache(101, 100));
    assert!(
        !picker.reuse_preview(&tiny_cache(100, 0).key),
        "oversized active previews must not be retained"
    );
    let profile = picker.debug_memory_profile();
    assert_eq!(
        profile["retained_preview_bytes"].as_u64().unwrap() as usize,
        picker.preview_reuse.retained_bytes()
    );
}

#[test]
fn session_list_stripes_cover_whole_entries() {
    let mut picker = SessionPicker::new((0..3).map(|id| session(id, 1)).collect());
    picker.list_state.select(None);
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
    terminal
        .draw(|f| picker.render_session_list_impl(f, f.area(), true))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let mut backgrounds = Vec::new();
    for y in 1..29 {
        let text: String = (1..99).map(|x| buffer[(x, y)].symbol()).collect();
        if text.contains("Performance session") {
            let bg = buffer[(98, y)].bg;
            // All five lines, including the trailing spacer, belong to the stripe.
            for row in y..y + 5 {
                assert_eq!(buffer[(1, row)].bg, bg);
                assert_eq!(buffer[(98, row)].bg, bg);
            }
            backgrounds.push(bg);
        }
    }
    assert_eq!(backgrounds.len(), 3);
    assert_eq!(backgrounds[0], backgrounds[2]);
    assert_ne!(backgrounds[0], backgrounds[1]);
    assert_eq!(backgrounds[1], rgb(28, 30, 34));
}

#[test]
fn virtualized_session_list_matches_full_ratatui_list() {
    let sessions: Vec<_> = (0..18)
        .map(|id| {
            let mut item = session(id, 1);
            if id % 3 == 0 {
                item.first_user_prompt = None;
            }
            if id % 4 == 0 {
                item.status = SessionStatus::Crashed {
                    message: Some("reason".into()),
                };
            }
            item
        })
        .collect();
    let mut actual = SessionPicker::new(sessions.clone());
    let mut expected = SessionPicker::new(sessions);
    // Include one-row headers among variable-height prompt/crash entries.
    actual
        .items
        .insert(0, PickerItem::SavedHeader { session_count: 18 });
    actual.item_to_session.insert(0, None);
    expected
        .items
        .insert(0, PickerItem::SavedHeader { session_count: 18 });
    expected.item_to_session.insert(0, None);
    for width in [1, 2, 3, 18, 48] {
        for height in [1, 2, 3, 4, 7, 14, 40] {
            let mut a =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
            let mut b =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
            for offset in [0, 4, 17] {
                for selected in [None, Some(1), Some(6), Some(18)] {
                    actual.list_state = ListState::default()
                        .with_offset(offset)
                        .with_selected(selected);
                    expected.list_state = actual.list_state.clone();
                    a.draw(|f| actual.render_session_list_impl(f, f.area(), true))
                        .unwrap();
                    b.draw(|f| expected.render_session_list_impl(f, f.area(), false))
                        .unwrap();
                    assert_eq!(
                        a.backend().buffer(),
                        b.backend().buffer(),
                        "width={width} height={height} offset={offset} selected={selected:?}"
                    );
                    assert_eq!(
                        actual.list_state, expected.list_state,
                        "width={width} height={height} offset={offset} selected={selected:?}"
                    );
                }
            }
        }
    }
}
