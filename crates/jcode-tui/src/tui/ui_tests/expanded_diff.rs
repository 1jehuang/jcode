use super::*;

#[test]
fn expanded_diff_full_terminal_preserves_indentation_gutters_and_long_source() {
    let _guard = viewport_snapshot_test_lock();
    let long_source = format!("    let value = \"{}\";", "abcdefghij_".repeat(22));
    for centered_mode in [false, true] {
        for width in [40, 80, 120] {
            let state = TestState {
                display_messages: vec![DisplayMessage::tool(
                    format!(
                        "9-     old_value();\n9+ {long_source}\n10+         nested();\n11+     done();"
                    ),
                    crate::message::ToolCall {
                        id: format!("expanded-frame-{width}-{centered_mode}"),
                        name: "edit".into(),
                        input: serde_json::json!({"file_path": "src/example.rs"}),
                        intent: Some("Update nested code".into()),
                        thought_signature: None,
                    },
                )],
                messages_version: 2000 + u64::from(width) + u64::from(centered_mode),
                centered_mode,
                diff_mode_override: Some(crate::config::DiffDisplayMode::FullInline),
                suppress_info_widgets: true,
                ..Default::default()
            };
            clear_test_render_state_for_tests();
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, 80))
                    .expect("terminal");
            terminal.draw(|frame| draw(frame, &state)).expect("draw");
            let buffer = terminal.backend().buffer();
            let rows: Vec<String> = (0..80)
                .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect())
                .collect();
            let diff_rows: Vec<&str> = rows
                .iter()
                .filter_map(|row| row.split_once("│ ").map(|(_, rest)| rest.trim_end()))
                .collect();
            assert!(rows.iter().any(|row| row.contains("╭─ diff")), "{rows:#?}");
            assert!(rows.iter().any(|row| row.contains("╰─")), "{rows:#?}");
            assert!(
                diff_rows.contains(&" 9 -     old_value();"),
                "{diff_rows:#?}"
            );
            assert!(
                diff_rows.contains(&"10 +         nested();"),
                "{diff_rows:#?}"
            );
            assert!(diff_rows.contains(&"11 +     done();"), "{diff_rows:#?}");
            let mut reconstructed = String::new();
            let mut capturing = false;
            let mut continuation_count = 0;
            for row in &diff_rows {
                if let Some(source) = row.strip_prefix(" 9 + ") {
                    capturing = true;
                    reconstructed.push_str(source);
                } else if capturing {
                    if let Some(source) = row.strip_prefix("   ↪ ") {
                        continuation_count += 1;
                        reconstructed.push_str(source);
                    } else {
                        break;
                    }
                }
            }
            assert!(continuation_count > 0, "{diff_rows:#?}");
            assert_eq!(
                reconstructed, long_source,
                "width={width} centered={centered_mode}: {diff_rows:#?}"
            );
            println!(
                "width={width} centered={centered_mode}, continuations={continuation_count}:\n{}",
                diff_rows.join("\n")
            );
        }
    }
}
