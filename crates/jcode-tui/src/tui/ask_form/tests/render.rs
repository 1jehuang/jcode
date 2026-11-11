//! Rendering tests for the `ask_user` form.

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::Rect;
use serde_json::json;

use super::*;
use crate::tui::ask_form::render as form_render;

fn long_header_form() -> Form {
    let schema = json!({
        "_meta": {"jcode.dev/askUser": {"version": 1}},
        "type": "object",
        "properties": {
            "q1": {
                "type": "string",
                "title": "選択してください",
                "description": "Which option would you like to choose today?",
                "_meta": {"jcode.dev/askUser": {
                    "header": "選択肢 選択肢 選択肢 選択肢 選択肢 選択肢 選択肢 選択肢"
                }},
                "oneOf": [
                    {"const": "a", "title": "Alpha option with a long label"},
                    {"const": "b", "title": "Beta"}
                ]
            },
            "q2": {
                "type": "string",
                "title": "Emoji",
                "description": "Emoji header question.",
                "_meta": {"jcode.dev/askUser": {"header": "🚀🚀🚀🚀🚀🚀"}},
                "oneOf": [{"const": "x", "title": "Xray"}]
            }
        }
    });
    Form::new(&super::form("e1", SESSION, schema), 1).expect("form")
}

fn long_body_schema() -> serde_json::Value {
    let long_question = (0..80)
        .map(|index| format!("distinct body line {index}"))
        .collect::<Vec<_>>()
        .join(" ");
    let long_description = "A long description that must remain reachable. ".repeat(40);
    json!({
        "_meta": {"jcode.dev/askUser": {"version": 1}},
        "type": "object",
        "properties": {
            "q1": {
                "type": "string",
                "title": "Long",
                "description": long_question,
                "_meta": {"jcode.dev/askUser": {"header": "Long"}},
                "oneOf": [
                    {"const": "a", "title": "Alpha", "description": long_description},
                    {"const": "b", "title": "Beta"}
                ]
            }
        }
    })
}

fn preview_heavy_schema(lines: usize) -> serde_json::Value {
    let preview = (0..lines)
        .map(|index| format!("preview line {index}"))
        .collect::<Vec<_>>()
        .join("\n");
    json!({
        "_meta": {"jcode.dev/askUser": {"version": 1}},
        "type": "object",
        "properties": {
            "q1": {
                "type": "string",
                "title": "Preview",
                "description": "Choose",
                "_meta": {"jcode.dev/askUser": {"header": "Preview"}},
                "oneOf": [
                    {"const": "a", "title": "Alpha", "_meta": {"preview": preview}},
                    {"const": "b", "title": "Beta"}
                ]
            }
        }
    })
}

