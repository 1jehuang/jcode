use super::*;
use ratatui::{Terminal, backend::TestBackend};

fn session() -> SessionInfo {
    let now = chrono::Utc::now();
    SessionInfo {
        id: "composer".into(),
        parent_id: None,
        short_name: "fox".into(),
        icon: "🦊".into(),
        title: "Composer".into(),
        message_count: 99,
        user_message_count: 8,
        assistant_message_count: 91,
        created_at: now,
        last_message_time: now,
        last_active_at: None,
        working_dir: None,
        model: None,
        provider_key: None,
        is_canary: false,
        is_debug: false,
        saved: false,
        save_label: None,
        status: SessionStatus::default(),
        needs_catchup: false,
        estimated_tokens: 0,
        first_user_prompt: None,
        messages_preview: vec![],
        search_index: String::new(),
        server_name: None,
        server_icon: None,
        source: SessionSource::Jcode,
        resume_target: ResumeTarget::JcodeSession {
            session_id: "composer".into(),
        },
        external_path: None,
    }
}

fn message(role: &str, content: &str) -> PreviewMessage {
    PreviewMessage {
        role: role.into(),
        content: content.into(),
        tool_calls: vec![],
        tool_data: None,
        duration_secs: None,
        title: None,
        timestamp: None,
    }
}

#[test]
fn numbering_uses_full_user_count_without_double_counting_hidden_window() {
    let mut session = session();
    session.messages_preview = vec![message("user", "last prompt")];
    let extras = PreviewExtras {
        hidden_prompts: 7,
        ..Default::default()
    };
    assert_eq!(next_prompt_number(&session, Some(&extras)), 9);
    assert_eq!(next_prompt_number(&session, None), 9);
    session.user_message_count = 0;
    session.messages_preview.push(message("assistant", "reply"));
    session.messages_preview.push(message("user", "   "));
    assert_eq!(next_prompt_number(&session, Some(&extras)), 9);
    session.user_message_count = usize::MAX;
    assert_eq!(next_prompt_number(&session, Some(&extras)), usize::MAX);
}

#[test]
fn preview_composer_matches_live_wrapper_styles_and_narrow_width_behavior() {
    for width in [0, 1, 3, 4, 8, 80] {
        let mut actual = Terminal::new(TestBackend::new(84, 2)).unwrap();
        let mut expected = Terminal::new(TestBackend::new(84, 2)).unwrap();
        let area = Rect::new(2, 1, width, 1);
        actual
            .draw(|frame| {
                crate::tui::ui::input_ui::draw_preview_input(frame, area, 9);
            })
            .unwrap();
        expected
            .draw(|frame| {
                if width > 3 {
                    let (lines, _, _) = crate::tui::ui::input_ui::wrap_input_text(
                        "",
                        0,
                        width as usize - 3,
                        "9",
                        "> ",
                        jcode_tui_style::theme::user_color(),
                        3,
                    );
                    frame.render_widget(Paragraph::new(lines), area);
                }
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
fn preview_reserves_idle_status_composer_and_footer_without_losing_last_transcript_row() {
    for height in 0..16 {
        let mut terminal = Terminal::new(TestBackend::new(60, 18)).unwrap();
        let area = Rect::new(2, 1, 50, height);
        let session = session();
        terminal
            .draw(|frame| {
                let transcript = draw(frame, area, &session, None);
                assert!(transcript.y >= area.y);
                assert!(transcript.bottom() <= area.bottom());
                if height > 0 {
                    assert!(transcript.height >= 1);
                }
                if height >= 8 {
                    assert_eq!(transcript.bottom(), area.bottom() - 3);
                    let cell = &frame.buffer_mut()[(area.x, area.bottom() - 2)];
                    assert_eq!(cell.symbol(), "9");
                }
            })
            .unwrap();
    }
}
