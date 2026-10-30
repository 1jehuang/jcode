//! Visual preview of the swarm gallery layout against mock streams.
//!
//! Run with: `cargo run --profile selfdev -p jcode-tui-render --example swarm_gallery_preview`

use jcode_tui_render::swarm_gallery::{
    GalleryMember, SwarmStripHint, render_swarm_compact, render_swarm_dock, render_swarm_panel,
    render_swarm_strip, render_swarm_strip_vertical,
};
use jcode_tui_render::swarm_tiles::{SwarmGalleryConfig, SwarmTile, render_swarm_gallery};
use ratatui::prelude::*;

fn accent(status: &str) -> Color {
    match status {
        "running" => Color::Rgb(255, 200, 100),
        "thinking" => Color::Rgb(140, 180, 255),
        "done" => Color::Rgb(100, 200, 100),
        "blocked" => Color::Rgb(255, 170, 80),
        "failed" => Color::Rgb(255, 100, 100),
        _ => Color::Rgb(140, 140, 150),
    }
}

fn mk(name: &str, role: Option<&str>, status: &str, body: &[&str]) -> SwarmTile {
    let mut t = SwarmTile::new(name, status, accent(status))
        .with_body(body.iter().map(|s| s.to_string()).collect());
    if let Some(r) = role {
        t = t.with_role_glyph(r);
    }
    t
}

