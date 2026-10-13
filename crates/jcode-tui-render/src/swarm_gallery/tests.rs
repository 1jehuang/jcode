use super::util::disp_w;
use super::*;
use jcode_tui_style::color::rgb;
use ratatui::prelude::Line;

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

#[test]
fn coordinator_sorts_first() {
    let members = vec![
        member("zeta", "running", None, &[]),
        member("alpha", "running", Some("coordinator"), &[]),
    ];
    let tiles = members_to_tiles(&members);
    assert_eq!(tiles[0].title, "alpha");
    assert_eq!(tiles[0].role_glyph.as_deref(), Some("★"));
}

/// `display_order` is the contract callers (e.g. pop-out selection) use to
/// map a displayed row back to an input member, so it must match tile
/// order exactly, including for mixed/unknown roles and sort_key ties.
#[test]
fn display_order_matches_tile_order_for_mixed_members() {
    let mut members = vec![
        member("zeta", "running", None, &[]),
        member("mid", "done", Some("mystery_role_2"), &[]),
        member("boss", "running", Some("coordinator"), &[]),
        member("alpha", "thinking", Some("mystery_role"), &[]),
        member("beta", "failed", None, &[]),
    ];
    // Full tie with "beta" on both role rank and sort_key.
    let mut dup = member("beta-label-2", "ready", None, &[]);
    dup.sort_key = "beta".to_string();
    members.push(dup);

    let order = display_order(&members);
    assert_eq!(order.len(), members.len());
    let ordered_labels: Vec<String> = order.iter().map(|&i| members[i].label.clone()).collect();
    let tile_titles: Vec<String> = members_to_tiles(&members)
        .into_iter()
        .map(|t| t.title)
        .collect();
    assert_eq!(ordered_labels, tile_titles);
    let ordered_labels: Vec<&str> = ordered_labels.iter().map(String::as_str).collect();
    // Role buckets come first (coordinator only), then active-before-
    // finished status order, then sort_key within the same status rank.
    assert_eq!(ordered_labels[0], "boss");
    assert_eq!(
        &ordered_labels[1..],
        &["alpha", "zeta", "beta", "beta-label-2", "mid"]
    );
}

/// Active agents must sort ahead of finished ones within a role bucket, so
/// they stay visible on the strip instead of hiding in the "+N" overflow.
#[test]
fn display_order_puts_active_agents_before_finished_ones() {
    let members = vec![
        member("ant", "completed", None, &[]),
        member("bat", "done", None, &[]),
        member("crab", "running", None, &[]),
        member("dove", "failed", None, &[]),
        member("elk", "ready", None, &[]),
        member("fox", "thinking", None, &[]),
        member("gnu", "stopped", None, &[]),
        member("hen", "blocked", None, &[]),
    ];
    let order = display_order(&members);
    let labels: Vec<&str> = order.iter().map(|&i| members[i].label.as_str()).collect();
    assert_eq!(
        labels,
        // active (crab, fox) < attention (dove, hen) < idle (elk)
        // < finished (ant, bat, gnu), each bucket sorted by sort_key.
        ["crab", "fox", "dove", "hen", "elk", "ant", "bat", "gnu"]
    );
}

#[test]
fn renders_header_and_is_width_bounded() {
    let members = vec![
        member("alpha", "running", None, &["editing config.rs"]),
        member("beta", "done", None, &["reviewed"]),
    ];
    let lines = render_gallery(&members, 80, 12, None);
    assert!(!lines.is_empty());
    let header: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(header.contains("🐝 2 agents · 1 active"), "got: {header}");
    assert!(!header.contains("swarm"), "got: {header}");
    for line in &lines {
        assert!(line.width() <= 80);
    }
}

#[test]
fn active_count_in_header() {
    let members = vec![
        member("a", "running", None, &[]),
        member("b", "thinking", None, &[]),
        member("c", "done", None, &[]),
    ];
    let lines = render_gallery(&members, 100, 12, None);
    let header: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(header.contains("2 active"), "got: {header}");
}

#[test]
fn empty_members_render_nothing() {
    assert!(render_gallery(&[], 80, 12, None).is_empty());
}

#[test]
fn humanize_age_buckets() {
    assert_eq!(humanize_age(0), "now");
    assert_eq!(humanize_age(5), "5s");
    assert_eq!(humanize_age(120), "2m");
    assert_eq!(humanize_age(7200), "2h");
}

