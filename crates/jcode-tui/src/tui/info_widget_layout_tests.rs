//! Regression tests for multi-widget placement contention and degenerate
//! terminal sizes.
//!
//! These pin three invariants of [`calculate_placements_anchored`]:
//!
//! 1. Panic safety: any combination of tiny margins/areas (including margin
//!    profiles that disagree with the area size) must never panic.
//! 2. Containment: every placement stays inside the messages area, so widgets
//!    can never clobber the banner above or the input/status rows below.
//! 3. Disjointness: placed widgets never overlap each other, even when several
//!    widgets (todos, background, overview, memory, ...) compete for the same
//!    margin pockets and lower-priority widgets are silently dropped.

use super::*;
use crate::tui::info_widget::{
    BackgroundInfo, CacheHitInfo, CompactionInfo, GitInfo, InfoWidgetData, MemoryInfo, SwarmInfo,
    UsageInfo, UsageProvider,
};

fn todo(id: &str, status: &str) -> crate::todo::TodoItem {
    crate::todo::TodoItem {
        group: None,
        content: format!("task {id}"),
        status: status.to_string(),
        priority: "high".to_string(),
        id: id.to_string(),
        blocked_by: Vec::new(),
        assigned_to: None,
        confidence: None,
        completion_confidence: None,
        confidence_history: Vec::new(),
    }
}

/// Kitchen-sink data: every enabled widget kind is eligible at once, so the
/// placement pass has maximum contention for margin space.
fn contended_data() -> InfoWidgetData {
    InfoWidgetData {
        model: Some("claude-test-1".to_string()),
        provider_name: Some("anthropic".to_string()),
        session_count: Some(3),
        queue_mode: Some(true),
        context_info: Some(crate::prompt::ContextInfo {
            system_prompt_chars: 20_000,
            total_chars: 60_000,
            ..Default::default()
        }),
        todos: vec![todo("t1", "in_progress"), todo("t2", "pending")],
        memory_info: Some(MemoryInfo {
            total_count: 4,
            ..Default::default()
        }),
        swarm_info: Some(SwarmInfo {
            session_count: 4,
            subagent_status: Some("running subtask".to_string()),
            session_names: vec!["alpha".to_string(), "beta".to_string()],
            // Managed agents make the SwarmStatus dock eligible, adding a
            // high-priority contender to the placement contention.
            managed_members: vec![
                crate::protocol::SwarmMemberStatus {
                    session_id: "worker-1".to_string(),
                    friendly_name: Some("worker-1".to_string()),
                    status: "running".to_string(),
                    detail: None,
                    task_label: None,
                    role: Some("coordinator".to_string()),
                    is_headless: Some(true),
                    live_attachments: None,
                    status_age_secs: Some(2),
                    output_tail: Some("doing work".to_string()),
                    report_back_to_session_id: Some("parent".to_string()),
                    todo_progress: Some((1, 4)),
                    todo_items: Vec::new(),
                    runtime: crate::protocol::SwarmMemberRuntime::default(),
                },
                crate::protocol::SwarmMemberStatus {
                    session_id: "worker-2".to_string(),
                    friendly_name: Some("worker-2".to_string()),
                    status: "blocked".to_string(),
                    detail: None,
                    task_label: None,
                    role: None,
                    is_headless: Some(true),
                    live_attachments: None,
                    status_age_secs: Some(30),
                    output_tail: None,
                    report_back_to_session_id: Some("parent".to_string()),
                    todo_progress: None,
                    todo_items: Vec::new(),
                    runtime: crate::protocol::SwarmMemberRuntime::default(),
                },
            ],
            plan_progress: Some((3, 1, 7)),
            ..Default::default()
        }),
        background_info: Some(BackgroundInfo {
            running_count: 2,
            running_tasks: vec!["bash".to_string(), "task".to_string()],
            ..Default::default()
        }),
        usage_info: Some(UsageInfo {
            provider: UsageProvider::Anthropic,
            primary_limit_label: Some("5-hour".to_string()),
            five_hour: 0.4,
            secondary_limit_label: Some("Weekly".to_string()),
            seven_day: 0.6,
            available: true,
            ..Default::default()
        }),
        cache_hit_info: Some(CacheHitInfo {
            reported_input_tokens: 2_000,
            prompt_tokens: Some(2_000),
            read_tokens: 1_500,
            ..Default::default()
        }),
        compaction_info: Some(CompactionInfo {
            is_compacting: false,
            compacted_messages: 12,
            active_messages: 6,
            summary_chars: 500,
            mode: "auto".to_string(),
        }),
        git_info: Some(GitInfo {
            branch: "master".to_string(),
            modified: 2,
            staged: 1,
            untracked: 1,
            ahead: 1,
            behind: 0,
            dirty_files: vec!["a.rs".to_string(), "b.rs".to_string()],
        }),
        ..Default::default()
    }
}

