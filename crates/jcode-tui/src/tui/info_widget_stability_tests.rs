use super::*;
use crate::tui::info_widget::InfoWidgetData;

/// Build widget data that yields a stable overview widget (model + queue line etc).
fn sample_data() -> InfoWidgetData {
    InfoWidgetData {
        model: Some("gpt-test".to_string()),
        queue_mode: Some(true),
        ..Default::default()
    }
}

/// Widget data with several independent widgets that compete for margin space, so
/// multiple widgets can be on screen at once (needed to exercise hide-in-place /
/// overlap behaviour and the information-coverage A/B).
fn rich_data() -> InfoWidgetData {
    use crate::tui::info_widget::{BackgroundInfo, UsageInfo, UsageProvider};
    InfoWidgetData {
        model: Some("gpt-test".to_string()),
        queue_mode: Some(true),
        todos: vec![
            crate::todo::TodoItem {
                group: None,
                content: "first task".to_string(),
                status: "in_progress".to_string(),
                priority: "high".to_string(),
                id: "t1".to_string(),
                blocked_by: Vec::new(),
                assigned_to: None,
                confidence: None,
                completion_confidence: None,
                confidence_history: Vec::new(),
            },
            crate::todo::TodoItem {
                group: None,
                content: "second task".to_string(),
                status: "pending".to_string(),
                priority: "medium".to_string(),
                id: "t2".to_string(),
                blocked_by: Vec::new(),
                assigned_to: None,
                confidence: None,
                completion_confidence: None,
                confidence_history: Vec::new(),
            },
        ],
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
        ..Default::default()
    }
}

#[test]
fn flat_content_is_perfectly_stable() {
    // Uniform narrow content: the negative-space shape never changes while scrolling.
    // Residents ride the transcript, so their absolute `y` tracks the scroll, but
    // relative to the content they must not drift at all, and they must not flicker.
    let content: Vec<u16> = vec![20; 200];
    let report = measure_scroll(&content, 100, 20, &sample_data());
    assert!(report.frames > 10, "expected many scroll frames");
    assert_eq!(
        report.total_content_travel, 0,
        "uniform content should produce zero content-relative travel, got {:#?}",
        report
    );
    assert_eq!(
        report.total_flicker, 0,
        "uniform content should not flicker"
    );
    assert_eq!(report.total_size_churn, 0, "widths must stay frozen");
}

#[test]
fn ragged_content_makes_widgets_move() {
    // Chat-like content: mostly narrow lines with a long line every few rows. This
    // leaves fitting regions in the right margin, but their top/height shift under
    // the fixed screen rows as you scroll, which is exactly the reported distraction.
    let content: Vec<u16> = (0..200).map(|i| if i % 7 == 0 { 95 } else { 28 }).collect();
    let report = measure_scroll(&content, 100, 24, &sample_data());
    assert!(
        report.widgets.iter().any(|w| w.frames_present > 0),
        "expected at least one widget to be placed, got {:#?}",
        report
    );
    assert!(
        report.distraction_per_100_lines > 0.0,
        "ragged content should register movement, got {:#?}",
        report
    );
}

#[test]
fn analyze_frames_counts_travel_and_flicker() {
    // Hand-built frames to lock the metric math independent of the layout engine.
    let a = PlacedRect {
        kind: "overview",
        x: 60,
        y: 4,
        width: 30,
        height: 8,
    };
    let moved = PlacedRect {
        kind: "overview",
        y: 7,
        ..a
    };
    let frames = vec![vec![a], vec![moved], vec![]];
    let report = analyze_frames(&frames);
    assert_eq!(report.steps, 2);
    let w = &report.widgets[0];
    assert_eq!(w.y_travel, 3, "expected |7-4| vertical travel");
    assert_eq!(w.move_events, 1);
    assert_eq!(w.disappearances, 1);
    assert_eq!(report.total_flicker, 1);
    // unstable in both steps (move then disappear)
    assert!((report.unstable_step_fraction - 1.0).abs() < f64::EPSILON);
}