fn plain_line(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

#[test]
fn panel_empty_renders_nothing() {
    assert!(render_swarm_panel(&[], 0, true, 60, 12, None).is_empty());
}

#[test]
fn panel_lists_all_agents_and_is_width_bounded() {
    let members = vec![
        member("researcher", "thinking", Some("coordinator"), &["· 1s ago"]),
        member("implementer", "running", None, &["building", "· 3s ago"]),
        member("reviewer", "done", None, &["LGTM", "· 1m ago"]),
    ];
    let lines = render_swarm_panel(&members, 0, true, 70, 14, None);
    assert!(!lines.is_empty());
    for line in &lines {
        assert!(line.width() <= 70, "line too wide: {}", plain_line(line));
    }
    let header = plain_line(&lines[0]);
    assert!(header.contains("🐝 3 agents"), "got: {header}");
    assert!(!header.contains("swarm"), "got: {header}");
    // Every agent label appears as a list row.
    let joined: String = lines.iter().map(plain_line).collect::<Vec<_>>().join("\n");
    for name in ["researcher", "implementer", "reviewer"] {
        assert!(joined.contains(name), "missing {name} in:\n{joined}");
    }
}

#[test]
fn panel_marks_selected_row() {
    let members = vec![
        member("a", "running", Some("coordinator"), &[]),
        member("b", "running", None, &[]),
    ];
    // After sort, coordinator "a" is index 0; selecting 1 marks "b".
    let lines = render_swarm_panel(&members, 1, true, 60, 14, None);
    let selected_row = lines
        .iter()
        .map(plain_line)
        .find(|l| l.contains('▸'))
        .expect("a row should be marked selected");
    assert!(selected_row.contains('b'), "got: {selected_row}");
}

#[test]
fn panel_detail_shows_selected_agent_body() {
    let members = vec![
        member("a", "running", Some("coordinator"), &["alpha work"]),
        member("b", "running", None, &["beta output here"]),
    ];
    let lines = render_swarm_panel(&members, 1, true, 60, 14, None);
    let joined: String = lines.iter().map(plain_line).collect::<Vec<_>>().join("\n");
    // The detail viewport (bordered box) shows the selected agent's tail.
    assert!(joined.contains("beta output here"), "got:\n{joined}");
    // And a bordered box was drawn.
    assert!(
        joined.contains('╭') && joined.contains('╰'),
        "got:\n{joined}"
    );
}

#[test]
fn panel_clamps_out_of_range_selection() {
    let members = vec![member("only", "running", None, &["x"])];
    // selected far beyond range must not panic and still render.
    let lines = render_swarm_panel(&members, 99, true, 40, 12, None);
    assert!(!lines.is_empty());
}

#[test]
fn panel_focus_hint_only_when_focused() {
    let members = vec![member("a", "running", None, &[])];
    let focused = plain_line(&render_swarm_panel(&members, 0, true, 60, 12, None)[0]);
    let unfocused = plain_line(&render_swarm_panel(&members, 0, false, 60, 12, None)[0]);
    assert!(focused.contains("pop out"), "got: {focused}");
    assert!(!unfocused.contains("pop out"), "got: {unfocused}");
}

fn hints() -> Vec<SwarmStripHint> {
    vec![
        SwarmStripHint {
            key: "alt+n".into(),
            label: "focus".into(),
        },
        SwarmStripHint {
            key: "j/k".into(),
            label: "select".into(),
        },
        SwarmStripHint {
            key: "o".into(),
            label: "pop out".into(),
        },
        SwarmStripHint {
            key: "esc".into(),
            label: "back".into(),
        },
    ]
}

#[test]
fn strip_empty_renders_nothing() {
    assert!(render_swarm_strip(&[], 0, true, &hints(), None, 0, 80, 12, None).is_empty());
}

#[test]
fn vertical_strip_empty_renders_nothing() {
    assert!(
        render_swarm_strip_vertical(&[], 0, true, &hints(), None, 0, 80, 4, 12, None).is_empty()
    );
}

#[test]
fn vertical_strip_lists_one_agent_per_row_with_icon_and_task() {
    let mut a = member("fox", "running", None, &[]);
    a.icon = Some("🦊".to_string());
    a.task = Some("wire the auth flow".to_string());
    a.todo = Some((3, 9));
    let mut b = member("bee", "completed", None, &[]);
    b.icon = Some("🐝".to_string());
    b.task = Some("audit the webhook path".to_string());
    let lines = render_swarm_strip_vertical(
        &[a, b],
        0,
        false,
        &hints(),
        Some("alt+n controls"),
        0,
        90,
        4,
        12,
        None,
    );
    assert_eq!(lines.len(), 2, "one row per agent");
    let row0 = plain_line(&lines[0]);
    let row1 = plain_line(&lines[1]);
    assert!(
        row0.contains("🐝"),
        "first row carries the swarm marker: {row0:?}"
    );
    assert!(row0.contains("🦊"), "icon replaces the name: {row0:?}");
    assert!(
        !row0.contains("fox"),
        "name hidden when icon present: {row0:?}"
    );
    assert!(row0.contains("wire the auth flow"), "task shown: {row0:?}");
    assert!(row0.contains("3/9"), "todo counter shown: {row0:?}");
    assert!(row0.contains("1/2 active"), "tally on first row: {row0:?}");
    assert!(
        row0.contains("alt+n controls"),
        "hint on first row: {row0:?}"
    );
    assert!(
        row1.contains("audit the webhook path"),
        "second agent row: {row1:?}"
    );
    assert!(
        !row1.contains("active"),
        "tally only on first row: {row1:?}"
    );
    for line in &lines {
        assert!(line.width() <= 90);
    }
}

#[test]
fn vertical_strip_falls_back_to_name_without_icon() {
    let members = vec![member("zeta", "running", None, &[])];
    let lines = render_swarm_strip_vertical(&members, 0, false, &hints(), None, 0, 80, 4, 12, None);
    assert!(plain_line(&lines[0]).contains("zeta"));
}

#[test]
fn vertical_strip_caps_rows_and_reports_overflow() {
    let members: Vec<GalleryMember> = (0..7)
        .map(|i| member(&format!("agent{i}"), "running", None, &[]))
        .collect();
    let lines = render_swarm_strip_vertical(&members, 0, false, &hints(), None, 0, 80, 4, 12, None);
    assert_eq!(lines.len(), 4, "capped to max_rows lines");
    let last = plain_line(&lines[3]);
    assert!(last.contains("+4 more"), "overflow marker: {last:?}");
}

#[test]
fn vertical_strip_windows_to_keep_selection_visible() {
    let members: Vec<GalleryMember> = (0..7)
        .map(|i| member(&format!("agent{i}"), "running", None, &[]))
        .collect();
    let lines = render_swarm_strip_vertical(&members, 6, true, &[], None, 0, 80, 4, 4, None);
    let text: String = lines.iter().map(plain_line).collect();
    assert!(text.contains("agent6"), "selected agent visible: {text:?}");
}

#[test]
fn vertical_strip_focused_expands_accordion_under_selected_row() {
    let mut fox = member("fox", "running", None, &["compiling the renderer"]);
    fox.icon = Some("🦊".to_string());
    let mut bee = member("bee", "ready", None, &["waiting for work"]);
    bee.icon = Some("🐝".to_string());
    let lines =
        render_swarm_strip_vertical(&[fox, bee], 1, true, &hints(), None, 0, 80, 4, 14, None);
    let texts: Vec<String> = lines.iter().map(plain_line).collect();
    let all = texts.join("\n");
    // Selected agent (bee, sorted second: fox is active and sorts first)
    // shows marker + icon without repeating the animal name; its detail
    // slides in directly beneath.
    let bee_row = texts
        .iter()
        .position(|l| l.contains("▸") && l.contains("🐝"))
        .expect("selected row shows marker and icon: {texts:?}");
    assert!(
        !texts[bee_row].contains("bee"),
        "selected compact row should stay emoji-only: {texts:?}"
    );
    assert!(
        texts[bee_row + 1].contains("waiting for work"),
        "detail expands directly under the selected row: {texts:?}"
    );
    // Unselected agent stays icon-only.
    assert!(
        !texts.iter().any(|l| l.contains("fox") && !l.contains("▸")),
        "unselected agents stay icon-only: {texts:?}"
    );
    assert!(
        !all.contains("compiling the renderer"),
        "unselected agent's transcript stays collapsed: {all:?}"
    );
    assert!(all.contains("pop out"), "hint line shown: {all:?}");
}

#[test]
fn chat_card_is_stable_while_live_card_shows_details() {
    let mut worker = member("reviewer", "running", None, &[]);
    worker.icon = Some("🦕".to_string());
    worker.task = Some("review authentication changes".to_string());
    worker.todo = Some((2, 4));
    worker.model = Some("openai:gpt-5.6-sol".into());
    worker.provider = Some("OpenAI".into());
    worker.auth_method = Some("OAuth".into());
    worker.effort = Some("high".into());
    worker.elapsed_secs = Some(18);
    worker.todo_items = vec![
        GalleryTodo {
            content: "inspect middleware".into(),
            status: "completed".into(),
            tool_intents: Vec::new(),
        },
        GalleryTodo {
            content: "test token refresh flow".into(),
            status: "in_progress".into(),
            tool_intents: vec![
                GalleryToolIntent {
                    tool_name: "read".into(),
                    intent: "Old intent that should roll off".into(),
                    status: "completed".into(),
                    progress: None,
                },
                GalleryToolIntent {
                    tool_name: "agentgrep".into(),
                    intent: "Locate refresh implementation".into(),
                    status: "completed".into(),
                    progress: None,
                },
                GalleryToolIntent {
                    tool_name: "read".into(),
                    intent: "Inspect refresh-token handler".into(),
                    status: "completed".into(),
                    progress: None,
                },
                GalleryToolIntent {
                    tool_name: "bash".into(),
                    intent: "Run targeted authentication tests".into(),
                    status: "running".into(),
                    progress: Some((27, 43, Some("tests".into()))),
                },
            ],
        },
        GalleryTodo {
            content: "check secure cookie configuration".into(),
            status: "pending".into(),
            tool_intents: Vec::new(),
        },
        GalleryTodo {
            content: "report findings".into(),
            status: "pending".into(),
            tool_intents: Vec::new(),
        },
    ];

    let lines = render_swarm_chat_cards(std::slice::from_ref(&worker), 100);
    let all = lines.iter().map(plain_line).collect::<Vec<_>>().join("\n");
    assert_eq!(lines.len(), 1, "chat summary must remain one line: {all}");
    assert!(
        all.contains("🦕 ● reviewer"),
        "assigned agent emoji missing: {all}"
    );
    assert!(
        all.contains("reviewer · Working · GPT-5.6 · OpenAI OAuth"),
        "stable status/model/route metadata missing: {all}"
    );
    assert!(
        STRIP_SPINNER_FRAMES
            .iter()
            .all(|frame| !all.contains(frame)),
        "transcript cards must not animate while being read: {all}"
    );
    assert!(
        !all.contains("00:18")
            && !all.contains("test token refresh flow")
            && !all.contains("Run targeted authentication tests")
            && !all.contains("27/43"),
        "chat summary must exclude frequently changing live details: {all}"
    );

    let live_lines = render_swarm_live_card(&worker, 0, 100, 12);
    let live = live_lines
        .iter()
        .map(plain_line)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        live.contains("reviewer · 00:18 · GPT-5.6 · OpenAI OAuth · high"),
        "live header metadata missing: {live}"
    );
    assert!(
        live.contains("test token refresh flow")
            && live.contains("bash · Run targeted authentication tests · 27/43"),
        "live page should retain todo and tool progress: {live}"
    );
}

