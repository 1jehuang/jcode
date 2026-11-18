use super::*;

#[test]
fn self_owned_todos_have_amber_pips_and_you_badges() {
    let local = assigned(todo("local", "Local task", "in_progress", Some(1)), "@self");
    let unknown = todo("unknown", "Unknown task", "in_progress", Some(1));
    let worker = assigned(todo("worker", "Worker task", "in_progress", Some(1)), "fox");
    let amber = Some(rgb(255, 200, 100));
    assert_eq!(pip_span(&local).style.fg, amber);
    assert_eq!(pip_span(&unknown).style.fg, Some(asap_color()));
    assert_eq!(pip_span(&worker).style.fg, Some(asap_color()));
    assert_eq!(
        lane_item_statuses(&[&local], &[]),
        [("self_in_progress", false)]
    );
    let (chip, _) = lane_agent_chip(&[&local, &local], &[]);
    assert_eq!(Line::from(chip.clone()).to_string(), "you");
    assert_eq!(chip[0].style.fg, amber);
    assert!(agent_roster_spans(&[&local], &[]).is_empty());

    for width in 0..100 {
        let mut lines = Vec::new();
        push_stage_lines(
            &mut lines,
            &[&local, &unknown, &worker],
            &[],
            &[],
            "",
            width,
        );
        assert!(
            lines.iter().all(|line| line.width() <= width),
            "width {width}: {lines:?}"
        );
    }
    let mut lines = Vec::new();
    push_stage_lines(&mut lines, &[&local], &[], &[], "", 100);
    assert!(lines.iter().any(|line| line.to_string().contains("you")));

    let mut flat = local.clone();
    flat.stage = None;
    flat.group = None;
    let mut overview = Vec::new();
    push_stage_pips(&mut overview, &[&flat], 1);
    assert_eq!(overview[0].style.fg, amber);
    let line = render_todo_item_line(&flat, &[], "", 100);
    assert!(line.to_string().contains("you"));
    assert!(
        line.spans
            .iter()
            .any(|span| span.content.contains('●') && span.style.fg == amber)
    );
    for width in 0..100 {
        assert!(render_todo_item_line(&flat, &[], "", width).width() <= width);
    }
}
