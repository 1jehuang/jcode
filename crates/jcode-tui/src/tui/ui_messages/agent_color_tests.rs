use super::*;

#[test]
fn dm_group_only_total_tokens_use_severity_color() {
    markdown::set_center_code_blocks(false);
    for (each, expected_color) in [
        (1_999usize, rgb(118, 118, 118)),
        (2_000, rgb(214, 184, 92)),
        (5_999, rgb(214, 184, 92)),
        (6_000, rgb(224, 118, 118)),
    ] {
        let messages = [
            DisplayMessage::swarm("DM from fox", "a".repeat(each * 4)),
            DisplayMessage::swarm("DM from sheep", "b".repeat(each * 4)),
        ];
        let row = render_agent_dm_run(&messages, 100);
        let text = row.to_string();
        assert!(text.starts_with("DMs · 🦊 🐑 · "), "{text}");
        assert_eq!(text.matches(" tok").count(), 1, "{text}");
        assert_eq!(text.matches(SWARM_EXPAND_BADGE).count(), 1, "{text}");
        assert!(text.contains(&crate::util::format_approx_token_count(each * 2)));
        for span in &row.spans {
            assert_eq!(
                span.style.fg,
                Some(if span.content.contains("tok") {
                    expected_color
                } else {
                    dim_color()
                }),
                "{span:?}"
            );
        }
    }
}

#[test]
fn dm_total_ignores_collapsible_summary_and_stays_stable_after_toggle() {
    markdown::set_center_code_blocks(false);
    let body = "Message body with private details";
    let encoded =
        jcode_tui_messages::encode_collapsible_swarm_content("Summary adds no real tokens", body);
    assert_eq!(
        swarm_notification_token_count(&encoded),
        crate::util::estimate_tokens(body)
    );
    let toggled = jcode_tui_messages::toggle_collapsible_swarm_content(&encoded).unwrap();
    assert_eq!(
        swarm_notification_token_count(&toggled),
        swarm_notification_token_count(body)
    );
}

#[test]
fn dm_group_reserves_total_and_badge_when_emoji_cluster_is_truncated() {
    markdown::set_center_code_blocks(false);
    let messages = vec![DisplayMessage::swarm("DM from fox", "a".repeat(400)); 20];
    let row = render_agent_dm_run(&messages, 30);
    assert!(row.width() <= 30);
    let text = row.to_string();
    assert!(text.contains("2k tok"), "{text}");
    assert!(text.ends_with(SWARM_EXPAND_BADGE), "{text}");
}

#[test]
fn collapsed_completion_lanes_match_pinned_todo_style() {
    markdown::set_center_code_blocks(false);
    for done in [0, 1] {
        let body = format!(
            "Agent tulip finished their work and is ready for more.\n\nLane plan:\nPhase 1 · restart-acceptance: {done}/1 completed\n\nFinal response:\nResult retained."
        );
        let content = jcode_tui_messages::encode_collapsible_swarm_content(
            &format!("restart-acceptance {done}/1 · Result retained."),
            &body,
        );
        let message = DisplayMessage::swarm("Swarm · tulip", content);
        for width in [1, 4, 16, 40, 100] {
            let rows = render_swarm_message(&message, width, crate::config::DiffDisplayMode::Off);
            assert_eq!(rows.len(), 1);
            assert!(rows[0].width() <= usize::from(width));
            if width == 100 {
                let text = rows[0].to_string();
                assert!(text.contains(&format!("{done}/1")), "{text}");
                assert!(text.contains(if done == 1 { "●" } else { "○" }), "{text}");
                assert!(text.contains("Result retained."), "{text}");
                assert!(
                    rows[0]
                        .spans
                        .iter()
                        .any(|span| span.content.contains("restart-acceptance")
                            && span.style.fg == Some(todo_group_color()))
                );
            }
        }
    }
}

#[test]
fn completion_lanes_use_pinned_todo_palette_and_dots() {
    let body = "Worker summary\nAgent daisy finished their work and is ready for more.\n\nLane plan:\nPhase 2 · 界-release: 1/3 completed\n- [in_progress] Verify\n- [pending] Publish\nPhase 3 · docs: 2/2 completed\n\nFinal response:\n**Report remains accented.**";
    let accent = swarm_agent_color("daisy");
    for width in [1, 4, 12, 80] {
        let rows = render_completion_lane_body(body, width, accent).unwrap();
        let text = rows
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!text.contains("Lane plan:"));
        assert!(!text.contains("Phase 2"));
        assert!(!text.contains("Final response:"));
        assert!(!text.contains("Agent daisy"));
        assert!(!text.contains("Worker summary"));
        for row in &rows {
            if row
                .spans
                .iter()
                .any(|span| span.style.fg == Some(todo_group_color()))
            {
                assert!(row.width() <= width, "{row:?}");
            }
        }
        if width == 80 {
            assert!(text.contains("界-release  ●●○"), "{text}");
            assert!(text.contains("docs  ●●"), "{text}");
            assert!(rows.iter().flat_map(|row| &row.spans).any(|span| {
                span.content.contains("界-release") && span.style.fg == Some(todo_group_color())
            }));
            let report = rows
                .iter()
                .flat_map(|row| &row.spans)
                .find(|span| span.content.contains("Report remains accented"))
                .unwrap();
            assert_eq!(report.style.fg, Some(accent));
            assert!(report.style.add_modifier.contains(Modifier::BOLD));
        }
    }
    assert!(render_completion_lane_body("Agent note without envelope", 80, accent).is_none());
    let missing = body.replace("**Report remains accented.**", "No report captured.");
    let rows = render_completion_lane_body(&missing, 80, accent).unwrap();
    let missing_span = rows
        .iter()
        .flat_map(|row| &row.spans)
        .find(|span| span.content == "No report captured.")
        .unwrap();
    assert_eq!(
        missing_span.style.fg,
        Some(jcode_tui_style::role_color(jcode_tui_style::Role::Dim))
    );
}