fn joined(line: &ratatui::text::Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

#[test]
fn chrome_rows_never_exceed_width_with_cjk_and_emoji_headers() {
    let form = long_header_form();
    for width in [40usize, 80, 120] {
        let tab = form_render::tab_line(&form, width);
        assert!(
            line_width(&tab) <= width,
            "tab width {} > {width}: {}",
            line_width(&tab),
            joined(&tab)
        );
        let status = form_render::status_line(&form, width);
        assert!(
            line_width(&status) <= width,
            "status width {} > {width}: {}",
            line_width(&status),
            joined(&status)
        );
    }
    // The narrow tab row truncates with an ellipsis measured in display cells.
    let narrow = form_render::tab_line(&form, 40);
    assert!(joined(&narrow).contains('…'));
    assert!(line_width(&narrow) <= 40);
}

#[test]
fn render_at_80x24_40x15_and_80x10_is_bottom_anchored_and_height_capped() {
    reset();
    let context = install_default();
    for (width, height) in [(80u16, 24u16), (40, 15), (80, 10), (80, 11), (80, 12)] {
        let buffer = render_buffer(&context, width, height);
        let expected_height = if height < 12 { height } else { height * 3 / 5 };
        let top = height - expected_height;
        let first = first_nonblank_row(&buffer).expect("panel content");
        assert!(
            first >= top,
            "{width}x{height}: content began at {first}, panel top {top}"
        );
        assert!(
            !row_text(&buffer, height - 1).is_empty(),
            "{width}x{height}: status row should not be blank"
        );
        active(|form| {
            assert_eq!(form.last_body_height as u16, expected_height - 2);
        });
    }
}

#[test]
fn page_keys_scroll_the_whole_body_and_keep_content_reachable() {
    reset();
    let context = install(SESSION, super::form("e1", SESSION, long_body_schema()));
    let before = render_buffer(&context, 80, 24);
    let (lines, body_height) = active(|form| (form.last_body_lines, form.last_body_height));
    assert!(lines > body_height, "long body should overflow: {lines}");

    let top_row_before = row_text(&before, (24 - form_render::panel_height(24)) + 1);
    assert_eq!(key(&context, KeyCode::PageDown), KeyOutcome::Consumed);
    active(|form| assert!(form.body_scroll > 0));
    let after = render_buffer(&context, 80, 24);
    let top_row_after = row_text(&after, (24 - form_render::panel_height(24)) + 1);
    assert_ne!(top_row_before, top_row_after);

    // PageUp returns toward the top.
    key(&context, KeyCode::PageUp);
    active(|form| assert_eq!(form.body_scroll, 0));
}

#[test]
fn long_question_description_and_preview_stay_reachable() {
    reset();
    let context = install(SESSION, super::form("e1", SESSION, long_body_schema()));
    // Render once to establish geometry, then page to the end.
    render_buffer(&context, 40, 15);
    for _ in 0..200 {
        key(&context, KeyCode::PageDown);
    }
    let (scroll, lines, body_height) = active(|form| {
        (
            form.body_scroll,
            form.last_body_lines,
            form.last_body_height,
        )
    });
    assert_eq!(scroll, lines.saturating_sub(body_height));
    let buffer = render_buffer(&context, 40, 15);
    assert!(first_nonblank_row(&buffer).is_some());
}

#[test]
fn preview_is_beside_the_list_at_100_columns_and_toggles_below() {
    reset();
    let context = install_default();

    let wide = render_buffer(&context, 120, 24);
    assert!(
        (0..24).any(|row| row_text(&wide, row).contains("Preview")),
        "wide layout should render a preview pane"
    );

    let narrow = render_buffer(&context, 80, 24);
    assert!(
        !(0..24).any(|row| row_text(&narrow, row).contains("Preview")),
        "narrow layout starts on the options viewport"
    );

    assert_eq!(key(&context, KeyCode::Char('p')), KeyOutcome::Consumed);
    active(|form| assert!(form.preview_focus));
    let toggled = render_buffer(&context, 80, 24);
    assert!(
        (0..24).any(|row| row_text(&toggled, row).contains("Preview")),
        "p should show the preview viewport"
    );
    let status = row_text(&toggled, 23);
    assert!(status.contains("PREVIEW"), "status: {status}");
}

#[test]
fn clipped_preview_shows_a_remaining_line_count() {
    reset();
    let context = install(
        SESSION,
        super::form("e1", SESSION, preview_heavy_schema(60)),
    );
    let buffer = render_buffer(&context, 120, 24);
    assert!(
        (0..24).any(|row| row_text(&buffer, row).contains("+")),
        "clipped preview should show +N lines"
    );
}

#[test]
fn resize_while_editing_preserves_selection_text_and_cursor() {
    reset();
    let context = install_default();
    key(&context, KeyCode::Char('3'));
    assert!(paste_text(&context, "hello"));
    render_buffer(&context, 120, 24);
    active(|form| {
        assert_eq!(form.questions[0].other_text, "hello");
        assert_eq!(form.questions[0].other_cursor, 5);
    });

    render_buffer(&context, 40, 15);
    active(|form| {
        assert_eq!(form.questions[0].confirmed.as_deref(), Some("__other__"));
        assert_eq!(form.questions[0].other_text, "hello");
        assert_eq!(form.questions[0].other_cursor, 5);
        assert!(form.body_scroll <= form.last_body_lines.saturating_sub(form.last_body_height));
    });
}

#[test]
fn tiny_viewports_degrade_without_underflow() {
    reset();
    let context = install_default();

    let one = render_buffer(&context, 80, 1);
    assert!(!row_text(&one, 0).is_empty());

    let two = render_buffer(&context, 80, 2);
    assert!(!row_text(&two, 0).is_empty());
    assert!(!row_text(&two, 1).is_empty());

    // Zero-sized area draws nothing.
    let backend = TestBackend::new(1, 1);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| {
            state::with(|state| {
                form_render::render_view(frame, state, &context, Rect::new(0, 0, 0, 0));
            });
        })
        .expect("zero render");
    let buffer = terminal.backend().buffer().clone();
    assert!(first_nonblank_row(&buffer).is_none());
}