/// The transcript card's model/route metadata must degrade gracefully:
/// narrower widths drop trailing pieces rather than overflowing, and a
/// member with no known runtime still renders its status label.
#[test]
fn chat_card_metadata_degrades_and_is_width_bounded() {
    let mut worker = member("reviewer", "running", None, &[]);
    worker.icon = Some("🦕".to_string());
    worker.model = Some("openai:gpt-5.6-sol".into());
    worker.provider = Some("OpenAI".into());
    worker.auth_method = Some("OAuth".into());

    for width in 0..80usize {
        for line in render_swarm_chat_cards(std::slice::from_ref(&worker), width) {
            assert!(
                disp_w(&plain_line(&line)) <= width,
                "chat card exceeded width {width}: {:?}",
                plain_line(&line)
            );
        }
    }

    // Wide: everything fits. Narrow: route drops before model, model before
    // status, and the label always survives.
    let wide = plain_line(&render_swarm_chat_cards(std::slice::from_ref(&worker), 100)[0]);
    assert!(wide.contains("Working · GPT-5.6 · OpenAI OAuth"), "{wide}");
    let mid = plain_line(&render_swarm_chat_cards(std::slice::from_ref(&worker), 34)[0]);
    assert!(
        mid.contains("Working") && !mid.contains("OpenAI OAuth"),
        "{mid}"
    );

    let mut bare = member("plain", "running", None, &[]);
    bare.icon = Some("🦕".to_string());
    let bare_line = plain_line(&render_swarm_chat_cards(std::slice::from_ref(&bare), 100)[0]);
    assert_eq!(bare_line.trim(), "🦕 ● plain · Working");
}

