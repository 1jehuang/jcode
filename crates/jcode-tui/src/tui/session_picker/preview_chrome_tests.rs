use super::*;
use ratatui::{Terminal, backend::TestBackend};

fn selected_session() -> SessionInfo {
    let now = chrono::Utc::now();
    SessionInfo {
        id: "selected".into(),
        parent_id: None,
        short_name: "fox".into(),
        icon: "🦊".into(),
        title: "Selected title".into(),
        message_count: 0,
        user_message_count: 0,
        assistant_message_count: 0,
        created_at: now,
        last_message_time: now,
        last_active_at: None,
        working_dir: Some("/selected/project".into()),
        model: Some("future-model".into()),
        provider_key: Some("selected-provider".into()),
        is_canary: false,
        is_debug: false,
        saved: false,
        save_label: None,
        status: SessionStatus::default(),
        needs_catchup: false,
        estimated_tokens: 999_999,
        first_user_prompt: None,
        messages_preview: vec![],
        search_index: String::new(),
        server_name: Some("cloud: dev-cloud".into()),
        server_icon: None,
        source: SessionSource::Jcode,
        resume_target: ResumeTarget::JcodeSession {
            session_id: "selected".into(),
        },
        external_path: None,
    }
}

#[test]
fn chrome_matches_shared_idle_composer_and_overscroll_buffers() {
    let session = selected_session();
    for width in [1, 4, 12, 40, 100] {
        let area = Rect::new(0, 0, width, 12);
        let mut actual = Terminal::new(TestBackend::new(width, 12)).unwrap();
        let mut expected = Terminal::new(TestBackend::new(width, 12)).unwrap();
        actual
            .draw(|frame| {
                draw(frame, area, &session, None);
            })
            .unwrap();
        expected
            .draw(|frame| {
                crate::tui::ui::input_ui::draw_preview_input(
                    frame,
                    Rect::new(0, 10, width, 1),
                    next_prompt_number(&session, None),
                );
                frame.render_widget(
                    Paragraph::new(crate::tui::ui::input_ui::preview_overscroll_line(
                        &selected_info(&session, None),
                        width,
                    )),
                    Rect::new(0, 11, width, 1),
                );
            })
            .unwrap();
        assert_eq!(
            actual.backend().buffer(),
            expected.backend().buffer(),
            "width={width}"
        );
    }
}

#[test]
fn cloud_preview_overscroll_shows_selected_host_without_live_telemetry() {
    let mut session = selected_session();
    session.server_icon = Some("☁".into());
    for width in [1, 12, 40, 100] {
        let mut terminal = Terminal::new(TestBackend::new(width, 12)).unwrap();
        terminal
            .draw(|frame| {
                draw(frame, Rect::new(0, 0, width, 12), &session, None);
            })
            .unwrap();
        let footer: String = (0..width)
            .map(|x| terminal.backend().buffer()[(x, 11)].symbol())
            .collect();
        if width == 100 {
            assert!(footer.contains("☁ dev-cloud:"), "{footer}");
            assert!(footer.contains("project"), "{footer}");
        }
        assert!(unicode_width::UnicodeWidthStr::width(footer.as_str()) <= width as usize);
    }
    session.server_icon = None;
    let mut terminal = Terminal::new(TestBackend::new(100, 12)).unwrap();
    terminal
        .draw(|frame| {
            draw(frame, Rect::new(0, 0, 100, 12), &session, None);
        })
        .unwrap();
    let footer: String = (0..100)
        .map(|x| terminal.backend().buffer()[(x, 11)].symbol())
        .collect();
    assert!(
        !footer.contains('☁'),
        "local rows must not inherit a cloud host: {footer}"
    );
}

#[test]
fn selected_facts_do_not_invent_live_telemetry() {
    let session = selected_session();
    let data = selected_info(&session, None);
    assert_eq!(data.model.as_deref(), Some("future-model"));
    assert_eq!(data.working_dir.as_deref(), Some("/selected/project"));
    assert!(data.usage_info.is_none());
    assert!(data.git_info.is_none());
    assert!(data.context_info.is_none());
    assert!(data.observed_context_tokens.is_none());
    assert!(data.background_info.is_none());
    assert!(data.swarm_info.is_none());
    assert!(matches!(data.auth_method, info_widget::AuthMethod::Unknown));
    let lines = crate::tui::ui::header::build_preview_header(&session, 100);
    assert!(
        lines
            .iter()
            .any(|line| line.to_string().contains("Cloud: dev-cloud"))
    );
}

#[test]
fn durable_reasoning_and_cap_are_retained_without_fake_context_usage() {
    let mut session = Session::create_with_id("selected".into(), None, None);
    session.reasoning_effort = Some("high".into());
    session.context_cap = Some(42_000);
    let data = persisted_info(&session);
    assert_eq!(data.reasoning_effort.as_deref(), Some("high"));
    assert_eq!(data.context_limit, Some(42_000));
    assert!(data.observed_context_tokens.is_none());
    assert!(data.context_info.is_none());
}

#[test]
fn persisted_session_widget_matches_real_margin_engine_and_buffer() {
    let _lock = crate::tui::ui::render_state_test_lock();
    let was_enabled = info_widget::is_enabled();
    if !was_enabled {
        info_widget::toggle_enabled();
    }
    let session = selected_session();
    let mut stored = Session::create_with_id(session.id.clone(), None, None);
    stored.short_name = Some(session.short_name.clone());
    let extras = PreviewExtras {
        info_data: Some(persisted_info(&stored)),
        ..Default::default()
    };
    let area = Rect::new(0, 0, 120, 30);
    let lines = vec![Line::from("short transcript")];
    let data = selected_info(&session, Some(&extras));
    let margins = crate::tui::ui::viewport::compute_visible_margins(&lines, &[], area, false);
    let placements = crate::tui::info_widget_layout::calculate_placements_anchored(
        area,
        &margins,
        &data,
        true,
        &[],
    );
    assert!(
        !placements.visible.is_empty(),
        "persisted session name must yield a real widget"
    );
    let mut actual = Terminal::new(TestBackend::new(120, 30)).unwrap();
    let mut expected = Terminal::new(TestBackend::new(120, 30)).unwrap();
    actual
        .draw(|frame| draw_widgets(frame, area, &lines, &session, Some(&extras), &[], 0))
        .unwrap();
    expected
        .draw(|frame| info_widget::render_all(frame, &placements.visible, &data))
        .unwrap();
    assert_eq!(actual.backend().buffer(), expected.backend().buffer());
    assert!(
        actual
            .backend()
            .buffer()
            .content
            .iter()
            .any(|cell| cell.symbol() != " ")
    );
    let image_regions = [jcode_tui_messages::ImageRegion {
        abs_line_idx: 0,
        end_line: 100,
        hash: 0,
        height: 100,
        width: 120,
        render: jcode_tui_messages::ImageRegionRender::Crop,
    }];
    let mut occupied = Terminal::new(TestBackend::new(120, 30)).unwrap();
    occupied
        .draw(|frame| {
            draw_widgets(
                frame,
                area,
                &[],
                &session,
                Some(&extras),
                &image_regions,
                10,
            )
        })
        .unwrap();
    assert!(
        occupied
            .backend()
            .buffer()
            .content
            .iter()
            .all(|cell| cell.symbol() == " "),
        "widgets must not dock over image rows after scrolling"
    );
    if !was_enabled {
        info_widget::toggle_enabled();
    }
}