#[test]
fn empty_input_is_safe() {
    let report = analyze_frames(&[]);
    assert_eq!(report.frames, 0);
    assert_eq!(report.steps, 0);
    assert!(report.widgets.is_empty());
}

/// Regression guard for the resident model: when content has occasional long
/// lines (the common chat/markdown shape), widgets must ride the transcript with
/// *zero content-relative travel* - they belong to their pocket of negative space
/// and never drift against the surrounding text or resize while placed.
#[test]
fn occasional_long_lines_do_not_move_widgets() {
    // Periods chosen so the gaps between long lines are tall enough to actually
    // hold a widget (very dense periods leave no placeable region at all).
    for period in [7usize, 9, 11, 13] {
        let content: Vec<u16> = (0..240)
            .map(|i| if i % period == 0 { 95 } else { 28 })
            .collect();
        let report = measure_scroll(&content, 100, 24, &sample_data());
        assert!(
            report.widgets.iter().any(|w| w.frames_present > 0),
            "period {period}: expected a widget to be placed: {report:#?}"
        );
        assert_eq!(
            report.total_content_travel, 0,
            "period {period}: widgets should not drift against the transcript, got {} \
             content travel: {:#?}",
            report.total_content_travel, report
        );
        assert_eq!(
            report.total_size_churn, 0,
            "period {period}: resident widgets must not resize in place: {report:#?}"
        );
    }
}

/// Demonstration / quantification harness. Run with:
///   cargo test -p jcode-tui info_widget_stability::tests::demo_quantify -- --ignored --nocapture
#[test]
#[ignore]
fn demo_quantify() {
    fn profile(name: &str, content: &[u16]) {
        let report = measure_scroll(content, 100, 24, &sample_data());
        println!(
            "{:<22} steps={:<4} travel/100={:>7.1} flicker/100={:>6.1} distraction/100={:>7.1} unstable={:>5.1}% worst={}",
            name,
            report.steps,
            report.travel_per_100_lines,
            report.flicker_per_100_lines,
            report.distraction_per_100_lines,
            report.unstable_step_fraction * 100.0,
            report.worst_widget.as_deref().unwrap_or("-"),
        );
    }

    println!("\n=== info-widget scroll-stability quantification (100x24 viewport) ===");
    profile("flat narrow", &vec![20; 300]);
    profile(
        "long line every 7",
        &(0..300)
            .map(|i| if i % 7 == 0 { 95 } else { 28 })
            .collect::<Vec<_>>(),
    );
    profile(
        "long line every 3",
        &(0..300)
            .map(|i| if i % 3 == 0 { 90 } else { 30 })
            .collect::<Vec<_>>(),
    );
    profile(
        "code-like (ragged)",
        &(0..300)
            .map(|i| 20 + ((i * 37) % 70) as u16)
            .collect::<Vec<_>>(),
    );
}

/// Widgets must never overlap each other while scrolling, even with the
/// hide-in-place anchoring (a hidden widget's slot must stay reserved so a
/// different widget can't be dropped into it and then collide when it returns).
#[test]
fn widgets_never_overlap_while_scrolling() {
    for period in [7usize, 9, 11, 14, 17] {
        let content: Vec<u16> = (0..240)
            .map(|i| if i % period == 0 { 95 } else { 26 })
            .collect();
        let report = measure_scroll_mode(&content, 100, 24, &rich_data(), SimMode::Anchored);
        assert_eq!(
            report.overlap_frames, 0,
            "period {period}: widgets overlapped in {} frames (max {} pairs)",
            report.overlap_frames, report.max_overlap_pairs
        );
    }
}