#[test]
fn vertical_strip_unfocused_keeps_live_card_details_in_chat() {
    let mut worker = member("reviewer", "running", None, &[]);
    worker.icon = Some("🐝".to_string());
    worker.task = Some("review authentication changes".to_string());
    worker.todo = Some((2, 4));
    worker.todo_items = vec![GalleryTodo {
        content: "test token refresh flow".into(),
        status: "in_progress".into(),
        tool_intents: Vec::new(),
    }];

    let lines = render_swarm_strip_vertical(
        &[worker],
        0,
        false,
        &hints(),
        Some("alt+n controls"),
        2,
        100,
        4,
        16,
        None,
    );
    let all = lines.iter().map(plain_line).collect::<Vec<_>>().join("\n");
    assert_eq!(lines.len(), 1, "unfocused strip stays compact: {all}");
    assert!(all.contains("🐝"), "assigned icon missing: {all}");
    assert!(
        !all.contains("reviewer"),
        "compact strip should not repeat the animal name: {all}"
    );
    assert!(
        !all.contains("test token refresh flow"),
        "details leaked: {all}"
    );
}

/// Focused vertical strip: plain typing must never be part of the deal.
/// (Key mapping lives in the TUI crate; this just pins that the strip
/// renders hint labels for the alt-chord scheme it advertises.)
#[test]
fn vertical_strip_focused_detail_stays_within_height_budget() {
    let mut m = member(
        "fox",
        "running",
        None,
        &["line 1", "line 2", "line 3", "line 4", "line 5", "line 6"],
    );
    m.todo_items = vec![
        GalleryTodo {
            content: "first".into(),
            status: "completed".into(),
            tool_intents: Vec::new(),
        },
        GalleryTodo {
            content: "second".into(),
            status: "in_progress".into(),
            tool_intents: Vec::new(),
        },
    ];
    let members = vec![m, member("bee", "ready", None, &[])];
    for max_height in 3..=14 {
        let lines = render_swarm_strip_vertical(
            &members,
            0,
            true,
            &hints(),
            None,
            0,
            80,
            4,
            max_height,
            None,
        );
        assert!(
            lines.len() <= max_height,
            "focused vertical strip exceeded max_height {max_height}: {} lines",
            lines.len()
        );
    }
}

