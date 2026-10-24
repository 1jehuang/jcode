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
//!    widgets (background, overview, memory, ...) compete for the same
//!    margin pockets and lower-priority widgets are silently dropped.

use super::*;
use crate::tui::info_widget::{
    BackgroundInfo, CacheHitInfo, CompactionInfo, DirtyFile, GitInfo, InfoWidgetData, MemoryInfo,
    SwarmInfo, UsageInfo, UsageProvider,
};

/// Kitchen-sink data: all represented widgets compete for margin space.
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
            dirty_files: vec![DirtyFile::new('M', "a.rs"), DirtyFile::new('M', "b.rs")],
            dirty_total: 2,
            ..Default::default()
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

/// (b) When Overview is shown, its mergeable widgets (background,
/// swarm, commits, ...) must not also place standalone.
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

#[test]
fn high_usage_is_not_duplicated_when_overview_can_fit() {
    let mut data = contended_data();
    data.usage_info.as_mut().unwrap().seven_day = 0.94;
    let area = Rect::new(0, 0, 100, 60);
    let margins = margins_for(40, 60, false);
    assert!(
        data.effective_priority(WidgetKind::UsageLimits)
            < data.effective_priority(WidgetKind::Overview)
    );

    // Exercise both a fresh priority-promoted card and a standalone card
    // carried from a previous frame before Overview had a place.
    let usage_anchor = WidgetAnchor {
        placement: WidgetPlacement {
            kind: WidgetKind::UsageLimits,
            rect: Rect::new(60, 55, 40, 5),
            side: Side::Right,
        },
        hidden_frames: 0,
        content_top: margins.scroll_top + 55,
    };
    for anchors in [Vec::new(), vec![usage_anchor]] {
        let outcome = calculate_placements_anchored(area, &margins, &data, true, &anchors);
        assert_placements_sane("high usage", area, &outcome.visible);
        assert!(
            outcome
                .visible
                .iter()
                .any(|p| p.kind == WidgetKind::Overview)
        );
        assert!(
            !outcome
                .visible
                .iter()
                .any(|p| p.kind == WidgetKind::UsageLimits)
        );
        assert!(
            !outcome
                .anchors
                .iter()
                .any(|a| a.placement.kind == WidgetKind::UsageLimits)
        );
        let steady = calculate_placements_anchored(area, &margins, &data, true, &outcome.anchors);
        assert!(
            steady
                .visible
                .iter()
                .any(|p| p.kind == WidgetKind::Overview)
        );
        assert!(
            !steady
                .visible
                .iter()
                .any(|p| p.kind == WidgetKind::UsageLimits)
        );
    }
}

#[test]
fn high_usage_stays_visible_when_overview_cannot_fit() {
    let mut data = contended_data();
    data.usage_info.as_mut().unwrap().seven_day = 0.94;
    let area = Rect::new(0, 0, 100, 7);
    let outcome = calculate_placements_anchored(area, &margins_for(40, 7, false), &data, true, &[]);
    assert!(
        !outcome
            .visible
            .iter()
            .any(|p| p.kind == WidgetKind::Overview)
    );
    assert!(
        outcome
            .visible
            .iter()
            .any(|p| p.kind == WidgetKind::UsageLimits)
    );
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
    // Shift past an actual resident, independent of how many non-plan
    // widgets currently fit in the layout.
    let shifted_y = first.visible.iter().map(|p| p.rect.y).min().unwrap() + 1;
    assert!(shifted_y < area0.height);
    let area1 = Rect::new(0, shifted_y, 80, area0.height - shifted_y);
    let second = calculate_placements_anchored(
        area1,
        &margins_for(40, area1.height as usize, false),
        &data,
        true,
        &first.anchors,
    );
    assert_placements_sane("shifted area", area1, &second.visible);
}

