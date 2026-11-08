use super::*;
use std::time::{Duration, Instant};

pub(super) fn session(id: usize, turns: usize) -> SessionInfo {
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
        todo_statuses: Vec::new(),
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
fn cold_preview_viewport_matches_full_wide_then_narrow_layout() {
    let mode = crate::config::DiffDisplayMode::Inline;
    for turns in [0, 1, 8] {
        let item = session(0, turns);
        let picker = SessionPicker::new(vec![item.clone()]);
        let messages: Vec<DisplayMessage> = item
            .messages_preview
            .iter()
            .map(|msg| DisplayMessage {
                role: msg.role.clone(),
                content: msg.content.clone(),
                tool_calls: msg.tool_calls.clone(),
                tool_data: msg.tool_data.clone(),
                title: msg.title.clone(),
                duration_secs: msg.duration_secs,
            })
            .collect();
        for width in [1, 2, 3, 20, 80, 120] {
            for height in [0, 1, 2, 5, 30, 200] {
                let area = Rect::new(0, 0, width, height);
                let actual = picker.build_preview_cache(
                    &item,
                    area,
                    area,
                    tiny_cache(0, 0).key,
                    Alignment::Left,
                    mode,
                );
                // Wrapping and chrome belong to the real chat renderer now.
                let roles: Vec<_> = actual.messages.iter().map(|m| m.role.clone()).collect();
                let expected: Vec<_> = if messages.is_empty() {
                    vec!["system".to_string()]
                } else {
                    messages.iter().map(|m| m.role.clone()).collect()
                };
                assert_eq!(
                    roles, expected,
                    "turns={turns} width={width} height={height}"
                );
            }
        }
    }
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
        Some(picker.preview_content_hash(picker.selected_session().unwrap(), mode))
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
        Some(picker.preview_content_hash(picker.selected_session().unwrap(), mode))
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
        Some(picker.preview_content_hash(picker.selected_session().unwrap(), mode))
    );
    picker.reseed_grouped(Vec::new(), vec![session(1, 2)]);
    let selected = picker.selected_session_ref().unwrap();
    assert_eq!(
        picker.selected_preview_content_hash(selected, mode),
        Some(picker.preview_content_hash(picker.selected_session().unwrap(), mode))
    );
    assert_ne!(
        picker.selected_preview_content_hash(selected, mode),
        original
    );
}

#[test]
fn stale_preview_selection_renders_empty_and_does_not_fingerprint() {
    let mut picker = SessionPicker::new(vec![session(0, 2)]);
    let selected = picker.selected_session_ref().unwrap();
    picker.all_sessions.clear();
    let mode = crate::config::config().display.diff_mode;
    assert_eq!(picker.selected_preview_content_hash(selected, mode), None);
    assert!(picker.preview_fingerprints.borrow().is_empty());
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
    draw_preview(&mut picker, &mut terminal);
    let text: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(text.contains("No session selected"));
    assert!(picker.preview_cache.is_none());
}

#[test]
fn missing_preview_is_a_cache_miss_and_rebuilds_on_render() {
    let mut picker = SessionPicker::new(vec![session(0, 2)]);
    assert!(!picker.reuse_preview(&tiny_cache(42, 0).key));
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
    draw_preview(&mut picker, &mut terminal);
    let key = picker.preview_cache.as_ref().unwrap().key.clone();
    assert!(picker.reuse_preview(&key));
    picker.preview_cache = None;
    draw_preview(&mut picker, &mut terminal);
    assert!(picker.preview_cache.as_ref().unwrap().key == key);
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
        messages: Vec::new(),
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
fn self_dev_entries_keep_here_marker_on_directory_row() {
    let mut item = session(0, 1);
    item.is_canary = true;
    item.working_dir = Some("/project/picker-layout".into());
    let mut picker = SessionPicker::new(vec![item.clone()]);
    picker.current_dir = item.working_dir.clone();
    for selected in [false, true] {
        let rows = picker.render_session_item_lines(&item, selected);
        assert!(!rows[0].to_string().contains("🔬"));
        assert!(!rows[0].to_string().contains("▸ here"));
        assert!(rows[1].to_string().contains("picker-layout  ▸ here"));
        assert!(rows[1].spans.last().unwrap().content.contains("ago"));
    }
    picker.current_dir = Some("/different-project".into());
    picker.current_repo = None;
    let rows = picker.render_session_item_lines(&item, false);
    assert!(!rows[1].to_string().contains("▸ here"));
}

#[test]
fn compact_session_entries_keep_context_without_spacer_rows() {
    let mut item = session(0, 1);
    item.working_dir = Some("/project/resume-layout".into());
    let picker = SessionPicker::new(vec![item.clone()]);
    let rows = picker.render_session_item_lines(&item, false);
    assert_eq!(rows.len(), 2);
    assert!(!rows[0].to_string().contains("tok"));
    assert!(rows[1].to_string().contains("resume-layout"));
    assert!(rows[1].to_string().contains("ago"));
    assert!(!rows[1].to_string().contains("active"));
    assert!(
        !rows[1]
            .to_string()
            .contains("improve resume responsiveness")
    );
    assert!(!rows[1].to_string().contains("messages"));

    item.first_user_prompt = Some("  \n ".into());
    let rows = picker.render_session_item_lines(&item, false);
    assert_eq!(rows.len(), 2);
    assert!(rows[1].to_string().contains("resume-layout"));

    item.working_dir = None;
    let rows = picker.render_session_item_lines(&item, false);
    assert_eq!(rows.len(), 2);
    assert!(rows[1].to_string().contains("No directory"));
    assert!(!rows[0].to_string().contains("tok"));

    item.status = SessionStatus::Crashed {
        message: Some("connection failed".into()),
    };
    let rows = picker.render_session_item_lines(&item, false);
    assert_eq!(
        rows.len(),
        3,
        "crash reasons remain visible, not hidden for density"
    );
    assert!(rows[2].to_string().contains("connection failed"));
    assert!(rows.iter().all(|row| !row.to_string().trim().is_empty()));
}

#[test]
fn resume_todo_dots_follow_item_statuses() {
    let mut item = session(0, 1);
    item.todo_statuses = ["completed", "in_progress", "pending", "cancelled"]
        .into_iter()
        .map(str::to_string)
        .collect();
    let dots = SessionPicker::todo_dot_spans(&item, 20);
    assert_eq!(dots.len(), 4);
    for (dot, color) in dots.iter().zip([
        rgb(129, 199, 132),
        rgb(255, 213, 79),
        rgb(140, 140, 140),
        rgb(80, 80, 80),
    ]) {
        assert_eq!(dot.content, "●");
        assert_eq!(dot.style.fg, Some(color));
    }
    assert_eq!(SessionPicker::todo_dot_spans(&item, 2)[1].content, "…");
    assert!(SessionPicker::todo_dot_spans(&item, 0).is_empty());
    item.todo_statuses.clear();
    assert!(SessionPicker::todo_dot_spans(&item, 20).is_empty());
}

#[test]
fn session_entry_reserves_todo_dots_and_activity_for_long_names_and_directories() {
    let mut item = session(0, 1);
    item.title = "界".repeat(60);
    item.working_dir = Some(format!("/{}", "界".repeat(80)));
    item.todo_statuses = vec!["completed".into(); 5];
    item.last_active_at = Some(chrono::Utc::now());
    let mut picker = SessionPicker::new(vec![item]);
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(48, 12)).unwrap();
    terminal
        .draw(|f| picker.render_session_list_impl(f, f.area(), true))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let rows: Vec<String> = (1..11)
        .filter(|&y| buffer[(1, y)].symbol() == "▌")
        .map(|y| (2..47).map(|x| buffer[(x, y)].symbol()).collect())
        .collect();
    assert_eq!(rows.len(), 2);
    assert!(rows[0].trim_end().ends_with("●●●●●"), "{}", rows[0]);
    assert!(!rows[0].contains("tok"));
    assert!(rows[1].contains("ago"), "{}", rows[1]);
    assert!(rows.iter().all(|row| row.contains('…')));
}