#[test]
fn swarm_agent_color_tracks_identity_not_delivery_kind_or_restart() {
    let daisy = swarm_agent_color("daisy");
    assert_ne!(daisy, swarm_agent_color("tulip"));
    assert_eq!(daisy, swarm_agent_color(" DAISY "));
    assert_eq!(daisy, swarm_agent_color("session_daisy_123_abc"));
    assert_eq!(daisy, swarm_agent_color("session_daisy_456_def"));
    for title in [
        "Swarm · daisy",
        "DM from daisy",
        "Task · daisy",
        "#release · daisy",
        "Broadcast · daisy",
        "Shared context · daisy",
        "Swarm · session_daisy_123_abc",
    ] {
        let notification = compact_swarm_notification(title).unwrap();
        assert_eq!(notification.text_color, daisy, "{title}");
        assert_eq!(notification.sender, "daisy");
    }
}

#[test]
fn swarm_agent_collapsed_and_expanded_reports_share_identity_foreground() {
    markdown::set_center_code_blocks(false);
    let report = "Agent daisy finished their work and is ready for more.\n\nLane plan: Phase 2 · release-candidate-validation: 0/1 completed\n\n- [in_progress] release-candidate-validation\n\nFinal response: **Blocked pending source freeze.**\n\n*Coordinator notified with concrete fixes.*";
    let collapsed = jcode_tui_messages::encode_collapsible_swarm_content(
        "Release validation needs a source freeze",
        report,
    );
    let expanded = jcode_tui_messages::toggle_collapsible_swarm_content(&collapsed).unwrap();
    let accent = swarm_agent_color("daisy");
    for content in [collapsed, expanded] {
        let message = DisplayMessage::swarm("Swarm · daisy", content);
        let rows = render_swarm_message(&message, 100, crate::config::DiffDisplayMode::Off);
        let mut prose_spans = 0;
        for row in &rows {
            for span in &row.spans {
                if span.content.contains("Release validation")
                    || span.content.contains("Lane plan")
                    || span.content.contains("Blocked pending")
                    || span.content.contains("Coordinator notified")
                    || span.content.contains("Final response")
                {
                    assert_eq!(span.style.fg, Some(accent), "{span:?}");
                    prose_spans += 1;
                }
            }
        }
        assert!(prose_spans > 0);
        assert_eq!(rows[0].spans[0].style.fg, Some(accent));
    }
}

#[test]
fn swarm_agent_expansion_preserves_markdown_modifiers_and_semantic_code_colors() {
    markdown::set_center_code_blocks(false);
    let body = "**Strong identity prose**\n\n*Italic identity prose*\n\n`inline_code`\n\n```rust\nlet answer = 42;\n```";
    let content = jcode_tui_messages::encode_collapsible_swarm_content("Validation report", body);
    let expanded = jcode_tui_messages::toggle_collapsible_swarm_content(&content).unwrap();
    let message = DisplayMessage::swarm("Swarm · tulip", expanded);
    let rows = render_swarm_message(&message, 100, crate::config::DiffDisplayMode::Off);
    let spans: Vec<_> = rows.iter().flat_map(|row| &row.spans).collect();
    let accent = swarm_agent_color("tulip");
    let strong = spans
        .iter()
        .find(|span| span.content.contains("Strong identity prose"))
        .unwrap();
    assert_eq!(strong.style.fg, Some(accent));
    assert!(strong.style.add_modifier.contains(Modifier::BOLD));
    let italic = spans
        .iter()
        .find(|span| span.content.contains("Italic identity prose"))
        .unwrap();
    assert_eq!(italic.style.fg, Some(accent));
    assert!(italic.style.add_modifier.contains(Modifier::ITALIC));
    let inline_code = spans
        .iter()
        .find(|span| span.content.contains("inline_code"))
        .unwrap();
    assert_ne!(inline_code.style.fg, Some(accent));
    assert!(inline_code.style.bg.is_some());
    assert!(
        spans
            .iter()
            .any(|span| span.content.contains("42") && span.style.fg != Some(accent))
    );
}

#[test]
fn swarm_agent_collapsed_rows_remain_single_line_and_cell_width_safe() {
    markdown::set_center_code_blocks(false);
    let content = jcode_tui_messages::encode_collapsible_swarm_content(
        "界界界 release candidate needs validation",
        "Full report from daisy\n\nMore report text.",
    );
    let message = DisplayMessage::swarm("Swarm · daisy", content);
    for width in [0, 1, 2, 3, 8, 16, 24, 40, 80] {
        let rows = render_swarm_message(&message, width, crate::config::DiffDisplayMode::Off);
        assert_eq!(rows.len(), usize::from(width > 0));
        for row in rows {
            assert!(row.width() <= width as usize, "{width}: {row:?}");
        }
    }
}
