use super::*;

#[test]
fn pinned_todo_band_stays_left_anchored_with_centered_code_blocks() {
    struct RestoreCentering(bool);
    impl Drop for RestoreCentering {
        fn drop(&mut self) {
            markdown::set_center_code_blocks(self.0);
        }
    }
    let _restore = RestoreCentering(markdown::center_code_blocks());
    let payload = serde_json::json!({
        "todos": [todo("alignment", "Fix pinned alignment", "in_progress", Some(1))],
        "plan": {"user_intention": "Anchor the pinned todo band", "understands_user_intent": "clear"},
        "goals": []
    });
    let msg = DisplayMessage::todos(payload.to_string());
    for centered in [false, true] {
        markdown::set_center_code_blocks(centered);
        for width in [16, 40, 100, 200] {
            let lines =
                render_pinned_todos_message(&msg, width, crate::config::DiffDisplayMode::Off);
            assert!(!lines.is_empty());
            assert!(lines.iter().all(|line| line.width() <= usize::from(width)));
            let first = lines[0].to_string();
            assert_eq!(
                first.len() - first.trim_start().len(),
                2,
                "pinned todo header must stay at the left gutter: centered={centered}, width={width}, {first:?}"
            );
        }
    }
}

#[test]
fn explicit_lane_prerequisites_replace_phase_dimming_and_fit_cells() {
    let mut dependency = todo("dependency", "Dependency", "completed", Some(1));
    let unrelated = todo("other", "Unrelated work", "in_progress", Some(1));
    let mut lane = todo("consumer", "界面 tests", "pending", Some(2));
    lane.group = Some("验证".into());
    lane.lane_after = Some(vec!["dependency".into()]);
    for status in ["completed", "cancelled", "pending"] {
        dependency.status = status.into();
        let refs = [&dependency, &unrelated, &lane];
        let ready = status == "completed";
        assert_eq!(
            lane_prerequisite_progress(&[&lane], &refs),
            Some((usize::from(ready), 1))
        );
        let mut lines = Vec::new();
        push_stage_lines(&mut lines, &refs, &[], &[], "", 100);
        let text = lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            text.contains(if ready {
                "验证: after 1/1 · met"
            } else {
                "after 0/1 · waiting"
            }),
            "{text}"
        );
        let name = lines
            .iter()
            .flat_map(|line| &line.spans)
            .find(|span| span.content.contains("验证") && !span.content.contains("after"))
            .unwrap();
        assert_eq!(name.style.fg == Some(later_phase_color()), !ready);
        for width in 0..80 {
            let mut lines = Vec::new();
            push_stage_lines(&mut lines, &refs, &[], &[], "", width);
            assert!(
                lines.iter().all(|line| line.width() <= width),
                "width {width}: {lines:?}"
            );
        }
    }
    lane.lane_after = Some(vec![]);
    assert_eq!(lane_prerequisite_progress(&[&lane], &[&lane]), Some((0, 0)));
    lane.lane_after = Some(vec!["missing".into(), "missing".into()]);
    assert_eq!(lane_prerequisite_progress(&[&lane], &[&lane]), Some((0, 1)));
    lane.lane_after = None;
    assert_eq!(lane_prerequisite_progress(&[&lane], &[&lane]), None);
}

#[test]
fn narrow_rows_fold_finished_titles_and_never_wrap() {
    for width in [60u16, 40, 24, 14] {
        let rows = render(&grid_todos(), &[], width);
        eprintln!("--- {width}\n{}", rows.join("\n"));
        assert_fits(&rows, width);
        assert_eq!(rows.len(), 6, "one row per lane at {width}");
    }
    let rows = render(&grid_todos(), &[], 50);
    let parser = rows.iter().find(|r| r.contains("parser")).unwrap();
    // Long rows cap each title rather than hiding later ones.
    assert!(
        parser.contains("parser       Extract…, Update…, Add lex…")
            || parser.matches(", ").count() == 2,
        "{rows:?}"
    );
    assert!(render(&grid_todos(), &[], 30)[0].contains("1 ●●○●●●  2 ○○○"));
    let narrow_header = &render(&grid_todos(), &[], 18)[0];
    assert!(
        narrow_header.contains("1 ") && narrow_header.contains("2 "),
        "{narrow_header}"
    );
    assert!(
        !narrow_header.contains('/'),
        "should compress dots instead of showing counts: {narrow_header}"
    );
    assert!(
        narrow_header.contains('●') && narrow_header.contains('○'),
        "{narrow_header}"
    );
}

#[test]
fn stage_pips_compress_proportionally_for_large_stages() {
    let mut todos = Vec::new();
    for i in 0..102 {
        let status = if i < 52 {
            "completed"
        } else if i < 57 {
            "in_progress"
        } else {
            "pending"
        };
        todos.push(todo(&format!("s1-{i}"), "work", status, Some(1)));
    }
    for i in 0..10 {
        let status = if i < 2 { "completed" } else { "pending" };
        todos.push(todo(&format!("s2-{i}"), "next", status, Some(2)));
    }
    todos.push(todo("s3-0", "final", "pending", Some(3)));

    let rows = render(&todos, &[], 80);
    assert_fits(&rows, 80);
    let header = &rows[0];
    assert!(
        !header.contains('/'),
        "large stages should compress dots instead of showing counts: {header}"
    );
    assert!(
        header.contains("1 ") && header.contains("2 ") && header.contains("3 "),
        "{header}"
    );
    let total_dots = header.chars().filter(|&c| c == '●' || c == '○').count();
    assert!(
        total_dots >= 50,
        "should fill header with compressed dots, got {total_dots}: {header}"
    );
    assert!(header.contains('●') && header.contains('○'), "{header}");
}
