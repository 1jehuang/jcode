#[test]
fn test_reasoning_emphasis_does_not_leak_into_following_text() {
    // After the reasoning emphasis closes, normal paragraph text must not be
    // styled as reasoning (dim/italic).
    let md = format!(
        "{}\n{}",
        crate::reasoning_line_markup("it's **thinking** with `code`."),
        "Normal answer text."
    );
    let lines = render_markdown(&md);

    let dim = md_dim_color();
    let answer_line = lines
        .iter()
        .find(|l| line_to_string(l).contains("Normal answer text."))
        .expect("answer line present");
    for span in &answer_line.spans {
        if span.content.trim().is_empty() {
            continue;
        }
        assert_ne!(
            span.style.fg,
            Some(dim),
            "answer span must not be dim: {:?}",
            span.content
        );
        assert!(
            !span.style.add_modifier.contains(Modifier::ITALIC),
            "answer span must not be italic: {:?}",
            span.content
        );
    }
}

#[test]
fn test_reasoning_summary_line_markup_folds_to_single_dim_italic_trace() {
    let sentinel = crate::REASONING_SENTINEL;

    // Pluralized count for multi-line blocks.
    let many = crate::reasoning_summary_line_markup(3);
    assert!(
        many.contains(&format!("*{0}▸ thought (3 lines){0}*", sentinel)),
        "expected pluralized summary markup, got: {many:?}"
    );

    // Single/zero-line blocks omit the count.
    let one = crate::reasoning_summary_line_markup(1);
    assert!(
        one.contains(&format!("*{0}▸ thought{0}*", sentinel)) && !one.contains("lines"),
        "expected bare summary markup, got: {one:?}"
    );
    let none = crate::reasoning_summary_line_markup(0);
    assert!(
        none.contains(&format!("*{0}▸ thought{0}*", sentinel)),
        "{none:?}"
    );

    // The summary line renders dim + italic with no sentinel leaking into text.
    let lines = render_markdown(&many);
    let dim = md_dim_color();
    let mut saw_marker = false;
    for rendered in &lines {
        for span in &rendered.spans {
            assert!(
                !span.content.contains(sentinel),
                "sentinel leaked into visible summary: {:?}",
                span.content
            );
            if span.content.trim().is_empty() {
                continue;
            }
            if span.content.contains('▸') {
                saw_marker = true;
            }
            assert_eq!(
                span.style.fg,
                Some(dim),
                "summary span not dim: {:?}",
                span.content
            );
            assert!(
                span.style.add_modifier.contains(Modifier::ITALIC),
                "summary span not italic: {:?}",
                span.content
            );
        }
    }
    assert!(saw_marker, "summary marker '▸' must be visible: {lines:?}");
}
