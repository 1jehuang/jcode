//! Exercise the public live draw entrypoint, not just the preview's helpers.
use super::*;
use crate::tui::session_picker::{
    PreviewMessage, ResumeTarget, SessionInfo, SessionPicker, SessionSource,
};
use ratatui::{Terminal, backend::TestBackend};

#[test]
fn shared_previous_prompt_band_fits_terminal_cells_without_splitting_graphemes() {
    let unicode = "界 👩‍💻 e\u{301} ".repeat(40);
    for width in 0u16..100 {
        for text in ["short prompt", unicode.as_str()] {
            let lines = viewport::prompt_preview_lines(4074, text, width, Alignment::Left);
            assert!((1..=2).contains(&lines.len()));
            for line in lines {
                assert!(
                    line.width() <= usize::from(width.saturating_sub(1)),
                    "width={width}: {line:?}"
                );
            }
        }
    }
}

#[test]
fn resumed_long_session_matches_live_transcript_and_numbered_composer() {
    let _guard = viewport_snapshot_test_lock();
    struct RestoreWidgets(bool);
    impl Drop for RestoreWidgets {
        fn drop(&mut self) {
            if info_widget::is_enabled() != self.0 {
                info_widget::toggle_enabled();
            }
        }
    }
    let enabled = info_widget::is_enabled();
    let _restore = RestoreWidgets(enabled);
    if enabled {
        info_widget::toggle_enabled();
    }
    let messages: Vec<_> = (0..60)
        .flat_map(|n| {
            [
                DisplayMessage {
                    role: "user".into(),
                    content: format!(
                        "Prompt {n} with Unicode 界 and a long request to verify wrapping"
                    ),
                    tool_calls: vec![],
                    duration_secs: None,
                    title: None,
                    tool_data: None,
                },
                DisplayMessage {
                    role: "assistant".into(),
                    content: format!(
                        "Answer {n}: **styled text** and `code`.\n\n- first item\n- second item"
                    ),
                    tool_calls: vec![],
                    duration_secs: None,
                    title: None,
                    tool_data: None,
                },
            ]
        })
        .collect();
    let now = chrono::Utc::now();
    let session = SessionInfo {
        id: "parity-no-disk-load".into(),
        parent_id: None,
        short_name: "fox".into(),
        icon: "🦊".into(),
        title: "Parity fixture".into(),
        message_count: messages.len(),
        user_message_count: 60,
        assistant_message_count: 60,
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
        status: Default::default(),
        needs_catchup: false,
        estimated_tokens: 0,
        first_user_prompt: None,
        messages_preview: messages
            .iter()
            .map(|m| PreviewMessage {
                role: m.role.clone(),
                content: m.content.clone(),
                tool_calls: m.tool_calls.clone(),
                duration_secs: m.duration_secs,
                title: m.title.clone(),
                tool_data: m.tool_data.clone(),
                timestamp: None,
            })
            .collect(),
        search_index: String::new(),
        server_name: None,
        server_icon: None,
        source: SessionSource::Jcode,
        resume_target: ResumeTarget::JcodeSession {
            session_id: "parity-no-disk-load".into(),
        },
        external_path: None,
    };
    for width in [40, 80, 120] {
        let height = 40;
        let state = TestState {
            display_messages: messages.clone(),
            messages_version: 1,
            chat_native_scrollbar: true,
            suppress_info_widgets: true,
            ..Default::default()
        };
        let mut live = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut preview = Terminal::new(TestBackend::new(width, height)).unwrap();
        reset_prompt_viewport_state_for_test();
        live.draw(|f| draw(f, &state)).unwrap();
        let mut picker = SessionPicker::new(vec![session.clone()]);
        preview.draw(|f| picker.render(f)).unwrap();
        // The compact card is confined to the right half. Compare all visible
        // conversation cells on the left, including top sticky prompt and input.
        let card_width = ((u32::from(width) * 36 / 100) as u16)
            .clamp(30, 52)
            .min(width.saturating_sub(12).max(1));
        let safe_width = width.saturating_sub(card_width + 2).min(width / 2);
        for y in 0..height - 1 {
            for x in 0..safe_width {
                assert_eq!(
                    preview.backend().buffer()[(x, y)],
                    live.backend().buffer()[(x, y)],
                    "live/preview difference at ({x},{y}), width={width}"
                );
            }
        }
    }
}