#[test]
fn vertical_strip_is_display_width_bounded_at_all_widths() {
    let mut wide = member("調整役エージェント", "running", Some("coordinator"), &[]);
    wide.icon = Some("🐯".to_string());
    wide.task = Some("寬字元邊界の検証テスト for the renderer".to_string());
    wide.todo = Some((12, 34));
    let members = vec![
        wide,
        member("bee", "completed", None, &[]),
        member("fox", "failed", None, &[]),
        member("owl", "thinking", None, &[]),
        member("ram", "ready", None, &[]),
    ];
    for width in 0..=120 {
        for focused in [false, true] {
            let lines = render_swarm_strip_vertical(
                &members,
                2,
                focused,
                &hints(),
                Some("alt+n controls"),
                3,
                width,
                4,
                12,
                None,
            );
            for line in &lines {
                assert!(
                    line.width() <= width,
                    "line wider than {width}: {:?}",
                    plain_line(line)
                );
            }
        }
    }
}

#[test]
fn strip_one_line_when_unfocused_expands_when_focused() {
    let members = vec![
        member("researcher", "thinking", Some("coordinator"), &["working"]),
        member("implementer", "running", None, &["building"]),
    ];
    let unfocused = render_swarm_strip(&members, 0, false, &hints(), None, 0, 80, 12, None);
    assert_eq!(
        unfocused.len(),
        1,
        "unfocused strip should be a single line"
    );
    // Focused: chips line + expanded detail viewport + hint line, bounded
    // by the max_height budget.
    let focused = render_swarm_strip(&members, 0, true, &hints(), None, 0, 80, 12, None);
    assert!(
        focused.len() > 3,
        "focused strip should expand into a detail viewport, got {} lines",
        focused.len()
    );
    assert!(focused.len() <= 12, "focused strip must respect max_height");
    // With a tiny budget the focused strip degrades to the compact
    // 3-line form (chips + one detail line + hints).
    let tiny = render_swarm_strip(&members, 0, true, &hints(), None, 0, 80, 3, None);
    assert_eq!(tiny.len(), 3, "tiny budget should degrade to 3 lines");
}