fn print_lines(label: &str, lines: &[Line<'static>]) {
    println!("\n=== {label} ===");
    for line in lines {
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        println!("{text}");
    }
}

fn main() {
    let header = Line::from(Span::styled(
        "🐝 swarm · 3 agents running",
        Style::default().fg(Color::Rgb(255, 200, 100)),
    ));

    let three = vec![
        mk(
            "researcher",
            Some("★"),
            "thinking",
            &[
                "Searching the codebase for the auth flow...",
                "Found 12 candidate files.",
                "Reading crates/jcode-app-core/src/auth.rs",
                "The OAuth callback is handled in handle_login()",
                "Now cross-referencing the token refresh path.",
            ],
        ),
        mk(
            "implementer",
            None,
            "running",
            &[
                "Editing crates/jcode-base/src/config.rs",
                "Added swarm_spawn_mode = inline",
                "Running cargo check...",
                "warning: unused import `Foo`",
                "Fixing the import.",
            ],
        ),
        mk(
            "reviewer",
            None,
            "done",
            &["Reviewed 4 files.", "No blocking issues found.", "LGTM ✓"],
        ),
    ];

    let cfg = SwarmGalleryConfig::default();
    print_lines(
        "3 agents @ width 100",
        &render_swarm_gallery(&three, 100, &cfg, Some(header.clone())),
    );
    print_lines(
        "3 agents @ width 60",
        &render_swarm_gallery(&three, 60, &cfg, Some(header.clone())),
    );

    let six: Vec<SwarmTile> = (0..6)
        .map(|i| {
            let status = ["running", "thinking", "done", "blocked"][i % 4];
            mk(
                &format!("agent-{i}"),
                None,
                status,
                &[
                    &format!("step {i}.1 doing work"),
                    &format!("step {i}.2 still going"),
                    &format!("step {i}.3 almost there"),
                ],
            )
        })
        .collect();
    print_lines(
        "6 agents @ width 120",
        &render_swarm_gallery(&six, 120, &cfg, Some(header.clone())),
    );

    let many: Vec<SwarmTile> = (0..12)
        .map(|i| {
            mk(
                &format!("worker-{i:02}"),
                None,
                "running",
                &["...", "working"],
            )
        })
        .collect();
    let tight = SwarmGalleryConfig {
        max_height: 12,
        ..Default::default()
    };
    print_lines(
        "12 agents @ width 120, height 12",
        &render_swarm_gallery(&many, 120, &tight, Some(header)),
    );

    // ---- New list+detail panel ----
    let gm = |name: &str, role: Option<&str>, status: &str, body: &[&str]| GalleryMember {
        label: name.to_string(),
        icon: None,
        status: status.to_string(),
        task: None,
        role: role.map(str::to_string),
        body: body.iter().map(|s| s.to_string()).collect(),
        sort_key: name.to_string(),
        todo: None,
        todo_items: Vec::new(),
        model: None,
        provider: None,
        auth_method: None,
        effort: None,
        elapsed_secs: None,
    };
    let panel_members = vec![
        gm(
            "researcher",
            Some("coordinator"),
            "thinking",
            &["Cross-referencing the token refresh path.", "· 2s ago"],
        ),
        gm(
            "implementer",
            None,
            "running",
            &[
                "Running cargo check...",
                "warning: unused import `Foo`",
                "· 5s ago",
            ],
        ),
        gm("reviewer", None, "done", &["LGTM ✓", "· 1m ago"]),
        gm(
            "doc-writer",
            None,
            "blocked",
            &["waiting on reviewer", "· 12s ago"],
        ),
    ];
    print_lines(
        "PANEL: 4 agents, selected #1 (implementer), focused @ width 70 h 14",
        &render_swarm_panel(&panel_members, 1, true, 70, 14),
    );
    print_lines(
        "PANEL: 4 agents, selected #0, unfocused @ width 70 h 14",
        &render_swarm_panel(&panel_members, 0, false, 70, 14),
    );
    print_lines(
        "PANEL: narrow @ width 44 h 12",
        &render_swarm_panel(&panel_members, 2, true, 44, 12),
    );

    // ---- New compact strip (above status line) ----
    let hints = vec![
        SwarmStripHint {
            key: "alt+↑/↓".into(),
            label: "select".into(),
        },
        SwarmStripHint {
            key: "alt+o".into(),
            label: "open".into(),
        },
        SwarmStripHint {
            key: "alt+shift+p".into(),
            label: "prompt".into(),
        },
        SwarmStripHint {
            key: "esc".into(),
            label: "exit".into(),
        },
    ];
    print_lines(
        "STRIP: unfocused @ width 90",
        &render_swarm_strip(
            &panel_members,
            1,
            false,
            &hints,
            Some("alt+n controls"),
            0,
            90,
            12,
        ),
    );
    print_lines(
        "STRIP: focused, selected #1 @ width 90",
        &render_swarm_strip(&panel_members, 1, true, &hints, None, 3, 90, 12),
    );
    print_lines(
        "STRIP: focused narrow @ width 54",
        &render_swarm_strip(&panel_members, 0, true, &hints, None, 5, 54, 12),
    );

    // ---- Vertical strip (default layout: one agent per row) ----
    let mut vert_members = panel_members.clone();
    for (m, icon) in vert_members.iter_mut().zip(["🦊", "🐝", "🐅", "🦉"]) {
        m.icon = Some(icon.to_string());
        m.task = Some(match m.label.as_str() {
            "researcher" => "wire the auth flow".to_string(),
            "implementer" => "audit the webhook path".to_string(),
            "reviewer" => "support/contact page".to_string(),
            _ => "misc background task".to_string(),
        });
    }
    vert_members[1].todo = Some((3, 9));
    print_lines(
        "VERTICAL: unfocused @ width 90",
        &render_swarm_strip_vertical(
            &vert_members,
            1,
            false,
            &hints,
            Some("alt+n controls"),
            0,
            90,
            4,
            12,
        ),
    );
    print_lines(
        "VERTICAL: focused (accordion), selected #1 @ width 90",
        &render_swarm_strip_vertical(&vert_members, 1, true, &hints, None, 3, 90, 4, 12),
    );
    print_lines(
        "VERTICAL: 7 agents overflow @ width 80",
        &render_swarm_strip_vertical(
            &{
                let mut many = Vec::new();
                for i in 0..7 {
                    let mut m = gm(&format!("agent-{i}"), None, "running", &[]);
                    m.icon = Some("🐜".to_string());
                    many.push(m);
                }
                many
            },
            0,
            false,
            &hints,
            Some("alt+n controls"),
            0,
            80,
            4,
            12,
        ),
    );

    // ---- Dock (vertical agent list for the info-widget margins) ----
    print_lines(
        "DOCK: 4 agents, selected #0, unfocused @ width 34 h 12",
        &render_swarm_dock(&panel_members, 0, false, Some((3, 7)), 0, 34, 12),
    );
    print_lines(
        "DOCK: 4 agents, selected #1, focused @ width 34 h 14",
        &render_swarm_dock(&panel_members, 1, true, Some((3, 7)), 2, 34, 14),
    );
    print_lines(
        "DOCK: narrow @ width 24 h 8",
        &render_swarm_dock(&panel_members, 2, false, None, 0, 24, 8),
    );

    // ---- Compact summary (two lines: tally + plan bar) ----
    print_lines(
        "COMPACT: 4 agents, nodes 5/12 (3 running) @ width 34",
        &render_swarm_compact(&panel_members, Some((5, 3, 12)), 34, 2),
    );
    print_lines(
        "COMPACT: no plan @ width 34",
        &render_swarm_compact(&panel_members, None, 34, 2),
    );
    print_lines(
        "COMPACT: narrow @ width 20",
        &render_swarm_compact(&panel_members, Some((5, 3, 12)), 20, 2),
    );
}
