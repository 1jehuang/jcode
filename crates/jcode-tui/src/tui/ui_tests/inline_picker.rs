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
        .position(|line| line.contains("Ctrl+F favorite"))
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