fn rects_overlap(a: &Rect, b: &Rect) -> bool {
    let ax2 = a.x as u32 + a.width as u32;
    let ay2 = a.y as u32 + a.height as u32;
    let bx2 = b.x as u32 + b.width as u32;
    let by2 = b.y as u32 + b.height as u32;
    ((a.x as u32) < bx2 && (b.x as u32) < ax2) && ((a.y as u32) < by2 && (b.y as u32) < ay2)
}

/// All placements must be non-degenerate, inside the messages area, and
/// pairwise disjoint.
fn assert_placements_sane(label: &str, area: Rect, placements: &[WidgetPlacement]) {
    for p in placements {
        assert!(
            p.rect.width > 0 && p.rect.height > 0,
            "{label}: degenerate placement {:?}",
            p
        );
        assert!(
            p.rect.x >= area.x
                && p.rect.y >= area.y
                && p.rect.x as u32 + p.rect.width as u32 <= area.x as u32 + area.width as u32
                && p.rect.y as u32 + p.rect.height as u32 <= area.y as u32 + area.height as u32,
            "{label}: placement escapes area {area:?}: {:?}",
            p
        );
    }
    for i in 0..placements.len() {
        for j in (i + 1)..placements.len() {
            assert!(
                !rects_overlap(&placements[i].rect, &placements[j].rect),
                "{label}: placements overlap: {:?} vs {:?}",
                placements[i],
                placements[j]
            );
        }
    }
}

fn margins_for(width: u16, rows: usize, centered: bool) -> Margins {
    Margins {
        right_widths: vec![width; rows],
        left_widths: if centered {
            vec![width; rows]
        } else {
            Vec::new()
        },
        centered,
        scroll_top: 10,
        ..Default::default()
    }
}

/// (a) Panic safety and containment across degenerate terminal sizes with all
/// widgets eligible: margin widths 0-12 (plus a few placeable widths) and
/// heights 0-5 (plus a few placeable heights), fresh and with carried anchors.
#[test]
fn degenerate_sizes_with_full_contention_never_panic_or_escape() {
    let data = contended_data();

    // Anchors captured from a healthy frame, then replayed into every
    // degenerate frame to exercise the Phase 1 (pinned-anchor) paths too.
    // Note: real margins always partition free space around content, so
    // left + right widths never exceed the area width; the synthetic margins
    // here respect that invariant (40 + 40 <= 100).
    let healthy_area = Rect::new(2, 1, 100, 12);
    let healthy =
        calculate_placements_anchored(healthy_area, &margins_for(40, 12, false), &data, true, &[]);
    assert!(
        !healthy.visible.is_empty(),
        "healthy frame should place at least one widget"
    );

    let widths: Vec<u16> = (0..=12).chain([24, 30, 40]).collect();
    let heights: Vec<u16> = (0..=5).chain([6, 8, 12]).collect();
    for &margin_w in &widths {
        for &h in &heights {
            for centered in [false, true] {
                let label = format!("margin_w={margin_w} h={h} centered={centered}");
                let area = Rect::new(2, 1, 100, h);
                let margins = margins_for(margin_w, h as usize, centered);

                let fresh = calculate_placements_anchored(area, &margins, &data, true, &[]);
                assert_placements_sane(&format!("{label} (fresh)"), area, &fresh.visible);

                // Same frame with the fresh anchors fed back (steady state).
                let steady =
                    calculate_placements_anchored(area, &margins, &data, true, &fresh.anchors);
                assert_placements_sane(&format!("{label} (steady)"), area, &steady.visible);

                // Anchors recorded against a bigger frame must not let a
                // widget escape a now-smaller area.
                let shrunk =
                    calculate_placements_anchored(area, &margins, &data, true, &healthy.anchors);
                assert_placements_sane(&format!("{label} (shrunk)"), area, &shrunk.visible);
            }
        }
    }
}

