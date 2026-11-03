//! Focused dock regression tests.
use super::*;

#[test]
fn dock_lists_agents_with_header_and_selected_tail() {
    let mut m1 = member(
        "researcher",
        "thinking",
        Some("coordinator"),
        &["tracing refresh path", "· 2s ago"],
    );
    m1.todo = Some((2, 5));
    let m2 = member("implementer", "running", None, &["building"]);
    let m3 = member("reviewer", "completed", None, &["LGTM"]);
    let lines = render_swarm_dock(&[m1, m2, m3], 0, false, Some((3, 7)), 0, 34, 12);
    let all: Vec<String> = lines.iter().map(plain_line).collect();
    let joined = all.join("\n");
    assert!(joined.contains("2/3 active"), "got:\n{joined}");
    assert!(joined.contains("plan 3/7"), "got:\n{joined}");
    for name in ["researcher", "implementer", "reviewer"] {
        assert!(joined.contains(name), "missing {name} in:\n{joined}");
    }
    // Selected (coordinator, sorts first) shows its live tail with gutter.
    assert!(joined.contains("│ tracing refresh path"), "got:\n{joined}");
    // Meta age line stays out of the tail.
    assert!(!joined.contains("2s ago"), "got:\n{joined}");
    // Todo counter on the row.
    assert!(joined.contains("2/5"), "got:\n{joined}");
    // Unfocused: no hint line.
    assert!(!joined.contains("j/k"), "got:\n{joined}");
}

#[test]
fn dock_focused_shows_hints_and_attention_count() {
    let members = vec![
        member("a", "running", None, &["working"]),
        member("b", "blocked", None, &["stuck"]),
        member("c", "failed", None, &["boom"]),
    ];
    let lines = render_swarm_dock(&members, 1, true, None, 0, 34, 14);
    let joined: String = lines.iter().map(plain_line).collect::<Vec<_>>().join("\n");
    assert!(joined.contains("⚠2"), "got:\n{joined}");
    assert!(joined.contains("j/k"), "got:\n{joined}");
    // Selected row marked.
    let sel = lines
        .iter()
        .map(plain_line)
        .find(|l| l.contains('▸'))
        .expect("selected row");
    assert!(sel.contains('b'), "got: {sel}");
}

#[test]
fn dock_windows_list_and_reports_overflow() {
    let members: Vec<GalleryMember> = (0..12)
        .map(|i| member(&format!("agent-{i:02}"), "running", None, &["w"]))
        .collect();
    // Budget: header + 5 rows -> 7 hidden.
    let lines = render_swarm_dock(&members, 11, false, None, 0, 30, 7);
    assert!(lines.len() <= 7, "got {} lines", lines.len());
    let joined: String = lines.iter().map(plain_line).collect::<Vec<_>>().join("\n");
    // Selection stays visible even at the end of the list.
    assert!(joined.contains("agent-11"), "got:\n{joined}");
    assert!(joined.contains("+7 more"), "got:\n{joined}");
}

#[test]
fn dock_is_display_width_bounded_at_all_widths() {
    use unicode_width::UnicodeWidthStr;
    let mut members: Vec<GalleryMember> = (0..6)
        .map(|i| {
            member(
                &format!("agent-🐝-{i}"),
                "running",
                None,
                &["line 一二三四五"],
            )
        })
        .collect();
    members[0].role = Some("coordinator".into());
    members[0].todo = Some((3, 9));
    for width in 8..=60 {
        for focused in [false, true] {
            for height in [0usize, 1, 2, 3, 8, 20] {
                let lines = render_swarm_dock(
                    &members,
                    3,
                    focused,
                    Some((2, 9)),
                    usize::MAX / 2,
                    width,
                    height,
                );
                assert!(
                    lines.len() <= height.max(1),
                    "width {width} height {height}: {} lines",
                    lines.len()
                );
                for line in &lines {
                    let text = plain_line(line);
                    let w = text.as_str().width();
                    assert!(
                        w <= width,
                        "width {width} focused {focused}: line overflows ({w}): {text:?}"
                    );
                }
            }
        }
    }
}
