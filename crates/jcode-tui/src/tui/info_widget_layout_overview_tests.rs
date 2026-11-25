/// Model + context only: the typical session data behind the right-hand box.
/// Model and context live on the status line, so the Overview is built from
/// the detail sections a typical session also has: the transport (Runtime) and
/// subscription limits.
fn model_and_context_data() -> InfoWidgetData {
    InfoWidgetData {
        model: Some("GLM-5.3".to_string()),
        provider_name: Some("e2e-mock".to_string()),
        connection_type: Some("websocket".to_string()),
        usage_info: Some(crate::tui::info_widget::UsageInfo {
            provider: crate::tui::info_widget::UsageProvider::Anthropic,
            five_hour: 0.2,
            seven_day: 0.4,
            available: true,
            ..Default::default()
        }),
        context_info: Some(crate::prompt::ContextInfo {
            system_prompt_chars: 20_000,
            total_chars: 200_000,
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Ragged chat: a full-width line every `period` rows, so the tallest free
/// pocket is `period - 1` rows.
fn ragged_margins(period: usize, scroll_top: usize) -> Margins {
    let free: Vec<u16> = (0..40)
        .map(|r| {
            if (scroll_top + r) % period == period - 1 {
                4
            } else {
                60
            }
        })
        .collect();
    Margins {
        right_widths: free.clone(),
        right_reliable: free,
        scroll_top,
        ..Default::default()
    }
}

fn right_kinds(placements: &[WidgetPlacement]) -> Vec<WidgetKind> {
    placements
        .iter()
        .filter(|p| p.side == Side::Right)
        .map(|p| p.kind)
        .collect()
}

/// A pocket tall enough for the combined box must get the combined box, not
/// its parts scattered as separate boxes. Previously Overview required a fixed
/// 10-row pocket even though it renders ~5 rows, so a ragged chat split it.
#[test]
fn overview_stays_whole_when_pocket_fits_its_real_height() {
    let data = model_and_context_data();
    let area = Rect::new(0, 0, 140, 40);
    let overview_h = calculate_widget_height(WidgetKind::Overview, &data, 40, 40);
    assert!(
        overview_h > 2 && overview_h < 10,
        "overview height {overview_h}"
    );
    // Pockets of 8 free rows: taller than the box, shorter than the old fixed 10.
    let out = calculate_placements_anchored(area, &ragged_margins(9, 0), &data, true, &[]);
    assert_placements_sane("ragged 9", area, &out.visible);
    assert_eq!(right_kinds(&out.visible), vec![WidgetKind::Overview]);
}

/// Over a long ragged chat that scrolls one line per frame, the right side
/// never shows the combined box and one of its parts at the same time, and
/// shows at most one box whenever the combined box fits.
#[test]
fn growing_ragged_chat_never_shows_overview_parts_alongside_overview() {
    let data = model_and_context_data();
    let area = Rect::new(0, 0, 140, 40);
    let mut anchors: Vec<WidgetAnchor> = Vec::new();
    for frame in 0..300usize {
        let out =
            calculate_placements_anchored(area, &ragged_margins(9, frame), &data, true, &anchors);
        assert_placements_sane("growing chat", area, &out.visible);
        let kinds = right_kinds(&out.visible);
        assert!(
            kinds.len() <= 1,
            "frame {frame}: right side fragmented into {kinds:?}"
        );
        anchors = out.anchors;
    }
}

fn right_anchor(kind: WidgetKind, y: u16, h: u16) -> WidgetAnchor {
    WidgetAnchor {
        placement: WidgetPlacement {
            kind,
            rect: Rect::new(100, y, 40, h),
            side: Side::Right,
        },
        hidden_frames: 0,
        content_top: y as usize,
    }
}

fn roomy_margins() -> Margins {
    Margins {
        right_widths: vec![60; 40],
        right_reliable: vec![60; 40],
        scroll_top: 0,
        ..Default::default()
    }
}

/// Review #1456 (1): an Overview seated in a slot sized for model + context
/// must not stay in that slot once background tasks make it taller - holding it would
/// suppress the parts and render nothing (the content no longer fits).
#[test]
fn overview_that_outgrows_its_slot_rehomes_instead_of_going_blank() {
    let mut data = model_and_context_data();
    let small = calculate_widget_height(WidgetKind::Overview, &data, 40, 40);
    let anchors = vec![right_anchor(WidgetKind::Overview, 3, small)];
    data.background_info = Some(BackgroundInfo {
        running_count: 3,
        running_tasks: vec!["compile".into(), "validate".into(), "package".into()],
        ..Default::default()
    });
    let grown = calculate_widget_height(WidgetKind::Overview, &data, 40, 40);
    assert!(
        grown > small,
        "precondition: background tasks grow overview ({small} -> {grown})"
    );
    let area = Rect::new(0, 0, 140, 40);
    let out = calculate_placements_anchored(area, &roomy_margins(), &data, true, &anchors);
    assert_placements_sane("outgrown", area, &out.visible);
    let overview = out
        .visible
        .iter()
        .find(|p| p.kind == WidgetKind::Overview)
        .expect("overview still shown");
    assert!(
        overview.rect.height >= grown,
        "overview kept a {}-row slot for {grown}-row content",
        overview.rect.height
    );
}

/// Scroll the viewport over a ragged transcript like a mouse wheel does (a few
/// lines per step) and record what the right-hand box looks like each frame.
fn scroll_frames(
    data: &InfoWidgetData,
    area: Rect,
    margins_at: impl Fn(usize) -> Margins,
    scroll_tops: impl Iterator<Item = usize>,
) -> Vec<Vec<WidgetPlacement>> {
    let mut anchors: Vec<WidgetAnchor> = Vec::new();
    let mut frames = Vec::new();
    for scroll_top in scroll_tops {
        let out =
            calculate_placements_anchored(area, &margins_at(scroll_top), data, true, &anchors);
        assert_placements_sane(&format!("scroll {scroll_top}"), area, &out.visible);
        anchors = out.anchors;
        frames.push(out.visible);
    }
    frames
}

/// Reported bug: scrolling the chat broke the right-hand box apart. The box
/// rode its transcript line until that line scrolled past the viewport edge,
/// was dropped, and was re-homed from scratch into whatever pocket had settled
/// that frame, which usually only fit its parts. It came back as separate
/// Context / KV cache / Model boxes. Residents now stop at the viewport edge.
#[test]
fn scrolling_up_keeps_overview_whole_instead_of_splitting_it() {
    let data = contended_overview_data();
    let area = Rect::new(0, 0, 140, 40);
    // What the live trace showed: the text leaves room for the whole box
    // (long lines every 13 rows), but the settled space a *new* widget may be
    // placed in is scarce (pockets of 5 rows), which fits only the parts.
    let margins_at = |scroll_top: usize| {
        let wall = |r: usize, period: usize| (scroll_top + r).is_multiple_of(period);
        Margins {
            right_widths: (0..40).map(|r| if wall(r, 13) { 4 } else { 60 }).collect(),
            right_reliable: (0..40).map(|r| if wall(r, 6) { 4 } else { 60 }).collect(),
            scroll_top,
            ..Default::default()
        }
    };
    // Settle the box in a roomy frame at the bottom, then wheel up 3 lines at a time.
    let mut anchors = Vec::new();
    let roomy = calculate_placements_anchored(area, &roomy_margins_at(600), &data, true, &anchors);
    anchors = roomy.anchors;
    assert_eq!(right_kinds(&roomy.visible), vec![WidgetKind::Overview]);
    let mut scattered = 0;
    let mut shown = 0;
    for step in 1..=60 {
        let scroll_top = 600 - step * 3;
        let out =
            calculate_placements_anchored(area, &margins_at(scroll_top), &data, true, &anchors);
        assert_placements_sane(&format!("wheel {step}"), area, &out.visible);
        anchors = out.anchors;
        let kinds = right_kinds(&out.visible);
        if kinds == vec![WidgetKind::Overview] {
            shown += 1;
        } else if !kinds.is_empty() {
            scattered += 1;
        }
    }
    assert_eq!(
        scattered, 0,
        "Overview split into its parts while scrolling"
    );
    assert!(
        shown >= 50,
        "Overview visible on only {shown}/60 wheel steps"
    );
}

/// Guard for edge sticking: scrolling down (toward newer messages) pushes the
/// box past the bottom edge, and it must stay whole there.
#[test]
fn scrolling_down_keeps_overview_whole_at_bottom_edge() {
    let data = contended_overview_data();
    let area = Rect::new(0, 0, 140, 40);
    let frames = scroll_frames(&data, area, roomy_margins_at, (0..40).map(|s| 100 + s * 3));
    for (i, frame) in frames.iter().enumerate() {
        assert_eq!(
            right_kinds(frame),
            vec![WidgetKind::Overview],
            "frame {i}: {frame:?}"
        );
    }
}

/// Guard for edge sticking: a resident stuck at an edge must not bounce. An
/// earlier version of the fix re-bound it to the line it was pushed onto, so
/// it rode away from the edge and snapped back every frame.
#[test]
fn edge_stuck_overview_does_not_saw_tooth() {
    let data = contended_overview_data();
    let area = Rect::new(0, 0, 140, 40);
    let frames = scroll_frames(&data, area, roomy_margins_at, (0..30).map(|s| 900 - s * 3));
    let ys: Vec<u16> = frames
        .iter()
        .skip(15)
        .map(|f| {
            f.iter()
                .find(|p| p.kind == WidgetKind::Overview)
                .unwrap()
                .rect
                .y
        })
        .collect();
    assert!(
        ys.windows(2).all(|w| w[0] == w[1]),
        "edge-stuck overview moved: {ys:?}"
    );
}

/// Guard for edge sticking: two residents pushed toward the same edge must not
/// be drawn on top of each other (seen live while developing the fix).
#[test]
fn edge_stuck_residents_never_overlap() {
    let data = model_and_context_data();
    let area = Rect::new(0, 0, 140, 40);
    let anchors = vec![
        right_anchor(WidgetKind::KvCache, 30, 5),
        right_anchor(WidgetKind::ModelInfo, 34, 3),
    ];
    let mut prev = anchors;
    for step in 0..20 {
        let out =
            calculate_placements_anchored(area, &roomy_margins_at(step * 3), &data, true, &prev);
        assert_placements_sane(&format!("step {step}"), area, &out.visible);
        prev = out.anchors;
    }
}

fn roomy_margins_at(scroll_top: usize) -> Margins {
    Margins {
        scroll_top,
        ..roomy_margins()
    }
}

/// Model + context + KV cache + git: the Overview is ~8 rows, taller than the
/// pockets a ragged chat usually leaves, while each part fits on its own.
fn contended_overview_data() -> InfoWidgetData {
    InfoWidgetData {
        background_info: Some(BackgroundInfo {
            running_count: 2,
            running_tasks: vec!["compile".into(), "validate".into()],
            ..Default::default()
        }),
        ..model_and_context_data()
    }
}

#[test]
fn edge_stuck_overview_stays_below_synthetic_top_rows() {
    let data = model_and_context_data();
    let area = Rect::new(0, 3, 140, 40);
    let initial = calculate_placements_anchored(area, &roomy_margins(), &data, true, &[]);
    let mut anchors = initial.anchors;
    for scroll_top in 1000..1004 {
        let margins = Margins {
            content_start_row: 5,
            scroll_top,
            ..roomy_margins()
        };
        let out = calculate_placements_anchored(area, &margins, &data, true, &anchors);
        assert_placements_sane("sticky top band", area, &out.visible);
        let overview = out
            .visible
            .iter()
            .find(|p| p.kind == WidgetKind::Overview)
            .expect("overview remains whole");
        assert_eq!(overview.rect.y, area.y + 5, "clamp below synthetic chrome");
        anchors = out.anchors;
    }
}