/// (b) Contention between the standalone todos and background widgets.
///
/// When both are eligible, Overview (higher priority) merges them, so the
/// standalone widgets only compete where Overview cannot fit:
/// - a single short pocket goes to the highest-priority standalone contender
///   (todos) and background is silently dropped, without overlap;
/// - two short pockets let both place disjointly.
#[test]
fn todo_and_background_contention_prioritizes_todos_without_overlap() {
    let data = InfoWidgetData {
        todos: vec![todo("t1", "in_progress"), todo("t2", "pending")],
        background_info: Some(BackgroundInfo {
            running_count: 2,
            running_tasks: vec!["bash".to_string(), "task".to_string()],
            ..Default::default()
        }),
        ..Default::default()
    };

    // One pocket of 6 rows: Overview (min 8 + borders) can't fit; todos
    // (height 5) wins the pocket and background is dropped.
    let area = Rect::new(0, 0, 80, 6);
    let outcome = calculate_placements_anchored(area, &margins_for(40, 6, false), &data, true, &[]);
    assert_placements_sane("short pocket", area, &outcome.visible);
    let kinds: Vec<WidgetKind> = outcome.visible.iter().map(|p| p.kind).collect();
    assert_eq!(
        kinds,
        vec![WidgetKind::Todos],
        "highest-priority standalone contender should win the only pocket"
    );

    // Two 7-row pockets separated by a full-width row: Overview still can't
    // fit (needs 10 rows), so todos and background each take a pocket without
    // overlapping.
    let mut widths = vec![40u16; 15];
    widths[7] = 0;
    let area = Rect::new(0, 0, 80, 15);
    let margins = Margins {
        right_widths: widths,
        left_widths: Vec::new(),
        centered: false,
        scroll_top: 10,
        ..Default::default()
    };
    let outcome = calculate_placements_anchored(area, &margins, &data, true, &[]);
    assert_placements_sane("two pockets", area, &outcome.visible);
    let kinds: Vec<WidgetKind> = outcome.visible.iter().map(|p| p.kind).collect();
    assert!(
        kinds.contains(&WidgetKind::Todos) && kinds.contains(&WidgetKind::BackgroundTasks),
        "both standalone widgets should place across two pockets, got {kinds:?}"
    );
}

/// (b) When Overview is shown, its mergeable widgets (todos, background,
/// swarm, ...) must not also place standalone.
#[test]
fn overview_suppresses_mergeable_widgets_under_contention() {
    let data = contended_data();
    let area = Rect::new(0, 0, 100, 40);
    let outcome =
        calculate_placements_anchored(area, &margins_for(40, 40, false), &data, true, &[]);
    assert_placements_sane("overview contention", area, &outcome.visible);
    let kinds: Vec<WidgetKind> = outcome.visible.iter().map(|p| p.kind).collect();
    assert!(
        kinds.contains(&WidgetKind::Overview),
        "overview should place with ample space, got {kinds:?}"
    );
    for kind in &kinds {
        if *kind != WidgetKind::Overview {
            assert!(
                !is_overview_mergeable(*kind),
                "mergeable widget {kind:?} placed alongside overview: {kinds:?}"
            );
        }
    }
}

/// (c) The swarm dock widget is gated on managed members: a session that
/// manages agents (spawn subtree) gets the dock in layout, while plain swarm
/// presence data (session counts/names) must never place it. The inline strip
/// stands down while the dock is visible so agents never render twice.
#[test]
fn swarm_status_dock_requires_managed_members() {
    // With managed members (contended_data has them) the dock is eligible and
    // can be placed. The inline strip avoids double-rendering by standing
    // down while the dock is visible (see ui.rs swarm_strip_lines gating).
    let data = contended_data();
    assert!(
        data.has_data_for(WidgetKind::SwarmStatus),
        "SwarmStatus dock must be eligible while this session manages agents"
    );
    assert!(data.available_widgets().contains(&WidgetKind::SwarmStatus));

    // Without managed members the widget stays out of layout entirely, no
    // matter how rich the rest of swarm_info is (legacy session-list data).
    let mut without = contended_data();
    without
        .swarm_info
        .as_mut()
        .expect("swarm info")
        .managed_members
        .clear();
    assert!(
        !without.has_data_for(WidgetKind::SwarmStatus),
        "SwarmStatus must stay hidden without managed agents"
    );
    assert!(
        !without
            .available_widgets()
            .contains(&WidgetKind::SwarmStatus)
    );

    let area = Rect::new(0, 0, 100, 40);
    let outcome =
        calculate_placements_anchored(area, &margins_for(40, 40, true), &without, true, &[]);
    assert!(
        outcome
            .visible
            .iter()
            .all(|p| p.kind != WidgetKind::SwarmStatus),
        "SwarmStatus should never be placed without managed agents"
    );
}

