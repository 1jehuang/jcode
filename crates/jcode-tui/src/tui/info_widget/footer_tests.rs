#[test]
fn special_widgets_have_bottom_border_text() {
    use super::{CloudProbeState, CloudStatusInfo, Side, WidgetPlacement};
    use ratatui::{Terminal, backend::TestBackend};
    let data = InfoWidgetData {
        workspace_rows: vec![crate::tui::workspace_map::VisibleWorkspaceRow {
            workspace: 0,
            is_current: true,
            focused_index: Some(0),
            sessions: vec![crate::tui::workspace_map::WorkspaceSessionTile::new("fox")],
        }],
        cloud_info: Some(CloudStatusInfo {
            host: "test-host".into(),
            remote_dir: "/workspace".into(),
            local_dir: None,
            local_is_source: false,
            started_at: std::time::SystemTime::now(),
            last_success: None,
            probe_duration_ms: None,
            state: CloudProbeState::Loading,
            machine: None,
            last_transfer_at: None,
        }),
        ..Default::default()
    };
    // The diagram branch must retain its footer even while image data is absent.
    for kind in [
        WidgetKind::Diagrams,
        WidgetKind::WorkspaceMap,
        WidgetKind::CloudVm,
    ] {
        for width in [12u16, 24, 44] {
            let mut terminal = Terminal::new(TestBackend::new(width, 12)).unwrap();
            terminal
                .draw(|f| {
                    super::render_all(
                        f,
                        &[WidgetPlacement {
                            kind,
                            rect: f.area(),
                            side: Side::Right,
                        }],
                        &data,
                    );
                })
                .unwrap();
            let buf = terminal.backend().buffer();
            let bottom = (0..12)
                .find(|&y| buf[(0, y)].symbol() == "╰")
                .expect("rendered widget must have a bottom border");
            let footer: String = (1..width - 1).map(|x| buf[(x, bottom)].symbol()).collect();
            assert!(
                footer.chars().any(char::is_alphabetic),
                "{kind:?}: {footer}"
            );
            for redundant in ["Workspace layout", "Remote machine", "Latest diagram"] {
                assert!(!footer.contains(redundant), "{kind:?}: {footer}");
            }
        }
    }
}

#[test]
fn metadata_footers_add_distinct_compact_facts() {
    let mut data = InfoWidgetData {
        observed_context_tokens: Some(25_000),
        context_limit: Some(100_000),
        diagrams: vec![super::DiagramInfo {
            hash: 0,
            width: 640,
            height: 480,
            label: None,
        }],
        ..Default::default()
    };
    assert_eq!(super::overview_footer(&data), "75K ctx left");
    assert_eq!(super::diagram_footer(&data), "640×480");
    data.diagrams.push(data.diagrams[0].clone());
    assert_eq!(super::diagram_footer(&data), "640×480 · 1/2");
    data.context_info_stale = true;
    assert_eq!(super::overview_footer(&data), "context syncing");
    data.context_info_stale = false;
    data.observed_context_tokens = Some(110_000);
    assert_eq!(super::overview_footer(&data), "0 ctx left");

    data.compaction_info = Some(super::CompactionInfo {
        is_compacting: false,
        compacted_messages: 20,
        active_messages: 4,
        summary_chars: 4_000,
        mode: "semantic".into(),
    });
    let framed = super::render_compaction_widget(&data, Rect::new(0, 0, 40, 6));
    let body = lines_text_concat(&framed.lines);
    let footer = lines_text_concat(framed.footer.as_slice());
    assert!(body.contains("20 old · 4 active"));
    assert!(footer.contains("tok") && footer.contains("semantic"));
    assert!(!body.contains("tok") && !body.contains("semantic"));
    assert!(framed.title_right.is_none());
}