#[test]
fn strip_focused_detail_prioritizes_todo_names() {
    let mut m = member(
        "researcher",
        "thinking",
        Some("coordinator"),
        &["editing ui.rs", "running tests now"],
    );
    m.todo = Some((1, 3));
    m.todo_items = vec![
        GalleryTodo {
            content: "wire the bus tap".into(),
            status: "completed".into(),
            tool_intents: Vec::new(),
        },
        GalleryTodo {
            content: "carve the gallery band".into(),
            status: "in_progress".into(),
            tool_intents: Vec::new(),
        },
        GalleryTodo {
            content: "run the ui tests".into(),
            status: "pending".into(),
            tool_intents: Vec::new(),
        },
    ];
    let lines = render_swarm_strip(&[m], 0, true, &hints(), None, 0, 80, 14, None);
    let text: Vec<String> = lines.iter().map(plain_line).collect();
    let all = text.join("\n");
    // Todo names replace the less actionable transcript tail when present.
    assert!(!all.contains("editing ui.rs"), "got: {all}");
    assert!(!all.contains("running tests now"), "got: {all}");
    assert!(all.contains("carve the gallery band"), "got: {all}");
    assert!(all.contains("run the ui tests"), "got: {all}");
    // Hint line still present.
    assert!(all.contains("pop out"), "got: {all}");
    for line in &lines {
        assert!(line.width() <= 80, "line too wide: {}", plain_line(line));
    }
}

#[test]
fn strip_shows_agents_and_tally_and_is_width_bounded() {
    let members = vec![
        member("researcher", "thinking", Some("coordinator"), &["working"]),
        member("implementer", "running", None, &["building"]),
        member("reviewer", "done", None, &["done"]),
    ];
    let lines = render_swarm_strip(&members, 1, true, &hints(), None, 0, 90, 12, None);
    for line in &lines {
        assert!(line.width() <= 90, "line too wide: {}", plain_line(line));
    }
    let chips = plain_line(&lines[0]);
    assert!(chips.contains("researcher"), "got: {chips}");
    assert!(chips.contains("implementer"), "got: {chips}");
    assert!(chips.contains("2/3 active"), "tally missing: {chips}");
    // Hint line carries the keybindings.
    let hint = plain_line(lines.last().unwrap());
    assert!(hint.contains("pop out"), "got: {hint}");
    assert!(hint.contains("select"), "got: {hint}");
}

#[test]
fn strip_unfocused_shows_enter_controls_hint() {
    let members = vec![member("a", "running", None, &[])];
    let lines = render_swarm_strip(
        &members,
        0,
        false,
        &hints(),
        Some("alt+n controls"),
        0,
        90,
        12,
        None,
    );
    let chips = plain_line(&lines[0]);
    assert!(chips.contains("alt+n controls"), "got: {chips}");
}

#[test]
fn strip_shows_todo_counter() {
    let mut m = member("worker", "running", None, &["step"]);
    m.todo = Some((8, 16));
    let lines = render_swarm_strip(&[m], 0, false, &hints(), None, 0, 90, 12, None);
    let chips = plain_line(&lines[0]);
    assert!(chips.contains("8/16"), "todo counter missing: {chips}");
}

#[test]
fn strip_shows_task_label_when_width_allows() {
    let mut a = member("fox", "running", None, &[]);
    a.task = Some("fix parser".to_string());
    let mut b = member("owl", "running", None, &[]);
    b.task = Some("write docs".to_string());
    let lines = render_swarm_strip(&[a, b], 0, false, &hints(), None, 0, 100, 12, None);
    let chips = plain_line(&lines[0]);
    assert!(chips.contains("fox·fix parser"), "got: {chips}");
    assert!(chips.contains("owl·write docs"), "got: {chips}");
    assert!(lines[0].width() <= 100);
}

#[test]
fn strip_drops_task_labels_before_hiding_agents() {
    // Same members with and without long task labels: labels are additive
    // only, so they must never reduce how many agents are visible.
    let base: Vec<GalleryMember> = (0..8)
        .map(|i| member(&format!("agent{i}"), "running", None, &[]))
        .collect();
    let with_tasks: Vec<GalleryMember> = base
        .iter()
        .map(|m| {
            let mut m = m.clone();
            m.task = Some("a very long task description that would eat the line".into());
            m
        })
        .collect();
    for width in [40usize, 60, 90, 120] {
        let plain =
            plain_line(&render_swarm_strip(&base, 0, false, &hints(), None, 0, width, 12, None)[0]);
        let labeled_lines =
            render_swarm_strip(&with_tasks, 0, false, &hints(), None, 0, width, 12, None);
        let labeled = plain_line(&labeled_lines[0]);
        assert!(labeled_lines[0].width() <= width);
        let count = |s: &str| (0..8).filter(|i| s.contains(&format!("agent{i}"))).count();
        assert_eq!(
            count(&plain),
            count(&labeled),
            "labels hid agents at width {width}: plain={plain} labeled={labeled}"
        );
    }
}