/// A margin profile longer than the messages area (caller bug or stale data)
/// must not let widgets place below the viewport, where they would draw over
/// the input and status rows.
#[test]
fn margin_profile_longer_than_area_cannot_place_below_viewport() {
    let data = contended_data();
    let area = Rect::new(0, 0, 100, 8);
    let margins = margins_for(40, 50, false);

    let fresh = calculate_placements_anchored(area, &margins, &data, true, &[]);
    assert_placements_sane("long profile (fresh)", area, &fresh.visible);

    // Anchors recorded against a 40-row frame, replayed into the 8-row frame
    // with the oversized profile: Phase 1 must not keep a placement whose rows
    // exist in the profile but not in the area.
    let tall_area = Rect::new(0, 0, 100, 40);
    let tall =
        calculate_placements_anchored(tall_area, &margins_for(40, 40, false), &data, true, &[]);
    let shrunk = calculate_placements_anchored(area, &margins, &data, true, &tall.anchors);
    assert_placements_sane("long profile (stale anchors)", area, &shrunk.visible);
}

/// When the messages area shifts down between frames (e.g. a banner appears
/// above it), anchors recorded against the old, higher area must be dropped
/// and re-homed inside the new bounds instead of rendering over the banner.
#[test]
fn stale_anchor_above_shifted_area_is_rehomed_not_drawn_out_of_bounds() {
    let data = contended_data();

    let area0 = Rect::new(0, 0, 80, 20);
    let first = calculate_placements_anchored(area0, &margins_for(40, 20, false), &data, true, &[]);
    assert!(!first.visible.is_empty());
    assert!(
        first.visible.iter().any(|p| p.rect.y < 5),
        "expected at least one widget anchored in the top rows"
    );

    // The area shifts down by 5 rows.
    let area1 = Rect::new(0, 5, 80, 15);
    let second = calculate_placements_anchored(
        area1,
        &margins_for(40, 15, false),
        &data,
        true,
        &first.anchors,
    );
    assert_placements_sane("shifted area", area1, &second.visible);
}