/// Content anchoring is the "stick to one negative-space spot while scrolling"
/// behaviour: a widget pins to a transcript line and rides the scroll, so its motion
/// *relative to the surrounding text* is ~0 even though its absolute screen `y`
/// tracks the scroll. This must dramatically reduce content-relative travel versus
/// the screen-anchored mode on ragged content (where the old behaviour churned).
#[test]
fn content_anchoring_reduces_content_relative_travel() {
    for period in [7usize, 9, 11, 13] {
        let content: Vec<u16> = (0..240)
            .map(|i| if i % period == 0 { 95 } else { 28 })
            .collect();
        let screen = measure_scroll_mode(&content, 100, 24, &sample_data(), SimMode::Anchored);
        let stuck =
            measure_scroll_mode(&content, 100, 24, &sample_data(), SimMode::ContentAnchored);
        assert!(
            stuck.widgets.iter().any(|w| w.frames_present > 0),
            "period {period}: expected a widget to be placed"
        );
        assert!(
            stuck.content_travel_per_100_lines <= screen.content_travel_per_100_lines + 1e-9,
            "period {period}: content anchoring should not increase content-relative travel \
             (content={:.1} vs screen={:.1})",
            stuck.content_travel_per_100_lines,
            screen.content_travel_per_100_lines,
        );
        // A perfectly stuck widget rides the scroll with zero residual travel.
        assert_eq!(
            stuck.total_content_travel, 0,
            "period {period}: content-anchored widget should not drift relative to the transcript, \
             got {} ({:#?})",
            stuck.total_content_travel, stuck
        );
    }
}

/// Content-anchored widgets must still never overlap while riding the scroll.
#[test]
fn content_anchored_widgets_never_overlap_while_scrolling() {
    for period in [7usize, 9, 11, 14, 17] {
        let content: Vec<u16> = (0..240)
            .map(|i| if i % period == 0 { 95 } else { 26 })
            .collect();
        let report = measure_scroll_mode(&content, 100, 24, &rich_data(), SimMode::ContentAnchored);
        assert_eq!(
            report.overlap_frames, 0,
            "period {period}: widgets overlapped in {} frames (max {} pairs)",
            report.overlap_frames, report.max_overlap_pairs
        );
    }
}

/// A/B: stick-to-the-transcript anchoring vs holding a fixed screen row. Run with:
///   cargo test -p jcode-tui info_widget_stability::tests::demo_content_anchor -- --ignored --nocapture
#[test]
#[ignore]
fn demo_content_anchor() {
    fn row(name: &str, content: &[u16]) {
        let s = measure_scroll_mode(content, 100, 24, &rich_data(), SimMode::Anchored);
        let c = measure_scroll_mode(content, 100, 24, &rich_data(), SimMode::ContentAnchored);
        println!(
            "{:<20} | screen-anchored: travel/100={:>6.1} content-travel/100={:>6.1} flicker/100={:>5.1} keepVis={:>3.0}% \
             | content-anchored: travel/100={:>6.1} content-travel/100={:>6.1} flicker/100={:>5.1} keepVis={:>3.0}%",
            name,
            s.travel_per_100_lines,
            s.content_travel_per_100_lines,
            s.flicker_per_100_lines,
            s.mean_kind_visibility * 100.0,
            c.travel_per_100_lines,
            c.content_travel_per_100_lines,
            c.flicker_per_100_lines,
            c.mean_kind_visibility * 100.0,
        );
    }

    println!(
        "\n=== content-anchor A/B (100x24, rich widget set) ===\n\
         content-travel = vertical motion relative to the transcript (scroll-ride removed); lower = sticks to its spot\n"
    );
    row("flat narrow", &vec![20; 300]);
    row(
        "long line every 7",
        &(0..300)
            .map(|i| if i % 7 == 0 { 95 } else { 28 })
            .collect::<Vec<_>>(),
    );
    row(
        "long line every 14",
        &(0..300)
            .map(|i| if i % 14 == 0 { 95 } else { 28 })
            .collect::<Vec<_>>(),
    );
    row(
        "code-like (ragged)",
        &(0..300)
            .map(|i| 20 + ((i * 37) % 70) as u16)
            .collect::<Vec<_>>(),
    );
}

