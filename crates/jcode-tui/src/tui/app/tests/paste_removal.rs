#[test]
fn paste_removal_review_deleted_payload_cannot_rebind_a_later_paste() {
    let a = "A1\nA2\nA3\nA4\nA5";
    let b = "B1\nB2\nB3\nB4\nB5";
    let c = "C1\nC2\nC3\nC4\nC5";
    for replacement in ["", "replacement"] {
        let mut app = create_test_app();
        super::input::handle_text_paste(&mut app, a.into());
        super::input::insert_input_text(&mut app, " | ");
        let start = app.cursor_pos;
        super::input::handle_text_paste(&mut app, b.into());
        let original = app.input.clone();
        app.remember_input_undo_state();
        app.replace_input_range(start, app.input.len(), replacement);
        super::input::handle_text_paste(&mut app, c.into());
        assert_eq!(app.pasted_contents, vec![a, c]);
        assert_eq!(
            super::input::expand_paste_placeholders(&app, &app.input),
            format!("{a} | {replacement}{c}")
        );
        app.undo_input_change();
        assert_eq!(app.pasted_contents, vec![a]);
        app.undo_input_change();
        assert_eq!(app.input, original);
        assert_eq!(app.pasted_contents, vec![a, b]);
        let prepared = super::input::take_prepared_input(&mut app);
        assert_eq!(prepared.expanded, format!("{a} | {b}"));
    }
}

#[test]
fn paste_removal_review_repeated_paste_expansion_keeps_other_payloads() {
    let a = "A1\nA2\nA3\nA4\nA5";
    let b = "B1\nB2\nB3\nB4\nB5";
    let mut app = create_test_app();
    super::input::handle_text_paste(&mut app, a.into());
    super::input::insert_input_text(&mut app, " | ");
    super::input::handle_text_paste(&mut app, b.into());
    super::input::handle_text_paste(&mut app, b.into());
    assert_eq!(app.pasted_contents, vec![a]);
    let prepared = super::input::take_prepared_input(&mut app);
    assert_eq!(prepared.expanded, format!("{a} | {b}"));
}

#[test]
fn paste_removal_review_literal_markers_and_distinct_sizes_stay_literal() {
    let literal = "[pasted 5 lines] [image 1] | ";
    let a = "A1\nA2\nA3\nA4\nA5";
    let b = "B1\nB2\nB3\nB4\nB5\nB6";
    let c = "C1\nC2\nC3\nC4\nC5\nC6";
    let mut app = create_test_app();
    super::input::insert_input_text(&mut app, literal);
    super::input::handle_text_paste(&mut app, a.into());
    super::input::insert_input_text(&mut app, " | ");
    let start = app.cursor_pos;
    super::input::handle_text_paste(&mut app, b.into());
    app.remember_input_undo_state();
    app.remove_input_range(start, app.input.len());
    super::input::handle_text_paste(&mut app, c.into());
    let prepared = super::input::take_prepared_input(&mut app);
    assert_eq!(prepared.expanded, format!("{literal}{a} | {c}"));
    assert!(prepared.images.is_empty());
    let mut app = create_test_app();
    super::input::insert_input_text(&mut app, literal);
    super::input::handle_text_paste(&mut app, a.into());
    app.remove_input_range(0, "[pasted 5 lines]".len());
    assert_eq!(app.pasted_contents, vec![a]);
    assert!(app.pending_images.is_empty());
    let prepared = super::input::take_prepared_input(&mut app);
    assert_eq!(prepared.expanded, format!(" [image 1] | {a}"));
}

#[test]
fn paste_removal_review_partial_middle_label_cannot_shift_equal_size_payloads() {
    let a = "A1\nA2\nA3\nA4\nA5";
    let b = "B1\nB2\nB3\nB4\nB5";
    let c = "C1\nC2\nC3\nC4\nC5";
    let mut app = create_test_app();
    super::input::handle_text_paste(&mut app, a.into());
    super::input::insert_input_text(&mut app, " | ");
    let middle = app.cursor_pos;
    super::input::handle_text_paste(&mut app, b.into());
    super::input::insert_input_text(&mut app, " | ");
    super::input::handle_text_paste(&mut app, c.into());
    app.remember_input_undo_state();
    app.remove_input_range(middle, middle + 1);
    let prepared = super::input::take_prepared_input(&mut app);
    assert_eq!(prepared.expanded, format!("{a} | pasted 5 lines] | {c}"));
    assert_eq!(prepared.pasted_contents, vec![a, c]);
}