/// Content that grows under a resident widget must not make it vanish.
///
/// Startup repro: the Overview is placed for {usage, changes}, then History
/// adds the session row, so its body needs one more line than the slot it is
/// anchored to. Phase 1 used to keep the stale-height slot, the body no longer
/// fit, and the Overview rendered nothing for the rest of the session while
/// still holding the slot (so Phase 2 never re-homed it either).
#[test]
fn resident_overview_grows_in_place_when_its_content_grows() {
    let area = Rect::new(0, 0, 80, 30);
    let margins = margins_for(40, 30, false);
    let usage = UsageInfo {
        provider: UsageProvider::Anthropic,
        primary_limit_label: Some("5-hour".to_string()),
        secondary_limit_label: Some("Weekly".to_string()),
        available: true,
        ..Default::default()
    };
    let git = GitInfo {
        branch: "master".to_string(),
        modified: 1,
        dirty_files: vec![DirtyFile::new('M', "src/main.rs")],
        dirty_total: 1,
        ..Default::default()
    };
    let before = InfoWidgetData {
        usage_info: Some(usage),
        git_info: Some(git),
        ..Default::default()
    };
    let first = calculate_placements_anchored(area, &margins, &before, true, &[]);
    let overview = first
        .visible
        .iter()
        .find(|p| p.kind == WidgetKind::Overview)
        .expect("overview placed for usage + changes")
        .rect;

    let after = InfoWidgetData {
        session_name: Some("hare".to_string()),
        ..before.clone()
    };
    let second = calculate_placements_anchored(area, &margins, &after, true, &first.anchors);
    let grown = second
        .visible
        .iter()
        .find(|p| p.kind == WidgetKind::Overview)
        .expect("overview must stay placed after its content grows")
        .rect;
    let needed = crate::tui::info_widget::calculate_widget_height(
        WidgetKind::Overview,
        &after,
        grown.width,
        30,
    );
    assert!(
        grown.height >= needed,
        "slot must grow to fit the new body: {grown:?} needs {needed} (was {overview:?})"
    );
    assert_eq!(grown.x, overview.x, "resident must not jump sideways");
    assert_placements_sane("grown overview", area, &second.visible);
}

