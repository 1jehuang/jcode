use super::*;
use ratatui::backend::TestBackend;
use ratatui::{Terminal, layout::Rect};

/// Render the inline interactive picker for the given state and return the
/// per-row text of the whole buffer.
fn render_inline_picker(state: &TestState, width: u16, height: u16) -> Vec<String> {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("failed to create test terminal");
    terminal
        .draw(|frame| {
            let area = Rect::new(0, 0, width, height);
            crate::tui::ui::inline_interactive_ui::draw_inline_interactive(frame, state, area);
        })
        .expect("failed to draw inline picker");

    let buf = terminal.backend().buffer();
    let mut lines = Vec::with_capacity(height as usize);
    for y in 0..height {
        let mut line = String::with_capacity(width as usize);
        for x in 0..width {
            line.push_str(buf[(x, y)].symbol());
        }
        lines.push(line.trim_end().to_string());
    }
    lines
}

fn model_picker_entry() -> crate::tui::PickerEntry {
    crate::tui::PickerEntry {
        name: "gpt-5.4".to_string(),
        options: vec![crate::tui::PickerOption {
            provider: "openai".to_string(),
            api_method: "oauth".to_string(),
            available: true,
            detail: String::new(),
            estimated_reference_cost_micros: None,
        }],
        action: crate::tui::PickerAction::Model,
        selected_option: 0,
        is_current: true,
        is_default: false,
        is_favorite: false,
        recommended: true,
        recommendation_rank: 0,
        usage_score: 0,
        old: false,
        created_date: None,
        effort: None,
    }
}

fn model_picker_state() -> TestState {
    TestState {
        inline_interactive_state: Some(crate::tui::InlineInteractiveState {
            scoped_route_restore: Vec::new(),
            effort_step: None,
            kind: crate::tui::PickerKind::Model,
            filtered: vec![0],
            selected: 0,
            column: 0,
            filter: String::new(),
            preview: false,
            entries: vec![model_picker_entry()],
        }),
        ..Default::default()
    }
}

#[test]
fn model_picker_hotkey_hint_renders_above_the_box() {
    let state = model_picker_state();
    let lines = render_inline_picker(&state, 80, 12);

    // The first non-empty row should be the hotkey hint, and it must sit ABOVE
    // the rounded top border of the picker box (which starts with '╭').
    let hint_row = lines
        .iter()
        .position(|line| line.contains("Ctrl+N favorite"))
        .unwrap_or_else(|| panic!("hotkey hint should be rendered:\n{}", lines.join("\n")));
    let border_row = lines
        .iter()
        .position(|line| line.contains('╭'))
        .expect("picker box top border should be rendered");

    assert!(
        hint_row < border_row,
        "hint (row {hint_row}) should appear above the box top border (row {border_row}):\n{}",
        lines.join("\n")
    );
    // The hint should not be enclosed by the border characters.
    assert!(
        !lines[hint_row].contains('│'),
        "hint row should be outside the box border:\n{}",
        lines[hint_row]
    );
}

fn render_model_suggestions(
    state: &TestState,
    width: u16,
    height: u16,
    input_y: u16,
) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| {
            let input = Rect::new(0, input_y, width, 1);
            crate::tui::ui::input_ui::draw_command_suggestions_overlay(frame, state, input);
        })
        .unwrap();
    let buf = terminal.backend().buffer();
    (0..height)
        .map(|y| (0..width).map(|x| buf[(x, y)].symbol()).collect::<String>())
        .collect()
}

#[test]
fn model_suggestions_replace_box_above_input_in_preview_and_focused_modes() {
    for preview in [true, false] {
        let mut state = model_picker_state();
        state.inline_interactive_state.as_mut().unwrap().preview = preview;
        assert_eq!(crate::tui::ui::inline_ui::inline_ui_height(&state), 0);
        let lines = render_model_suggestions(&state, 120, 20, 12);
        let text = lines.join("\n");
        assert!(text.contains("GPT-5.4"), "{text}");
        assert!(text.contains("openai"), "{text}");
        assert!(text.contains("current"), "{text}");
        assert!(text.contains("Ctrl+N favorite"), "{text}");
        assert!(!text.contains('╭') && !text.contains('│'), "{text}");
        assert!(lines[12..].iter().all(|line| line.trim().is_empty()));
    }
}

#[test]
fn model_suggestions_keep_selection_visible_on_short_terminals() {
    let mut state = model_picker_state();
    let picker = state.inline_interactive_state.as_mut().unwrap();
    picker.entries = (0..20)
        .map(|i| {
            let mut entry = model_picker_entry();
            entry.name = format!("custom-model-{i}");
            entry
        })
        .collect();
    picker.filtered = (0..20).collect();
    picker.selected = 17;
    for height in [1, 2, 3, 10] {
        let lines = render_model_suggestions(&state, 120, 20, height);
        let text = lines.join("\n");
        assert!(text.contains("▸ custom-model-17"), "{text}");
        assert!(
            lines[height as usize..]
                .iter()
                .all(|line| line.trim().is_empty())
        );
    }
    assert!(
        render_model_suggestions(&state, 120, 20, 0)
            .iter()
            .all(|line| line.trim().is_empty())
    );
}

