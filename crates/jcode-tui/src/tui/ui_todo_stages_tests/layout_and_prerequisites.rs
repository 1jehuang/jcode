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
