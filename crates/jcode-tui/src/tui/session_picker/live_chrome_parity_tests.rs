//! Compare the preview's data adapter to the real live status drawing entrypoint.
use super::*;
use ratatui::{Terminal, backend::TestBackend};

#[test]
fn picker_history_uses_identical_live_message_rendering_and_wrapping() {
    let _lock = viewport_snapshot_test_lock();
    let mut messages = Vec::new();
    for (role, content) in [
        (
            "user",
            "A long prompt with Unicode 界 and emoji 🦔 that must wrap exactly like the live transcript\r\nsecond source line",
        ),
        (
            "assistant",
            "Here is **bold text** and `code`.\n\n- a list item\n- another list item",
        ),
        ("meta", "Session metadata"),
        ("system", "A saved system notice"),
        ("usage", "55s · 18.8 tps · ↑26K ↓45"),
        ("error", "First diagnostic\nSecond diagnostic"),
        ("user", "<transcription>Spoken prompt</transcription>"),
        ("spacer", "3"),
    ] {
        messages.push(DisplayMessage {
            role: role.into(),
            content: content.into(),
            tool_calls: Vec::new(),
            duration_secs: Some(2.3),
            title: Some("Saved title".into()),
            tool_data: None,
        });
    }
    messages.insert(
        1,
        DisplayMessage {
            role: "tool".into(),
            content: "Build succeeded".into(),
            tool_calls: Vec::new(),
            duration_secs: Some(2.3),
            title: None,
            tool_data: Some(crate::message::ToolCall {
                id: "parity-tool".into(),
                name: "bash".into(),
                input: serde_json::json!({"command": "echo hello"}),
                intent: Some("Check the preview".into()),
                thought_signature: None,
            }),
        },
    );
    for width in [4, 12, 40, 80, 120] {
        let state = TestState {
            display_messages: messages.clone(),
            centered_mode: false,
            compacted_hidden_user_prompts: 9,
            ..Default::default()
        };
        let live = prepare::prepare_body(&state, width, false);
        let preview = prepare::prepare_history_preview(&messages, 9, width, state.diff_mode());
        assert_eq!(live.wrapped_lines, preview.wrapped_lines, "width={width}");
        assert_eq!(
            live.wrapped_user_prompt_starts,
            preview.wrapped_user_prompt_starts
        );
        assert_eq!(live.user_prompt_texts, preview.user_prompt_texts);
    }
}

#[test]
fn picker_overscroll_matches_real_live_status_with_same_session_facts() {
    let _lock = viewport_snapshot_test_lock();
    let data = info_widget::InfoWidgetData {
        model: Some("future-model".into()),
        provider_name: Some("selected-provider".into()),
        working_dir: Some("/selected/project".into()),
        reasoning_effort: Some("high".into()),
        ..Default::default()
    };
    let state = TestState {
        provider_model: data.model.clone(),
        provider_name: data.provider_name.clone(),
        working_dir: data.working_dir.clone(),
        info_widget_data: data.clone(),
        ..Default::default()
    };
    for width in [1, 4, 12, 40, 100] {
        let mut live = Terminal::new(TestBackend::new(width, 1)).unwrap();
        let mut preview = Terminal::new(TestBackend::new(width, 1)).unwrap();
        live.draw(|frame| {
            let area = frame.area();
            input_ui::draw_overscroll_status(frame, &state, area);
        })
        .unwrap();
        preview
            .draw(|frame| {
                frame.render_widget(
                    Paragraph::new(input_ui::preview_overscroll_line(&data, width)),
                    frame.area(),
                );
            })
            .unwrap();
        assert_eq!(
            live.backend().buffer(),
            preview.backend().buffer(),
            "width={width}"
        );
    }
}