#[test]
fn model_suggestions_show_empty_filter_and_route_notices() {
    let mut state = model_picker_state();
    let route = &mut state.inline_interactive_state.as_mut().unwrap().entries[0].options[0];
    route.available = false;
    route.detail = "Login required".to_string();
    let text = render_model_suggestions(&state, 100, 20, 12).join("\n");
    assert!(text.contains("unavailable · Login required"), "{text}");
    state
        .inline_interactive_state
        .as_mut()
        .unwrap()
        .filtered
        .clear();
    let text = render_model_suggestions(&state, 100, 20, 12).join("\n");
    assert!(text.contains("No matching models"), "{text}");
}

#[test]
fn non_model_pickers_still_reserve_inline_panel_height() {
    let mut state = model_picker_state();
    state.inline_interactive_state.as_mut().unwrap().kind = crate::tui::PickerKind::Login;
    assert!(crate::tui::ui::inline_ui::inline_ui_height(&state) > 0);
    assert!(
        render_model_suggestions(&state, 100, 20, 12)
            .iter()
            .all(|line| line.trim().is_empty())
    );
}

#[test]
fn model_suggestions_full_frame_keeps_composer_position() {
    let _lock = viewport_snapshot_test_lock();
    let mut state = model_picker_state();
    state.input = "/model ".to_string();
    state.display_messages = vec![DisplayMessage::user("hello".to_string())];
    state.inline_interactive_state.as_mut().unwrap().preview = true;
    let render = |state: &TestState| {
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal
            .draw(|frame| crate::tui::ui::draw(frame, state))
            .unwrap();
        let buf = terminal.backend().buffer();
        (0..40)
            .map(|y| (0..120).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
    };
    let with_picker = render(&state);
    state.inline_interactive_state = None;
    let without_picker = render(&state);
    let input_row = |lines: &[String]| {
        lines
            .iter()
            .position(|line| line.contains("> /model"))
            .unwrap()
    };
    assert_eq!(input_row(&with_picker), input_row(&without_picker));
    let model_row = with_picker
        .iter()
        .position(|line| line.contains("GPT-5.4"))
        .unwrap();
    assert!(
        model_row < input_row(&with_picker),
        "{}",
        with_picker.join("\n")
    );
}

#[test]
fn model_suggestions_show_focused_filter_and_route_focus() {
    let mut state = model_picker_state();
    let picker = state.inline_interactive_state.as_mut().unwrap();
    picker.filter = "gpt".to_string();
    picker.column = 1;
    let lines = crate::tui::ui::inline_interactive_ui::model_suggestion_lines(picker, 10);
    let text = lines
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("Search: gpt"), "{text}");
    assert!(text.contains("↑↓ route"), "{text}");
    assert!(
        lines
            .iter()
            .flat_map(|line| &line.spans)
            .any(|span| span.content.contains("openai")
                && span.style.add_modifier.contains(Modifier::UNDERLINED))
    );
}

#[test]
fn model_browser_chrome_fits_narrow_unicode_terminals() {
    for width in [1, 2, 8, 24, 48, 120] {
        for height in [1, 2, 3, 4, 12] {
            let mut state = model_picker_state();
            let picker = state.inline_interactive_state.as_mut().unwrap();
            picker.filter = "@超長供應商 模型模型模型".repeat(10);
            let lines = render_model_suggestions(&state, width, 20, height);
            assert!(
                lines[height as usize..]
                    .iter()
                    .all(|line| line.trim().is_empty())
            );
            let raw = crate::tui::ui::inline_interactive_ui::model_suggestion_lines(
                state.inline_interactive_state.as_ref().unwrap(),
                height as usize,
            );
            for (index, line) in raw.iter().enumerate() {
                if line.width() > width as usize {
                    assert!(
                        lines[height as usize - raw.len() + index]
                            .trim_end()
                            .ends_with('…'),
                        "{width}/{height}: {lines:?}"
                    );
                }
            }
        }
    }
    let mut state = model_picker_state();
    state.inline_interactive_state.as_mut().unwrap().filter = "@openai future".into();
    let text = render_model_suggestions(&state, 120, 20, 12).join("\n");
    assert!(text.contains("Search: future▏"), "{text}");
    assert!(text.contains("Provider: openai"), "{text}");
    assert!(text.contains("Ctrl+P provider"), "{text}");
}

#[test]
fn model_effort_step_keeps_selected_level_visible_in_short_terminals() {
    let mut state = model_picker_state();
    let mut parent = state.inline_interactive_state.take().unwrap();
    parent.entries[0].name = "未來模型".repeat(20);
    let model = parent.entries[0].clone();
    let mut picker = parent.clone();
    picker.entries = ["low", "med", "high", "xhigh", "max"]
        .iter()
        .map(|name| {
            let mut entry = model.clone();
            entry.name = (*name).into();
            entry.options.clear();
            entry.effort = Some((*name).into());
            entry
        })
        .collect();
    picker.filtered = (0..5).collect();
    picker.selected = 4;
    picker.effort_step = Some(Box::new(crate::tui::ModelEffortStep {
        model,
        parent,
        save_default: false,
    }));
    state.inline_interactive_state = Some(picker);
    for height in [1, 2, 3, 4, 12] {
        for width in [1, 2, 8, 24, 80] {
            let lines = render_model_suggestions(&state, width, 20, height);
            assert!(
                lines[height as usize..]
                    .iter()
                    .all(|line| line.trim().is_empty())
            );
            if width >= 8 {
                assert!(
                    lines.join("\n").contains("▸ max"),
                    "{width}/{height}: {lines:?}"
                );
            }
        }
    }
}
