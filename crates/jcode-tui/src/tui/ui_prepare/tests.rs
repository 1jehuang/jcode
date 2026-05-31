use super::*;

#[test]
fn centered_mode_centers_unstructured_messages_and_preserves_structured_left_blocks() {
    for role in ["user", "assistant", "meta", "usage", "error", "memory"] {
        assert_eq!(
            default_message_alignment(role, true),
            ratatui::layout::Alignment::Center,
            "role {role} should default to centered alignment"
        );
    }
    for role in ["tool", "system", "swarm", "background_task"] {
        assert_eq!(
            default_message_alignment(role, true),
            ratatui::layout::Alignment::Left,
            "role {role} should keep left/default alignment"
        );
    }
}

#[test]
fn prepare_body_preserves_multiline_user_prompt_lines() {
    let mut lines = Vec::new();
    let mut raw_plain_lines = Vec::new();
    let mut line_raw_overrides = Vec::new();
    let mut line_copy_offsets = Vec::new();
    let mut user_line_indices = Vec::new();

    push_user_prompt_lines(
        &mut lines,
        &mut raw_plain_lines,
        &mut line_raw_overrides,
        &mut line_copy_offsets,
        &mut user_line_indices,
        1,
        user_color(),
        "first line\nsecond line\n\nthird line",
        ratatui::layout::Alignment::Left,
    );

    let plain: Vec<String> = lines.iter().map(ui::line_plain_text).collect();

    assert_eq!(plain.len(), 4);
    assert_eq!(plain[0], "1› first line");
    assert_eq!(plain[1], "   second line");
    assert_eq!(plain[2], "   ");
    assert_eq!(plain[3], "   third line");
    assert_eq!(
        raw_plain_lines,
        vec!["first line", "second line", "", "third line"]
    );
    assert_eq!(user_line_indices, vec![0]);
    assert_eq!(line_copy_offsets, vec![3, 3, 3, 3]);
}
