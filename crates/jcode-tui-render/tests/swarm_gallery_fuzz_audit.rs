//! Audit sweep: panic-safety and width-bound checks for the swarm gallery,
//! panel, and strip renderers across degenerate inputs (empty members, huge
//! member counts, tiny widths/heights, wide glyphs).

use jcode_tui_render::swarm_gallery::{
    GalleryMember, SwarmStripHint, render_gallery, render_swarm_dock, render_swarm_panel,
    render_swarm_strip,
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
    }
}

fn plain(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
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
                let lines = render_gallery(&members, width, max_height);
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
            let _ = render_gallery(&members, width, 16);
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
                        let _ = render_swarm_panel(&members, selected, focused, width, max_height);
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
                let lines = render_swarm_panel(&members, 0, true, width, max_height);
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
