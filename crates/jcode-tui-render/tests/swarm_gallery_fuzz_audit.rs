//! Audit sweep: panic-safety and width-bound checks for the swarm gallery,
//! panel, and strip renderers across degenerate inputs (empty members, huge
//! member counts, tiny widths/heights, wide glyphs).

use jcode_tui_render::swarm_gallery::{
    GalleryMember, SwarmStripHint, render_gallery, render_swarm_dock, render_swarm_panel,
    render_swarm_strip, summary_line,
};
use ratatui::prelude::Line;
use unicode_width::UnicodeWidthStr;

fn member(id: &str, status: &str, role: Option<&str>, body: &[&str]) -> GalleryMember {
    GalleryMember {
        label: id.to_string(),
        icon: None,
        status: status.to_string(),
        task: None,
        role: role.map(str::to_string),
        body: body.iter().map(|s| s.to_string()).collect(),
        sort_key: id.to_string(),
        todo: None,
        todo_items: Vec::new(),
        model: None,
        provider: None,
        auth_method: None,
        effort: None,
        elapsed_secs: None,
        input_tokens: None,
        output_tokens: None,
        queue_depth: None,
        cost_cents: None,
    }
}

fn plain(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

/// Helper: a small fixed set of members for targeted selected-index tests.
fn selected_members() -> Vec<GalleryMember> {
    (0..10)
        .map(|i| {
            member(
                &format!("agent-{i}"),
                match i % 3 {
                    0 => "running",
                    1 => "done",
                    _ => "thinking",
                },
                if i == 0 { Some("coordinator") } else { None },
                &["output", "· 1s ago"],
            )
        })
        .collect()
}

fn hints() -> Vec<SwarmStripHint> {
    vec![
        SwarmStripHint {
            key: "alt+w".into(),
            label: "focus".into(),
        },
        SwarmStripHint {
            key: "esc".into(),
            label: "back".into(),
        },
    ]
}

fn member_sets() -> Vec<Vec<GalleryMember>> {
    let wide = {
        let mut m = member(
            "🐝🦀日本語エージェント",
            "running",
            Some("coordinator"),
            &["全角テキスト🐝🐝🐝 very wide glyph body line", "· 5s ago"],
        );
        m.todo = Some((3, 8));
        m
    };
    let huge: Vec<GalleryMember> = (0..1500)
        .map(|i| {
            let mut m = member(
                &format!("agent-{i:04}"),
                match i % 6 {
                    0 => "running",
                    1 => "thinking",
                    2 => "done",
                    3 => "failed",
                    4 => "blocked",
                    _ => "spawned",
                },
                if i == 0 { Some("coordinator") } else { None },
                &["some output", "· 2m ago"],
            );
            m.todo = Some((i as u32 % 10, 10));
            m
        })
        .collect();
    vec![
        vec![],
        vec![member("a", "running", None, &[])],
        vec![wide.clone()],
        vec![
            wide,
            member("b", "done", Some("mystery_role_2"), &["ok", "· 1h ago"]),
            member(
                "",
                "weird-status-xyz",
                Some("unknown-role"),
                &["", "  ", "·"],
            ),
        ],
        huge,
    ]
}

#[test]
fn gallery_never_panics_and_stays_width_bounded() {
    for members in member_sets() {
        for width in 0..=60 {
            for max_height in 0..=20 {
                let lines = render_gallery(&members, width, max_height, None);
                for line in &lines {
                    let w = plain(line).as_str().width();
                    assert!(
                        w <= width,
                        "gallery width={width} h={max_height} n={} overflow {w}: {:?}",
                        members.len(),
                        plain(line)
                    );
                }
            }
        }
        // A couple of large widths too.
        for width in [80usize, 200, 500] {
            let _ = render_gallery(&members, width, 16, None);
        }
    }
}

#[test]
fn panel_never_panics_across_degenerate_inputs() {
    for members in member_sets() {
        for width in 0..=40 {
            for max_height in 0..=16 {
                for selected in [0usize, 1, 5, usize::MAX] {
                    for focused in [false, true] {
                        let _ = render_swarm_panel(
                            &members, selected, focused, width, max_height, None,
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn panel_lines_respect_width_bound() {
    let mut violations: Vec<String> = Vec::new();
    for members in member_sets() {
        if members.is_empty() {
            continue;
        }
        for width in 8..=60 {
            for max_height in 3..=16 {
                let lines = render_swarm_panel(&members, 0, true, width, max_height, None);
                for line in &lines {
                    let text = plain(line);
                    let w = text.as_str().width();
                    if w > width {
                        violations.push(format!(
                            "n={} width={width} h={max_height}: {w} cols: {text:?}",
                            members.len()
                        ));
                    }
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "panel emitted over-wide lines ({}):\n{}",
        violations.len(),
        violations[..violations.len().min(10)].join("\n")
    );
}

#[test]
fn strip_never_panics_and_stays_width_bounded() {
    for members in member_sets() {
        for width in 0..=60 {
            for selected in [0usize, 3, usize::MAX] {
                for focused in [false, true] {
                    for spinner in [0usize, 7, usize::MAX] {
                        let lines = render_swarm_strip(
                            &members,
                            selected,
                            focused,
                            &hints(),
                            Some("ctrl+t controls"),
                            spinner,
                            width,
                            12,
                            None,
                        );
                        for line in &lines {
                            let text = plain(line);
                            let w = text.as_str().width();
                            assert!(
                                w <= width,
                                "strip width={width} n={} overflow {w}: {text:?}",
                                members.len()
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn dock_never_panics_and_respects_width_and_height_bounds() {
    for members in member_sets() {
        for width in 0..=48 {
            for max_height in 0..=16 {
                for selected in [0usize, 3, usize::MAX] {
                    for focused in [false, true] {
                        for plan in [None, Some((3u32, 7u32))] {
                            let lines = render_swarm_dock(
                                &members,
                                selected,
                                focused,
                                plan,
                                usize::MAX,
                                width,
                                max_height,
                            );
                            assert!(
                                lines.len() <= max_height.max(1),
                                "dock width={width} h={max_height} n={}: {} lines",
                                members.len(),
                                lines.len()
                            );
                            for line in &lines {
                                let text = plain(line);
                                let w = text.as_str().width();
                                assert!(
                                    w <= width,
                                    "dock width={width} n={} overflow {w}: {text:?}",
                                    members.len()
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Targeted integration tests for recently added/modified rendering paths.
// ---------------------------------------------------------------------------

#[test]
fn gallery_selected_first_and_beyond() {
    let members = selected_members();
    let width = 80;
    let max_height = 16;

    // selected=Some(0) -- first member highlighted
    let lines = render_gallery(&members, width, max_height, Some(0));
    assert!(
        !lines.is_empty(),
        "gallery with selected=0 must produce lines"
    );
    for line in &lines {
        assert!(
            plain(line).as_str().width() <= width,
            "gallery selected=0 overflow: {:?}",
            plain(line)
        );
    }

    // selected=Some(5) -- mid-range
    let lines = render_gallery(&members, width, max_height, Some(5));
    assert!(
        !lines.is_empty(),
        "gallery with selected=5 must produce lines"
    );
    for line in &lines {
        assert!(
            plain(line).as_str().width() <= width,
            "gallery selected=5 overflow: {:?}",
            plain(line)
        );
    }

    // selected=None -- no highlight
    let lines_no = render_gallery(&members, width, max_height, None);
    assert!(
        !lines_no.is_empty(),
        "gallery with selected=None must produce lines"
    );
    for line in &lines_no {
        assert!(
            plain(line).as_str().width() <= width,
            "gallery selected=None overflow: {:?}",
            plain(line)
        );
    }

    // selected far beyond member count -- must not panic
    let lines = render_gallery(&members, width, max_height, Some(999));
    assert!(
        !lines.is_empty(),
        "gallery with out-of-range selected must produce lines"
    );

    // Empty members -- must return empty
    assert!(render_gallery(&[], 80, 12, Some(0)).is_empty());
    // width=0 -- must not panic
    let _ = render_gallery(&members, 0, 12, Some(0));
}

#[test]
fn panel_focused_vs_unfocused() {
    let members = selected_members();
    let width = 60;
    let max_height = 14;

    let focused = render_swarm_panel(&members, 0, true, width, max_height, None);
    let unfocused = render_swarm_panel(&members, 0, false, width, max_height, None);

    // Both should produce lines
    assert!(!focused.is_empty(), "focused panel must produce lines");
    assert!(!unfocused.is_empty(), "unfocused panel must produce lines");

    // Width bounds
    for line in focused.iter().chain(unfocused.iter()) {
        assert!(
            plain(line).as_str().width() <= width,
            "panel overflow: {:?}",
            plain(line)
        );
    }

    // Focused panel should include an interaction hint ("alt+w" or similar)
    let focused_text: String = focused
        .iter()
        .map(|l| plain(l))
        .collect::<Vec<_>>()
        .join("\n");
    let unfocused_text: String = unfocused
        .iter()
        .map(|l| plain(l))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        focused_text.len() >= unfocused_text.len(),
        "focused panel should have at least as much content as unfocused"
    );

    // Panel with no members returns empty
    assert!(render_swarm_panel(&[], 0, true, 60, 14, None).is_empty());
    // Panel with width < 8 returns empty
    assert!(render_swarm_panel(&members, 0, true, 4, 14, None).is_empty());
    // Panel with max_height < 3 returns empty
    assert!(render_swarm_panel(&members, 0, true, 60, 2, None).is_empty());
}

#[test]
fn strip_selected_indices_no_panic() {
    let members = selected_members();
    let width = 80;
    let max_height = 4;

    // selected=0, 3, 9 (last), usize::MAX (beyond range), each with focused/unfocused
    for selected in [0usize, 3, 9, usize::MAX] {
        for focused in [false, true] {
            let lines = render_swarm_strip(
                &members,
                selected,
                focused,
                &hints(),
                Some("ctrl+t controls"),
                5, // spinner_frame
                width,
                max_height,
                None,
            );
            for line in &lines {
                assert!(
                    plain(line).as_str().width() <= width,
                    "strip selected={selected} focused={focused} overflow: {:?}",
                    plain(line)
                );
            }
        }
    }

    // Empty members returns empty
    assert!(render_swarm_strip(&[], 0, true, &hints(), None, 0, 80, 4, None).is_empty());
}

#[test]
fn dock_focused_true_and_false() {
    let members = selected_members();
    let width = 60;
    let max_height = 14;

    for focused in [false, true] {
        for selected in [0usize, 5, usize::MAX] {
            let lines = render_swarm_dock(
                &members,
                selected,
                focused,
                Some((3, 7)),
                0,
                width,
                max_height,
            );
            assert!(
                lines.len() <= max_height,
                "dock focused={focused} selected={selected} exceeded height: {}",
                lines.len()
            );
            for line in &lines {
                assert!(
                    plain(line).as_str().width() <= width,
                    "dock focused={focused} selected={selected} overflow: {:?}",
                    plain(line)
                );
            }
        }
    }

    // Empty members returns empty
    assert!(render_swarm_dock(&[], 0, true, None, 0, 60, 14).is_empty());
    // width < 12 returns empty
    assert!(render_swarm_dock(&members, 0, true, None, 0, 10, 14).is_empty());
    // max_height < 2 returns empty
    assert!(render_swarm_dock(&members, 0, true, None, 0, 60, 1).is_empty());
}

#[test]
fn summary_line_is_width_bounded_and_format_correct() {
    let line = summary_line(4, 2, 12, 20, 185, 80);
    let text = plain(&line);
    assert!(
        text.as_str().width() <= 80,
        "summary_line overflow: {:?}",
        text
    );
    assert!(text.contains("4 agents"));
    assert!(text.contains("2 active"));
    assert!(text.contains("12/20 tasks"));
    assert!(text.contains("3m 5s elapsed"));

    // Single agent
    let line = summary_line(1, 1, 0, 3, 45, 60);
    let text = plain(&line);
    assert!(text.contains("1 agent"));
    assert!(text.contains("0/3 tasks"));
    assert!(text.contains("45s elapsed"));

    // Narrow width: still bounded (clamp_line_to_width trims externally)
    let line = summary_line(2, 0, 5, 5, 3600, 50);
    let text = plain(&line);
    assert!(
        text.as_str().width() <= 50,
        "summary_line narrow overflow: {:?}",
        text
    );
}