fn footer_probe(kind: WidgetKind, data: &InfoWidgetData, width: u16) -> (String, String) {
    use ratatui::{Terminal, backend::TestBackend};

    let mut terminal = Terminal::new(TestBackend::new(width, 16)).unwrap();
    terminal
        .draw(|frame| {
            super::render_all(
                frame,
                &[super::WidgetPlacement {
                    kind,
                    rect: frame.area(),
                    side: super::Side::Right,
                }],
                data,
            );
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    let bottom = (0..16)
        .find(|&y| buffer[(0, y)].symbol() == "╰")
        .expect("public renderer must preserve the bottom border");
    assert_eq!(buffer[(width - 1, bottom)].symbol(), "╯");
    let row = |y| {
        (0..width)
            .map(|x| buffer[(x, y)].symbol())
            .collect::<String>()
    };
    let footer = row(bottom);
    assert!(
        footer
            .chars()
            .any(|ch| !ch.is_whitespace() && !"╰╯─".contains(ch)),
        "{kind:?} at {width} cells lost its footer: {footer}"
    );
    let above = (0..bottom).map(row).collect::<Vec<_>>().join("\n");
    (above, footer)
}

#[test]
fn public_runtime_footer_keeps_throughput_once_without_wrapping() {
    let data = InfoWidgetData {
        provider_name: Some("openai".into()),
        service_tier: Some("priority".into()),
        connection_type: Some("websocket".into()),
        tokens_per_second: Some(61.7),
        ..Default::default()
    };
    for width in [8, 12, 24, 44] {
        let (above, footer) = footer_probe(WidgetKind::ModelInfo, &data, width);
        assert!(!above.contains("tok/s"));
        assert!(!footer.contains("Runtime"));
        if width >= 24 {
            assert!(footer.contains("62 tok/s"), "{footer}");
            assert_eq!(footer.matches("62 tok/s").count(), 1);
        }
        if width == 44 {
            assert!(above.contains("fast tier") && above.contains("websocket"));
        }
    }
}

#[test]
fn public_memory_footer_drops_stale_trace_and_keeps_scope() {
    let now = Instant::now();
    let mut data = InfoWidgetData {
        memory_info: Some(MemoryInfo {
            total_count: 7,
            project_count: 4,
            global_count: 3,
            activity: Some(MemoryActivity {
                state: MemoryState::Embedding,
                state_since: now,
                pipeline: None,
                recent_events: vec![MemoryEvent {
                    kind: MemoryEventKind::MemoryInjected {
                        count: 2,
                        prompt_chars: 318,
                        age_ms: 44,
                        preview: "prefers terse answers".into(),
                        items: Vec::new(),
                    },
                    timestamp: now,
                    detail: None,
                }],
            }),
            ..Default::default()
        }),
        ..Default::default()
    };
    for width in [8, 12, 24, 44] {
        let (above, footer) = footer_probe(WidgetKind::MemoryActivity, &data, width);
        assert!(!above.contains("Trace:"));
        if width >= 24 {
            assert!(footer.contains("Trace:"), "{footer}");
        }
    }
    let activity = data
        .memory_info
        .as_mut()
        .unwrap()
        .activity
        .as_mut()
        .unwrap();
    activity.state = MemoryState::Idle;
    activity.state_since = now - Duration::from_secs(60);
    for width in [8, 12, 24, 44] {
        let (above, footer) = footer_probe(WidgetKind::MemoryActivity, &data, width);
        assert!(!above.contains("Trace:") && !footer.contains("Trace:"));
        if width >= 24 {
            assert!(footer.contains("4 project · 3 global"), "{footer}");
            assert!(!above.contains("4 project"));
        }
    }
}

#[test]
fn public_swarm_footer_tracks_output_clients_and_overflow() {
    let mut first = managed_member("fox", "running", None);
    first.runtime.output_tokens = Some(12_000);
    let mut second = managed_member("owl", "completed", None);
    second.runtime.output_tokens = Some(8_000);
    let mut data = InfoWidgetData {
        swarm_info: Some(SwarmInfo {
            managed_members: vec![first, second],
            client_count: Some(1),
            ..Default::default()
        }),
        ..Default::default()
    };
    for width in [8, 12, 24, 44] {
        let (above, footer) = footer_probe(WidgetKind::SwarmStatus, &data, width);
        assert!(!above.contains("20K out"));
        if width >= 12 {
            assert!(footer.contains("20K out"), "{footer}");
        }
    }
    for member in &mut data.swarm_info.as_mut().unwrap().managed_members {
        member.runtime.output_tokens = None;
    }
    let (_, footer) = footer_probe(WidgetKind::SwarmStatus, &data, 44);
    assert!(footer.contains("1 client"), "{footer}");
    assert!(!footer.contains("1 clients"), "{footer}");

    data.swarm_info = Some(SwarmInfo {
        session_count: 4,
        session_names: vec!["one".into(), "two".into(), "three".into(), "four".into()],
        client_count: Some(1),
        ..Default::default()
    });
    for width in [8, 12, 24, 44] {
        let (above, footer) = footer_probe(WidgetKind::SwarmStatus, &data, width);
        assert!(!above.contains("1 client"));
        if width >= 12 {
            assert!(footer.contains("+1 more"), "{footer}");
        }
        if width == 44 {
            assert!(footer.contains("1 client"), "{footer}");
        }
    }
}

#[test]
fn public_background_footer_distinguishes_states_and_preserves_overflow() {
    use crate::tui::{BackgroundTaskRow, BackgroundTaskRowStatus};

    let row = |status| BackgroundTaskRow {
        task_id: "task".into(),
        label: "build".into(),
        percent: Some(100.0),
        status,
        completed_at: Some(Instant::now()),
    };
    let mut data = InfoWidgetData::default();
    for (status, legend) in [
        (BackgroundTaskRowStatus::Running, "◌ running"),
        (BackgroundTaskRowStatus::Completed, "✓ complete"),
        (BackgroundTaskRowStatus::Failed, "✗ failed"),
    ] {
        data.background_info = Some(super::BackgroundInfo {
            rows: vec![row(status)],
            ..Default::default()
        });
        for width in [8, 12, 24, 44] {
            let (above, footer) = footer_probe(WidgetKind::BackgroundTasks, &data, width);
            assert!(!above.contains(legend));
            if width >= 24 {
                assert!(footer.contains(legend), "{footer}");
            }
        }
    }
    data.background_info = Some(super::BackgroundInfo {
        rows: vec![
            row(BackgroundTaskRowStatus::Running),
            row(BackgroundTaskRowStatus::Completed),
        ],
        ..Default::default()
    });
    let (_, footer) = footer_probe(WidgetKind::BackgroundTasks, &data, 44);
    assert!(footer.contains("◌ running · ✓ complete"), "{footer}");

    data.background_info = Some(super::BackgroundInfo {
        running_count: 8,
        running_tasks: (0..8).map(|index| format!("task {index}")).collect(),
        ..Default::default()
    });
    let (_, footer) = footer_probe(WidgetKind::BackgroundTasks, &data, 44);
    assert!(footer.contains("+2 more"), "{footer}");
    data.background_info.as_mut().unwrap().running_tasks.clear();
    let (_, footer) = footer_probe(WidgetKind::BackgroundTasks, &data, 44);
    assert!(footer.contains("this session"), "{footer}");
}

#[test]
fn public_workspace_footer_counts_visible_sessions_and_running_state() {
    use crate::tui::workspace_map::{
        VisibleWorkspaceRow, WorkspaceSessionTile, WorkspaceSessionVisualState,
    };

    let mut data = InfoWidgetData {
        workspace_rows: vec![VisibleWorkspaceRow {
            workspace: 0,
            is_current: true,
            focused_index: Some(0),
            sessions: vec![WorkspaceSessionTile::new("雲")],
        }],
        ..Default::default()
    };
    let (above, footer) = footer_probe(WidgetKind::WorkspaceMap, &data, 44);
    assert!(above.contains("Workspace"));
    assert!(footer.contains("1 session"));
    assert!(!footer.contains("sessions"));

    data.workspace_rows[0].sessions[0].state = WorkspaceSessionVisualState::Running;
    data.workspace_rows.push(VisibleWorkspaceRow {
        workspace: 1,
        is_current: false,
        focused_index: Some(0),
        sessions: vec![
            WorkspaceSessionTile::with_state("狐", WorkspaceSessionVisualState::Running),
            WorkspaceSessionTile::new("bird"),
        ],
    });
    for width in [8, 12, 24, 44] {
        let (above, footer) = footer_probe(WidgetKind::WorkspaceMap, &data, width);
        assert!(!above.contains("3 sessions"));
        if width == 44 {
            assert!(footer.contains("3 sessions · 2 running"), "{footer}");
        }
    }
}

#[test]
fn public_diagram_footer_keeps_dimensions_when_image_is_not_cached() {
    let mut data = InfoWidgetData::default();
    let (_, footer) = footer_probe(WidgetKind::Diagrams, &data, 44);
    assert!(footer.contains("image pending"));
    data.diagrams = vec![
        super::DiagramInfo {
            hash: u64::MAX,
            width: 640,
            height: 480,
            label: None,
        };
        2
    ];
    for width in [8, 12, 24, 44] {
        let (above, footer) = footer_probe(WidgetKind::Diagrams, &data, width);
        assert!(!above.contains("640×480") && !above.contains("1/2"));
        if width >= 24 {
            assert!(footer.contains("640×480 · 1/2"), "{footer}");
        }
    }
}

#[test]
fn ambient_footer_moves_next_run_out_of_body() {
    let data = InfoWidgetData {
        ambient_info: Some(super::AmbientWidgetData {
            show_widget: true,
            status: crate::ambient::AmbientStatus::Idle,
            queue_count: 1,
            next_queue_preview: Some("check CI".into()),
            reminder_count: 0,
            next_reminder_preview: None,
            last_run_ago: None,
            last_summary: None,
            next_wake: Some("in 3m".into()),
            next_reminder_wake: None,
            budget_percent: None,
        }),
        ..Default::default()
    };
    let framed =
        super::render_widget_content(WidgetKind::AmbientMode, &data, Rect::new(0, 0, 40, 6));
    assert_eq!(lines_text_concat(framed.footer.as_slice()), "next in 3m");
    assert!(!lines_text_concat(&framed.lines).contains("3m"));
    assert_eq!(framed.lines.len(), 1);
}

#[test]
fn narrow_overview_background_row_keeps_progress_visible() {
    use crate::tui::{BackgroundTaskRow, BackgroundTaskRowStatus};
    use ratatui::{Terminal, backend::TestBackend};
    let data = InfoWidgetData {
        model: Some("claude-opus-5-5".into()),
        background_info: Some(BackgroundInfo {
            rows: vec![BackgroundTaskRow {
                task_id: "t".into(),
                label: "Live verification task".into(),
                percent: Some(20.0),
                status: BackgroundTaskRowStatus::Running,
                completed_at: None,
            }],
            ..Default::default()
        }),
        usage_info: Some(UsageInfo {
            provider: UsageProvider::Anthropic,
            five_hour: 0.3,
            seven_day: 0.5,
            available: true,
            ..Default::default()
        }),
        ..Default::default()
    };
    let (w, h) = (24u16, 9u16);
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| {
        super::render_all(
            f,
            &[super::WidgetPlacement {
                kind: WidgetKind::Overview,
                rect: Rect::new(0, 0, w, h),
                side: super::Side::Right,
            }],
            &data,
        )
    })
    .unwrap();
    let buf = term.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..h {
        for x in 0..w {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    assert!(out.contains("◌ Live"), "{out}");
    assert!(
        out.contains("20%"),
        "progress must fit at overview width:\n{out}"
    );
}