#[test]
fn compact_session_list_fits_ten_sessions_in_twenty_two_rows() {
    let mut picker = SessionPicker::new((0..20).map(|id| session(id, 1)).collect());
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(48, 24)).unwrap();
    terminal
        .draw(|f| picker.render_session_list_impl(f, f.area(), true))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let count = (1..23)
        .filter(|&y| {
            let text: String = (1..47).map(|x| buffer[(x, y)].symbol()).collect();
            text.contains("Performance session")
        })
        .count();
    assert!(
        count >= 10,
        "expected at least ten compact sessions, got {count}"
    );
}

#[test]
fn compact_rows_truncate_wide_text_without_wrapping() {
    for width in [8, 20, 42] {
        let mut item = session(0, 1);
        item.title = "界".repeat(60);
        item.todo_statuses = vec!["pending".into(); 1_000];
        item.working_dir = Some(format!("/{}", "界".repeat(80)));
        let mut picker = SessionPicker::new(vec![item]);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, 12)).unwrap();
        terminal
            .draw(|f| picker.render_session_list_impl(f, f.area(), true))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let selected_rows: Vec<_> = (1..11)
            .filter(|&y| buffer[(1, y)].symbol() == "▌")
            .collect();
        assert_eq!(
            selected_rows.len(),
            2,
            "entries never wrap at width {width}"
        );
        for y in selected_rows {
            // A double-width glyph may leave one spare cell before the border.
            assert!(
                (1..width - 1).any(|x| buffer[(x, y)].symbol() == "…"),
                "missing truncation marker: width={width}, row={y}"
            );
        }
    }
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
            // Both compact lines share one full-width session stripe.
            for row in y..y + 2 {
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
fn session_selection_has_stable_rail_and_blended_background() {
    let mut picker = SessionPicker::new((0..3).map(|id| session(id, 1)).collect());
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
    let indices: Vec<_> = picker
        .item_to_session
        .iter()
        .enumerate()
        .filter_map(|(i, session)| session.map(|_| i))
        .collect();
    let mut positions = Vec::new();
    for selected in indices {
        picker.list_state.select(Some(selected));
        terminal
            .draw(|f| picker.render_session_list_impl(f, f.area(), true))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let mut titles = Vec::new();
        let mut rail_rows = 0;
        for y in 1..29 {
            if buffer[(1, y)].symbol() == "▌" {
                rail_rows += 1;
                assert_eq!(buffer[(1, y)].bg, rgb(36, 43, 42));
                assert_eq!(buffer[(98, y)].bg, rgb(36, 43, 42));
            }
            let text: String = (1..99).map(|x| buffer[(x, y)].symbol()).collect();
            if text.contains("Performance session") {
                let x = (1..99).find(|&x| buffer[(x, y)].symbol() == "P").unwrap();
                titles.push((x, y));
            }
        }
        assert_eq!(rail_rows, 2);
        assert_eq!(titles.len(), 3);
        positions.push(titles);
    }
    assert!(positions.windows(2).all(|pair| pair[0] == pair[1]));
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