/// Model + context only: the typical session data behind the right-hand box.
fn model_and_context_data() -> InfoWidgetData {
    InfoWidgetData {
        model: Some("GLM-5.3".to_string()),
        provider_name: Some("e2e-mock".to_string()),
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

/// Split parts anchored while space was scarce must merge back into the
/// combined box once a pocket that fits it appears, instead of riding the
/// transcript as separate boxes until they scroll off.
#[test]
fn split_widgets_merge_back_into_overview_when_space_returns() {
    let data = model_and_context_data();
    let area = Rect::new(0, 0, 140, 40);
    // Anchors as a previous frame left them: model and context placed as two
    // separate right-side boxes, still on screen.
    let split = |kind: WidgetKind, y: u16, h: u16| WidgetAnchor {
        placement: WidgetPlacement {
            kind,
            rect: Rect::new(100, y, 40, h),
            side: Side::Right,
        },
        hidden_frames: 0,
        content_top: y as usize,
    };
    let anchors = vec![
        split(WidgetKind::ModelInfo, 2, 4),
        split(WidgetKind::ContextUsage, 20, 3),
    ];
    let roomy = Margins {
        right_widths: vec![60; 40],
        right_reliable: vec![60; 40],
        scroll_top: 0,
        ..Default::default()
    };
    let out = calculate_placements_anchored(area, &roomy, &data, true, &anchors);
    assert_placements_sane("merge back", area, &out.visible);
    assert_eq!(right_kinds(&out.visible), vec![WidgetKind::Overview]);
    assert!(
        out.anchors
            .iter()
            .all(|a| !is_overview_mergeable(a.placement.kind)),
        "split anchors must be retired once overview is placed: {:?}",
        out.anchors
            .iter()
            .map(|a| a.placement.kind)
            .collect::<Vec<_>>()
    );
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

/// Re-merging must not oscillate: in a ragged chat where the combined box only
/// fits some of the time, the right side must not flip between one merged box
/// and scattered parts frame after frame.
#[test]
fn remerge_does_not_flicker_between_merged_and_split() {
    let data = contended_data();
    let area = Rect::new(0, 0, 140, 40);
    let mut anchors: Vec<WidgetAnchor> = Vec::new();
    let mut flips = 0usize;
    let mut last_had_overview: Option<bool> = None;
    for frame in 0..300usize {
        // Alternate dense and sparse stretches of transcript.
        let period = if (frame / 40) % 2 == 0 { 5 } else { 12 };
        let out = calculate_placements_anchored(
            area,
            &ragged_margins(period, frame),
            &data,
            true,
            &anchors,
        );
        assert_placements_sane("remerge flicker", area, &out.visible);
        let kinds: Vec<WidgetKind> = out.visible.iter().map(|p| p.kind).collect();
        if kinds.contains(&WidgetKind::Overview) {
            for k in &kinds {
                assert!(
                    !is_overview_mergeable(*k),
                    "frame {frame}: {k:?} shown alongside overview {kinds:?}"
                );
            }
        }
        let has = kinds.contains(&WidgetKind::Overview);
        if last_had_overview.is_some_and(|l| l != has) {
            flips += 1;
        }
        last_had_overview = Some(has);
        anchors = out.anchors;
    }
    // Seven dense/sparse transitions over the run: allow a switch per
    // transition, not per frame.
    assert!(flips <= 14, "merged/split state flipped {flips} times");
}

/// A widget that is not part of Overview (memory) keeps its own anchor when
/// the split parts are merged back.
#[test]
fn remerge_keeps_non_mergeable_anchors() {
    let data = contended_data();
    let area = Rect::new(0, 0, 140, 40);
    let anchor = |kind: WidgetKind, y: u16, h: u16| WidgetAnchor {
        placement: WidgetPlacement {
            kind,
            rect: Rect::new(100, y, 40, h),
            side: Side::Right,
        },
        hidden_frames: 0,
        content_top: y as usize,
    };
    let anchors = vec![
        anchor(WidgetKind::ContextUsage, 1, 4),
        anchor(WidgetKind::MemoryActivity, 30, 6),
    ];
    let roomy = Margins {
        right_widths: vec![60; 40],
        right_reliable: vec![60; 40],
        scroll_top: 0,
        ..Default::default()
    };
    let out = calculate_placements_anchored(area, &roomy, &data, true, &anchors);
    assert_placements_sane("keep memory", area, &out.visible);
    let kinds: Vec<WidgetKind> = out.visible.iter().map(|p| p.kind).collect();
    assert!(kinds.contains(&WidgetKind::Overview), "{kinds:?}");
    let memory = out
        .visible
        .iter()
        .find(|p| p.kind == WidgetKind::MemoryActivity)
        .expect("memory widget kept");
    assert_eq!(memory.rect.y, 30, "memory must hold its anchored slot");
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
/// must not stay in that slot once todos make it taller - holding it would
/// suppress the parts and render nothing (the content no longer fits).
#[test]
fn overview_that_outgrows_its_slot_rehomes_instead_of_going_blank() {
    let mut data = model_and_context_data();
    let small = calculate_widget_height(WidgetKind::Overview, &data, 40, 40);
    let anchors = vec![right_anchor(WidgetKind::Overview, 3, small)];
    data.todos = vec![
        todo("a", "in_progress"),
        todo("b", "pending"),
        todo("c", "pending"),
    ];
    let grown = calculate_widget_height(WidgetKind::Overview, &data, 40, 40);
    assert!(
        grown > small,
        "precondition: todos grow overview ({small} -> {grown})"
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

/// Review #1456 (2): the re-merge fit test must use space left after other
/// anchored widgets reserve their rows. If memory holds the only pocket tall
/// enough for Overview, the split context box must stay visible.
#[test]
fn remerge_keeps_split_part_when_other_anchor_holds_the_only_pocket() {
    let data = contended_data();
    let area = Rect::new(0, 0, 140, 40);
    let overview_need = super::phase2_min_height(WidgetKind::Overview, &data);
    // One tall pocket (rows 0..=17), then dense lines, then a 4-row pocket
    // (rows 30..=33) that only fits the small context box.
    let free: Vec<u16> = (0..40)
        .map(|r| {
            if r <= 17 || (30..=33).contains(&r) {
                60
            } else {
                4
            }
        })
        .collect();
    assert!(
        18 >= overview_need,
        "precondition: tall pocket fits overview"
    );
    let margins = Margins {
        right_widths: free.clone(),
        right_reliable: free,
        scroll_top: 0,
        ..Default::default()
    };
    // Memory occupies the tall pocket; context sits split in the small one.
    let anchors = vec![
        right_anchor(WidgetKind::MemoryActivity, 0, 18),
        right_anchor(WidgetKind::ContextUsage, 30, 4),
    ];
    let out = calculate_placements_anchored(area, &margins, &data, true, &anchors);
    assert_placements_sane("reserved pocket", area, &out.visible);
    let kinds: Vec<WidgetKind> = out.visible.iter().map(|p| p.kind).collect();
    assert!(
        kinds.contains(&WidgetKind::ContextUsage) || kinds.contains(&WidgetKind::Overview),
        "context information vanished: {kinds:?}"
    );
}

/// Review #1456 (3): swarm and compaction are Overview-suppressed but Overview
/// does not render them, so re-merging must keep their boxes.
#[test]
fn remerge_keeps_swarm_and_compaction_boxes() {
    let data = contended_data();
    let area = Rect::new(0, 0, 140, 40);
    for kind in [WidgetKind::SwarmStatus, WidgetKind::Compaction] {
        assert!(data.has_data_for(kind), "precondition: {kind:?} has data");
        let anchors = vec![
            right_anchor(WidgetKind::ContextUsage, 1, 4),
            right_anchor(kind, 30, 6),
        ];
        let out = calculate_placements_anchored(area, &roomy_margins(), &data, true, &anchors);
        assert_placements_sane("keep swarm/compaction", area, &out.visible);
        let kinds: Vec<WidgetKind> = out.visible.iter().map(|p| p.kind).collect();
        assert!(kinds.contains(&WidgetKind::Overview), "{kinds:?}");
        assert!(
            kinds.contains(&kind),
            "{kind:?} box dropped by re-merge: {kinds:?}"
        );
    }
}

/// Review #1456 (4): re-merge must not retire a split part and then lose the
/// only Overview-sized pocket to a higher-priority widget (Diagrams). The
/// retirement is only committed if Overview is actually placed.
#[test]
fn remerge_keeps_split_part_when_higher_priority_widget_takes_the_pocket() {
    let mut data = model_and_context_data();
    data.diagrams = vec![crate::tui::info_widget::DiagramInfo {
        hash: 70,
        width: 800,
        height: 400,
        label: None,
    }];
    assert!(data.available_widgets().first() == Some(&WidgetKind::Diagrams));
    let area = Rect::new(0, 0, 140, 40);
    // A single 12-row pocket (rows 0..=11) that Diagrams fills, dense lines,
    // then a 4-row pocket (rows 17..=20) holding the split context box.
    let free: Vec<u16> = (0..40)
        .map(|r| {
            if r <= 11 || (17..=20).contains(&r) {
                60
            } else {
                4
            }
        })
        .collect();
    let margins = Margins {
        right_widths: free.clone(),
        right_reliable: free,
        scroll_top: 0,
        ..Default::default()
    };
    let anchors = vec![right_anchor(WidgetKind::ContextUsage, 17, 4)];
    let out = calculate_placements_anchored(area, &margins, &data, true, &anchors);
    assert_placements_sane("diagram takes pocket", area, &out.visible);
    let kinds: Vec<WidgetKind> = out.visible.iter().map(|p| p.kind).collect();
    assert!(
        kinds.contains(&WidgetKind::ContextUsage) || kinds.contains(&WidgetKind::Overview),
        "context information vanished: {kinds:?}"
    );
}