/// A resident whose slot is permanently covered by settled content re-homes
/// right away instead of staying hidden.
///
/// Startup repro: the `mcp:` header row arrives with History and lands under
/// the bottom of the Overview's slot. An idle client draws only a handful of
/// frames, so the frame-counted hide-in-place backstop never expired and the
/// Overview stayed invisible for the whole session.
#[test]
fn covered_resident_rehomes_immediately_when_content_is_settled() {
    let area = Rect::new(0, 0, 80, 30);
    let data = InfoWidgetData {
        background_info: Some(BackgroundInfo {
            running_count: 2,
            running_tasks: vec!["bash".into(), "task".into()],
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut margins = margins_for(40, 30, false);
    let first = calculate_placements_anchored(area, &margins, &data, true, &[]);
    let anchored = first.visible.first().expect("a widget placed").clone();

    // A settled wide row now covers the anchored slot's first row, with plenty
    // of free space elsewhere in the margin.
    margins.right_widths[anchored.rect.y as usize] = 10;

    let settled = calculate_placements_anchored(area, &margins, &data, true, &first.anchors);
    let rehomed = settled
        .visible
        .iter()
        .find(|p| p.kind == anchored.kind)
        .unwrap_or_else(|| {
            panic!(
                "settled cover must re-home {:?}, not hide it",
                anchored.kind
            )
        });
    let covered = anchored.rect.y;
    assert!(
        !(rehomed.rect.y..rehomed.rect.y + rehomed.rect.height).contains(&covered),
        "re-homed slot must clear the covered row: {rehomed:?}"
    );
    assert_placements_sane("re-homed", area, &settled.visible);

    // While content is churning (streaming / scrolling), it waits in place.
    margins.content_churning = true;
    let churning = calculate_placements_anchored(area, &margins, &data, true, &first.anchors);
    assert!(
        !churning.visible.iter().any(|p| p.kind == anchored.kind),
        "churning cover hides in place"
    );
    assert!(
        churning
            .anchors
            .iter()
            .any(|a| a.placement.kind == anchored.kind),
        "and keeps its anchor to return to"
    );
}

/// `display.single_overview = false` restores the classic layout: the
/// consolidated Overview never places and each mergeable section
/// (background, ...) docks as its own widget. The default (true) keeps the
/// single consolidated panel.
#[test]
fn single_overview_false_places_separate_widgets_instead_of_overview() {
    let area = Rect::new(0, 0, 80, 30);
    let data = InfoWidgetData {
        background_info: Some(BackgroundInfo {
            running_count: 1,
            running_tasks: vec!["bash".to_string()],
            ..Default::default()
        }),
        ..Default::default()
    };
    let margins = margins_for(40, 30, false);

    // Default: the Overview consolidates the background section.
    let consolidated = calculate_placements_anchored(area, &margins, &data, true, &[]);
    assert_placements_sane("consolidated", area, &consolidated.visible);
    assert!(
        consolidated
            .visible
            .iter()
            .any(|p| p.kind == WidgetKind::Overview),
        "default layout should place the consolidated Overview"
    );
    assert!(
        !consolidated
            .visible
            .iter()
            .any(|p| p.kind == WidgetKind::BackgroundTasks),
        "with the Overview placed, the standalone Background widget stays suppressed"
    );

    // Opt out: classic separate widgets, no Overview anywhere.
    let separate_data = InfoWidgetData {
        overview_disabled: true,
        ..data.clone()
    };
    let separate = calculate_placements_anchored(area, &margins, &separate_data, true, &[]);
    assert_placements_sane("separate", area, &separate.visible);
    assert!(
        !separate
            .visible
            .iter()
            .any(|p| p.kind == WidgetKind::Overview),
        "single_overview=false must not place the consolidated Overview"
    );
    assert!(
        separate
            .visible
            .iter()
            .any(|p| p.kind == WidgetKind::BackgroundTasks),
        "single_overview=false must let the standalone Background widget place"
    );

    // An anchor recorded under the consolidated layout must not resurrect the
    // Overview after the flip: the stale anchor dies because the Overview is
    // no longer in `available`.
    let flip =
        calculate_placements_anchored(area, &margins, &separate_data, true, &consolidated.anchors);
    assert_placements_sane("flip", area, &flip.visible);
    assert!(
        !flip.visible.iter().any(|p| p.kind == WidgetKind::Overview),
        "a stale Overview anchor must not survive the single_overview flip"
    );
}

/// Folded sections must not suppress their standalone widgets.
///
/// The Overview folds sections that do not fit its rect, but the placement
/// pass used to suppress every mergeable widget whenever an Overview was
/// placed, regardless of whether the rect actually showed that section. The
/// result: a short Overview slot (borderline height after its content grew,
/// or a covered resident that came back at a smaller height) folded the
/// details AND kept their standalone widgets suppressed - the content was
/// visible nowhere. `overview_shows_section` now consults the same fits
/// flags the renderer folds by, so a folded section's standalone widget
/// places again.
#[test]
fn overview_folded_sections_do_not_suppress_standalone_widgets() {
    // Runtime (1 row) + usage (label + primary + secondary = 3 rows) +
    // changes (5 dirty files, no overflow) = 9 inner rows of content.
    let data = InfoWidgetData {
        model: Some("claude-test-model".to_string()),
        provider_name: Some("anthropic".to_string()),
        usage_info: Some(UsageInfo {
            provider: UsageProvider::Anthropic,
            primary_limit_label: Some("5-hour".to_string()),
            secondary_limit_label: Some("Weekly".to_string()),
            available: true,
            ..Default::default()
        }),
        git_info: Some(GitInfo {
            branch: "master".to_string(),
            modified: 5,
            dirty_files: vec![
                DirtyFile::new('M', "src/a.rs"),
                DirtyFile::new('M', "src/b.rs"),
                DirtyFile::new('M', "src/c.rs"),
                DirtyFile::new('M', "src/d.rs"),
                DirtyFile::new('M', "src/e.rs"),
            ],
            dirty_total: 5,
            ..Default::default()
        }),
        ..Default::default()
    };

    // Both pockets are 10 rows (inner height 8): the Overview places on the
    // right, capped at the pocket, so its 9-row content folds the tail
    // (changes) away. Centered mode opens the left pocket for the standalone
    // widgets (left margins only exist when centered).
    let area = Rect::new(0, 0, 80, 10);
    let margins = margins_for(36, 10, true);
    let outcome = calculate_placements_anchored(area, &margins, &data, true, &[]);
    assert_placements_sane("folded overview", area, &outcome.visible);
    let overview = outcome
        .visible
        .iter()
        .find(|p| p.kind == WidgetKind::Overview)
        .expect("overview places with model + usage + changes");
    let vis = super::super::info_widget_overview::section_visibility(&data, overview.rect);
    assert!(
        vis.model_info && vis.usage,
        "front sections must still fit: {:?} vis={vis:?}",
        overview.rect
    );
    assert!(
        !vis.changes,
        "changes section must fold in a 10-row slot: {:?} vis={vis:?}",
        overview.rect
    );

    // Folded-out section: its standalone widget must place (left pocket)
    // instead of staying suppressed and hiding the dirty-file list.
    assert!(
        outcome
            .visible
            .iter()
            .any(|p| p.kind == WidgetKind::GitStatus),
        "folded-out changes section must keep its standalone GitStatus widget: {:?}",
        outcome.visible.iter().map(|p| p.kind).collect::<Vec<_>>()
    );
    // Sections the Overview does show stay suppressed exactly as before.
    assert!(
        !outcome
            .visible
            .iter()
            .any(|p| p.kind == WidgetKind::UsageLimits),
        "a section the Overview shows must keep its standalone widget suppressed"
    );
}