#[test]
fn strip_task_labels_never_break_width_bound_across_widths() {
    let members: Vec<GalleryMember> = (0..5)
        .map(|i| {
            let mut m = member(&format!("worker-{i}"), "running", None, &[]);
            m.task = Some(format!("task {i}: refactor the swarm gallery renderer"));
            m.todo = Some((i as u32, 9));
            m
        })
        .collect();
    for width in 8..200 {
        let lines = render_swarm_strip(&members, 2, false, &hints(), None, 0, width, 12, None);
        for line in &lines {
            assert!(
                line.width() <= width,
                "width {width} exceeded: {}",
                plain_line(line)
            );
        }
    }
}

#[test]
fn strip_overflow_collapses_to_more_count() {
    let members: Vec<GalleryMember> = (0..12)
        .map(|i| member(&format!("agent-number-{i:02}"), "running", None, &[]))
        .collect();
    let lines = render_swarm_strip(&members, 0, false, &hints(), None, 0, 50, 12, None);
    assert!(lines[0].width() <= 50, "too wide");
    let chips = plain_line(&lines[0]);
    assert!(chips.contains('+'), "expected +N overflow marker: {chips}");
}

#[test]
fn strip_is_display_width_bounded_at_all_widths() {
    use unicode_width::UnicodeWidthStr;
    let mut members: Vec<GalleryMember> = (0..9)
        .map(|i| member(&format!("agent-{i}"), "running", None, &["working"]))
        .collect();
    members[0].todo = Some((3, 8));
    members[0].role = Some("coordinator".into());
    for width in 8..=140 {
        for focused in [false, true] {
            let lines = render_swarm_strip(
                &members,
                2,
                focused,
                &hints(),
                Some("ctrl+shift+tab controls"),
                0,
                width,
                12,
                None,
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

#[test]
fn strip_drops_hint_before_tally_when_narrow() {
    let members: Vec<GalleryMember> = (0..6)
        .map(|i| member(&format!("agent-{i}"), "running", None, &[]))
        .collect();
    // Narrow enough that the long hint cannot fit, but the tally can.
    let lines = render_swarm_strip(
        &members,
        0,
        false,
        &hints(),
        Some("ctrl+shift+tab controls"),
        0,
        60,
        12,
        None,
    );
    let chips = plain_line(&lines[0]);
    assert!(chips.contains("6/6 active"), "tally missing: {chips}");
    assert!(!chips.contains("controls"), "hint should drop: {chips}");
}

#[test]
fn panel_is_display_width_bounded_with_wide_labels() {
    use unicode_width::UnicodeWidthStr;
    let members = vec![
        member(
            "日本語のエージェント名前が長い場合の確認",
            "running",
            None,
            &["· 3s ago"],
        ),
        member("ascii", "done", None, &["· 1m ago"]),
    ];
    for width in 8..=80 {
        for focused in [false, true] {
            for lines in [
                render_swarm_panel(&members, 0, focused, width, 14, None),
                render_gallery(&members, width, 12, None),
            ] {
                for line in &lines {
                    let text = plain_line(line);
                    let w = text.as_str().width();
                    assert!(w <= width, "width {width}: line overflows ({w}): {text:?}");
                }
            }
        }
    }
}

#[test]
fn degenerate_sizes_and_large_member_counts_do_not_panic() {
    let statuses = [
        "spawned",
        "ready",
        "running",
        "thinking",
        "blocked",
        "failed",
        "completed",
        "stopped",
        "unknown-status",
    ];
    for count in [0usize, 1, 3, 50, 400] {
        let members: Vec<GalleryMember> = (0..count)
            .map(|i| {
                let mut m = member(
                    &format!("agent-🐝-{i}"),
                    statuses[i % statuses.len()],
                    if i == 0 { Some("coordinator") } else { None },
                    &["line 一二三四五", "· 2s ago"],
                );
                m.todo = Some((i as u32, (i as u32).max(1)));
                m
            })
            .collect();
        for width in [0usize, 1, 2, 7, 8, 9, 40] {
            for height in [0usize, 1, 2, 3, 7, 20] {
                let _ = render_gallery(&members, width, height, None);
                let _ = render_swarm_panel(&members, count + 5, true, width, height, None);
                let _ = render_swarm_strip(
                    &members,
                    count + 5,
                    true,
                    &hints(),
                    Some("ctrl+t controls"),
                    usize::MAX / 2,
                    width,
                    12,
                    None,
                );
            }
        }
    }
}

#[test]
fn strip_right_tail_is_right_aligned() {
    use unicode_width::UnicodeWidthStr;
    let members = vec![
        member("alpha", "running", None, &[]),
        member("beta", "done", None, &[]),
    ];
    let width = 80;
    let lines = render_swarm_strip(
        &members,
        0,
        false,
        &hints(),
        Some("alt+n controls"),
        0,
        width,
        12,
        None,
    );
    let chips = plain_line(&lines[0]);
    assert!(
        chips.as_str().width() == width,
        "tail should pad to exactly the width: got {} for {chips:?}",
        chips.as_str().width()
    );
    assert!(chips.trim_end().ends_with("controls"), "got: {chips}");
}

#[test]
fn dock_empty_renders_nothing() {
    assert!(render_swarm_dock(&[], 0, false, None, 0, 30, 10).is_empty());
}

#[test]
fn compact_empty_renders_nothing() {
    assert!(render_swarm_compact(&[], Some((1, 1, 3)), 30, 2).is_empty());
}

#[test]
fn compact_shows_agent_tally_node_counts_and_bar() {
    let members = vec![
        member("a", "running", Some("coordinator"), &[]),
        member("b", "thinking", None, &[]),
        member("c", "completed", None, &[]),
        member("d", "blocked", None, &[]),
    ];
    let lines = render_swarm_compact(&members, Some((5, 3, 12)), 32, 2);
    assert_eq!(lines.len(), 2, "expected summary + bar");
    let header = plain_line(&lines[0]);
    assert!(header.contains("2/4 agents"), "got: {header}");
    assert!(header.contains("nodes 5/12"), "got: {header}");
    assert!(header.contains("⚠1"), "got: {header}");

    // Bar: green done, yellow running, dim remainder, exactly `width` cells.
    let bar = &lines[1];
    let bar_text = plain_line(bar);
    assert_eq!(disp_w(&bar_text), 32, "bar fills the width: {bar_text:?}");
    assert_eq!(bar.spans.len(), 3, "done + running + empty segments");
    for span in &bar.spans {
        assert!(span.content.chars().all(|c| c == '▁'), "got: {bar_text:?}");
    }
    assert_eq!(bar.spans[0].style.fg, Some(rgb(100, 200, 100)));
    assert_eq!(bar.spans[1].style.fg, Some(rgb(255, 200, 100)));
}

#[test]
fn compact_without_plan_is_single_line() {
    let members = vec![member("a", "running", None, &[])];
    let lines = render_swarm_compact(&members, None, 30, 2);
    assert_eq!(lines.len(), 1);
    assert!(plain_line(&lines[0]).contains("1/1 agents"));
}

#[test]
fn compact_bar_gives_nonempty_classes_at_least_one_cell() {
    let members = vec![member("a", "running", None, &[])];
    // 1 done + 1 running out of 100: both must still be visible.
    let lines = render_swarm_compact(&members, Some((1, 1, 100)), 20, 2);
    let bar = &lines[1];
    assert!(bar.spans.len() >= 3, "got {} spans", bar.spans.len());
    assert!(!bar.spans[0].content.is_empty());
    assert!(!bar.spans[1].content.is_empty());
}

#[test]
fn compact_is_width_and_height_bounded_at_all_sizes() {
    use unicode_width::UnicodeWidthStr;
    let members: Vec<GalleryMember> = (0..9)
        .map(|i| member(&format!("agent-🐝-{i}"), "running", None, &[]))
        .collect();
    for width in 0..=60 {
        for height in 0..=4 {
            for plan in [
                None,
                Some((0, 0, 0)),
                Some((0, 1, 1)),
                Some((7, 3, 9)),
                Some((u32::MAX, u32::MAX, u32::MAX)),
            ] {
                let lines = render_swarm_compact(&members, plan, width, height);
                assert!(
                    lines.len() <= height.min(2),
                    "w={width} h={height} plan={plan:?}: {} lines",
                    lines.len()
                );
                for line in &lines {
                    let text = plain_line(line);
                    let w = text.as_str().width();
                    assert!(w <= width, "w={width} overflow ({w}): {text:?}");
                }
            }
        }
    }
}

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