/// A/B: does stable (anchored) placement cost information vs greedy max-info?
/// Run with:
///   cargo test -p jcode-tui info_widget_stability::tests::demo_info_tradeoff -- --ignored --nocapture
#[test]
#[ignore]
fn demo_info_tradeoff() {
    fn row(name: &str, content: &[u16]) {
        let g = measure_scroll_mode(content, 100, 24, &rich_data(), SimMode::Greedy);
        let a = measure_scroll_mode(content, 100, 24, &rich_data(), SimMode::Anchored);
        println!(
            "{:<20} | greedy: vis={:.2} cells={:>6.0} kinds={} keepVis={:>3.0}% travel/100={:>6.1} overlap={} \
             | anchored: vis={:.2} cells={:>6.0} kinds={} keepVis={:>3.0}% travel/100={:>6.1} overlap={}",
            name,
            g.avg_widgets_visible,
            g.avg_visible_cells,
            g.distinct_kinds_seen,
            g.mean_kind_visibility * 100.0,
            g.travel_per_100_lines,
            g.overlap_frames,
            a.avg_widgets_visible,
            a.avg_visible_cells,
            a.distinct_kinds_seen,
            a.mean_kind_visibility * 100.0,
            a.travel_per_100_lines,
            a.overlap_frames,
        );
    }

    println!(
        "\n=== information vs stability A/B (100x24, rich widget set) ===\n\
         vis=avg widgets/frame cells=avg area/frame kinds=distinct seen keepVis=mean kind visible% travel=churn\n"
    );
    row("flat narrow", &vec![20; 300]);
    row(
        "long line every 7",
        &(0..300)
            .map(|i| if i % 7 == 0 { 95 } else { 28 })
            .collect::<Vec<_>>(),
    );
    row(
        "long line every 14",
        &(0..300)
            .map(|i| if i % 14 == 0 { 95 } else { 28 })
            .collect::<Vec<_>>(),
    );
    row(
        "code-like (ragged)",
        &(0..300)
            .map(|i| 20 + ((i * 37) % 70) as u16)
            .collect::<Vec<_>>(),
    );
}

/// Tune the look-ahead window `W`: for each profile, compare anchored (W=0) with
/// LookAhead(W) across a sweep, reporting flicker, coverage, and per-kind
/// visibility so we can pick the smallest W that kills the blink without
/// sacrificing too much coverage. Run with:
///   cargo test -p jcode-tui info_widget_stability::tests::demo_lookahead_sweep -- --ignored --nocapture
#[test]
#[ignore]
fn demo_lookahead_sweep() {
    fn line(label: &str, r: &super::StabilityReport) {
        println!(
            "  {:<14} vis={:.2} cells={:>5.0} kinds={} keepVis={:>3.0}% flicker/100={:>5.1} travel/100={:>5.1} overlap={}",
            label,
            r.avg_widgets_visible,
            r.avg_visible_cells,
            r.distinct_kinds_seen,
            r.mean_kind_visibility * 100.0,
            r.flicker_per_100_lines,
            r.travel_per_100_lines,
            r.overlap_frames,
        );
    }

    let profiles: Vec<(&str, Vec<u16>)> = vec![
        ("flat narrow", vec![20; 300]),
        (
            "long line /7",
            (0..300).map(|i| if i % 7 == 0 { 95 } else { 28 }).collect(),
        ),
        (
            "long line /14",
            (0..300)
                .map(|i| if i % 14 == 0 { 95 } else { 28 })
                .collect(),
        ),
        (
            "code-like",
            (0..300).map(|i| 20 + ((i * 37) % 70) as u16).collect(),
        ),
    ];

    println!("\n=== look-ahead window sweep (100x24, rich widget set) ===");
    for (name, content) in &profiles {
        println!("{name}:");
        line(
            "greedy",
            &measure_scroll_mode(content, 100, 24, &rich_data(), SimMode::Greedy),
        );
        line(
            "anchored(W=0)",
            &measure_scroll_mode(content, 100, 24, &rich_data(), SimMode::Anchored),
        );
        for w in [2u16, 4, 6, 8, 12] {
            line(
                &format!("lookahead({w})"),
                &measure_scroll_mode(content, 100, 24, &rich_data(), SimMode::LookAhead(w)),
            );
        }
        for w in [2u16, 4, 8] {
            line(
                &format!("la-fresh({w})"),
                &measure_scroll_mode(content, 100, 24, &rich_data(), SimMode::LookAheadFresh(w)),
            );
        }
    }
}
