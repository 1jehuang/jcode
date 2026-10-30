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
fn chrome_matches_shared_header_and_overscroll_buffers() {
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
                let lines = crate::tui::ui::header::build_preview_header(&session, width);
                let height = lines.len() as u16;
                frame.render_widget(Paragraph::new(lines), Rect { height, ..area });
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
            .any(|line| line.to_string().contains("cloud: dev-cloud"))
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
