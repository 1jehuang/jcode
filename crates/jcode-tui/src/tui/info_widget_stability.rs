//! Quantifying info-widget movement during scrolling.
//!
//! Info widgets are placed into the "negative space" (free width to the right/left
//! of the rendered text) of the *currently visible* viewport, anchored to absolute
//! screen rows. As the user scrolls, the per-row free-width profile changes line by
//! line, so a widget that fit at one screen position may reflow or jump elsewhere.
//! That reflow is the "distracting movement" users report.
//!
//! This module turns that distraction into deterministic numbers so it can be
//! measured and A/B-tested. It exposes:
//!
//! * [`analyze_frames`] - given the widget placements observed across a sequence of
//!   scroll frames, compute movement/flicker metrics. This is the shared analyzer
//!   used by both the synthetic simulation here and the live debug bench.
//! * [`simulate_scroll`] - drive the real layout algorithm over a synthetic content
//!   width profile, one scroll line at a time, producing the frame sequence.

use super::info_widget::{InfoWidgetData, WidgetPlacement};
use super::info_widget_layout::{Margins, WidgetAnchor, calculate_placements_anchored};
use ratatui::layout::Rect;
use serde::Serialize;
use std::collections::BTreeMap;

/// A single widget rectangle observed in one frame.
#[derive(Debug, Clone, Copy)]
pub struct PlacedRect {
    pub kind: &'static str,
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl PlacedRect {
    pub fn from_placement(p: &WidgetPlacement) -> Self {
        Self {
            kind: p.kind.as_str(),
            x: p.rect.x,
            y: p.rect.y,
            width: p.rect.width,
            height: p.rect.height,
        }
    }
}

/// Per-widget motion statistics accumulated across the scroll sequence.
#[derive(Debug, Clone, Default, Serialize)]
pub struct WidgetMotion {
    pub kind: String,
    /// Number of frames in which this widget was present.
    pub frames_present: usize,
    /// Transitions from absent -> present after the first frame (flicker in).
    pub appearances: usize,
    /// Transitions from present -> absent (flicker out).
    pub disappearances: usize,
    /// Frames where the widget moved (x or y changed) vs the previous present frame.
    pub move_events: usize,
    /// Frames where the widget changed width or height.
    pub resize_events: usize,
    /// Total absolute vertical travel (sum of |dy|) across consecutive present frames.
    pub y_travel: u32,
    /// Total absolute horizontal travel (sum of |dx|).
    pub x_travel: u32,
    /// Total absolute width change.
    pub width_churn: u32,
    /// Total absolute height change.
    pub height_churn: u32,
    /// Largest single-frame top-left jump (Chebyshev distance).
    pub max_jump: u16,
    /// Total *content-relative* vertical travel: |actual dy - expected scroll-ride|,
    /// counting only small frame-to-frame residuals (<= [`RIDE_TOLERANCE`]). A widget
    /// glued to its transcript line scores 0 here; a widget holding a fixed screen row
    /// while the text scrolls under it accrues ~1 per frame (it drifts against the
    /// text). This is the per-frame "jiggle" the user notices while scrolling.
    pub content_y_travel: u32,
    /// Number of steps where the widget moved much more than the scroll-ride (a
    /// "recycle": it left the viewport at one edge and a fresh instance entered at
    /// another). Visually this is one widget leaving with its content and another
    /// joining new content, not a slide, so it is tracked separately from jiggle.
    pub recycles: usize,
}

impl WidgetMotion {
    /// Composite "distraction" contribution from this widget: positional travel
    /// plus flicker (each appear/disappear weighted like a full-widget jump).
    fn distraction(&self) -> u32 {
        self.x_travel
            + self.y_travel
            + (self.appearances as u32 + self.disappearances as u32) * FLICKER_WEIGHT
    }
}

/// A flicker (appear/disappear) is at least this visually disruptive, in
/// equivalent cells of positional travel.
const FLICKER_WEIGHT: u32 = 8;

/// Maximum content-relative residual (rows) that still counts as "riding the scroll"
/// rather than a recycle. A perfectly content-anchored widget has residual 0; a
/// screen-anchored widget drifts ~1 row/frame against the text. A jump larger than
/// this means the widget was recycled to a different pocket (left at one edge,
/// re-entered at another), which is counted separately from per-frame jiggle.
const RIDE_TOLERANCE: i64 = 2;

/// Aggregate stability report over a scroll sequence.
#[derive(Debug, Clone, Default, Serialize)]
pub struct StabilityReport {
    /// Number of frames (scroll positions) analyzed.
    pub frames: usize,
    /// Number of scroll steps (frames - 1).
    pub steps: usize,
    /// Per-widget breakdown, sorted by descending distraction.
    pub widgets: Vec<WidgetMotion>,
    /// Total move events across all widgets.
    pub total_move_events: usize,
    /// Total flicker transitions (appearances + disappearances).
    pub total_flicker: usize,
    /// Total positional travel (x + y) across all widgets.
    pub total_travel: u32,
    /// Total *content-relative* vertical travel across all widgets (residual after
    /// subtracting the expected scroll-ride). This is near-zero when widgets stick to
    /// their negative-space spot in the transcript and only grows when they actually
    /// jump between pockets. Headline metric for the "ride the scroll" behaviour.
    pub total_content_travel: u32,
    /// Total recycles across all widgets: a widget left the viewport at one edge and a
    /// fresh instance entered at another (transcript scrolled past its pocket). This is
    /// expected and calm, unlike per-frame jiggle, so it is reported on its own.
    pub total_recycles: usize,
    /// Total size churn (width + height) across all widgets.
    pub total_size_churn: u32,
    /// Positional travel per 100 scroll lines (the headline distraction metric).
    pub travel_per_100_lines: f64,
    /// Content-relative vertical travel per 100 scroll lines. Lower means widgets
    /// ride the transcript more faithfully (stick to one negative-space spot).
    pub content_travel_per_100_lines: f64,
    /// Flicker transitions per 100 scroll lines.
    pub flicker_per_100_lines: f64,
    /// Composite distraction score per 100 scroll lines (travel + weighted flicker).
    pub distraction_per_100_lines: f64,
    /// Fraction of scroll steps in which at least one widget moved or flickered.
    pub unstable_step_fraction: f64,
    /// Widget kind contributing the most distraction.
    pub worst_widget: Option<String>,
    /// Average number of widgets visible per frame (information *breadth*).
    pub avg_widgets_visible: f64,
    /// Average total widget cell area visible per frame (information *volume*).
    pub avg_visible_cells: f64,
    /// Distinct widget kinds that were visible in at least one frame.
    pub distinct_kinds_seen: usize,
    /// For each kind ever seen, the fraction of frames it was actually visible.
    pub kind_visibility: Vec<KindVisibility>,
    /// Mean per-kind visible fraction (how reliably shown info stays shown).
    pub mean_kind_visibility: f64,
    /// Number of frames in which two visible widgets overlapped (should be 0).
    pub overlap_frames: usize,
    /// Worst-case count of overlapping widget pairs in any single frame.
    pub max_overlap_pairs: usize,
}

/// How reliably a single widget kind stayed visible across the scroll.
#[derive(Debug, Clone, Default, Serialize)]
pub struct KindVisibility {
    pub kind: String,
    pub frames_visible: usize,
    pub visible_fraction: f64,
}

/// Analyze a sequence of frames (each a list of placed widget rects) and compute
/// movement/flicker metrics. Frames are assumed to be consecutive scroll positions
/// differing by one content line, scrolling downward (transcript top advances by 1).
/// For the content-relative travel metric to reflect real scroll deltas, prefer
/// [`analyze_frames_with_scroll`].
pub fn analyze_frames(frames: &[Vec<PlacedRect>]) -> StabilityReport {
    // Default: assume each step advances the transcript top by exactly one line.
    let scroll_tops: Vec<i64> = (0..frames.len() as i64).collect();
    analyze_frames_with_scroll(frames, &scroll_tops)
}

/// Like [`analyze_frames`] but with explicit per-frame transcript tops, so the
/// content-relative travel metric subtracts the *real* scroll-ride. `scroll_tops[i]`
/// is the absolute transcript line shown on the first visible row of frame `i`. A
/// widget that rides its transcript line perfectly contributes zero content travel
/// even though its absolute `y` moves with the scroll.
pub fn analyze_frames_with_scroll(
    frames: &[Vec<PlacedRect>],
    scroll_tops: &[i64],
) -> StabilityReport {
    let mut report = StabilityReport {
        frames: frames.len(),
        steps: frames.len().saturating_sub(1),
        ..Default::default()
    };
    if frames.len() < 2 {
        // Still record presence so single-frame callers see widget set.
        if let Some(first) = frames.first() {
            let mut by_kind: BTreeMap<&'static str, WidgetMotion> = BTreeMap::new();
            for r in first {
                by_kind.entry(r.kind).or_default().frames_present += 1;
            }
            report.widgets = by_kind
                .into_iter()
                .map(|(k, mut m)| {
                    m.kind = k.to_string();
                    m
                })
                .collect();
        }
        return report;
    }

    let mut by_kind: BTreeMap<&'static str, WidgetMotion> = BTreeMap::new();
    let mut unstable_steps = 0usize;

    // Count presence in the very first frame.
    for r in &frames[0] {
        let m = by_kind.entry(r.kind).or_default();
        m.frames_present += 1;
    }

    for step in 0..frames.len() - 1 {
        let prev = &frames[step];
        let cur = &frames[step + 1];
        let mut step_unstable = false;

        // Signed scroll delta for this step: how far the transcript top advanced. A
        // content-anchored widget should move by `-scroll_delta` rows on screen
        // (content scrolls up as the top line advances), so the residual is
        // `signed_dy + scroll_delta`.
        let scroll_delta = scroll_tops
            .get(step + 1)
            .copied()
            .unwrap_or(step as i64 + 1)
            - scroll_tops.get(step).copied().unwrap_or(step as i64);

        // Index current frame by kind for lookup.
        let cur_index = |kind: &str| cur.iter().find(|r| r.kind == kind).copied();
        let prev_index = |kind: &str| prev.iter().find(|r| r.kind == kind).copied();

        // Gather the union of kinds present in either frame.
        let mut kinds: Vec<&'static str> = Vec::new();
        for r in prev.iter().chain(cur.iter()) {
            if !kinds.contains(&r.kind) {
                kinds.push(r.kind);
            }
        }

        for kind in kinds {
            let m = by_kind.entry(kind).or_default();
            match (prev_index(kind), cur_index(kind)) {
                (Some(p), Some(c)) => {
                    m.frames_present += 1;
                    let dx = abs_diff(p.x, c.x);
                    let dy = abs_diff(p.y, c.y);
                    let dw = abs_diff(p.width, c.width);
                    let dh = abs_diff(p.height, c.height);
                    if dx != 0 || dy != 0 {
                        m.move_events += 1;
                        step_unstable = true;
                    }
                    if dw != 0 || dh != 0 {
                        m.resize_events += 1;
                        step_unstable = true;
                    }
                    m.x_travel += dx as u32;
                    m.y_travel += dy as u32;
                    // Residual after removing the expected scroll-ride. Small residuals
                    // are per-frame jiggle (drift against the text); a large residual
                    // means the widget jumped to a different pocket (a recycle), which
                    // is counted separately so it doesn't masquerade as smooth travel.
                    let signed_dy = c.y as i64 - p.y as i64;
                    let residual = (signed_dy + scroll_delta).abs();
                    if residual <= RIDE_TOLERANCE {
                        m.content_y_travel += residual as u32;
                    } else {
                        m.recycles += 1;
                    }
                    m.width_churn += dw as u32;
                    m.height_churn += dh as u32;
                    m.max_jump = m.max_jump.max(dx.max(dy));
                }
                (None, Some(_)) => {
                    m.frames_present += 1;
                    m.appearances += 1;
                    step_unstable = true;
                }
                (Some(_), None) => {
                    m.disappearances += 1;
                    step_unstable = true;
                }
                (None, None) => {}
            }
        }

        if step_unstable {
            unstable_steps += 1;
        }
    }

    let mut widgets: Vec<WidgetMotion> = by_kind
        .into_iter()
        .map(|(k, mut m)| {
            m.kind = k.to_string();
            m
        })
        .collect();
    widgets.sort_by(|a, b| {
        b.distraction()
            .cmp(&a.distraction())
            .then(a.kind.cmp(&b.kind))
    });

    report.total_move_events = widgets.iter().map(|w| w.move_events).sum();
    report.total_flicker = widgets
        .iter()
        .map(|w| w.appearances + w.disappearances)
        .sum();
    report.total_travel = widgets.iter().map(|w| w.x_travel + w.y_travel).sum();
    report.total_content_travel = widgets
        .iter()
        .map(|w| w.x_travel + w.content_y_travel)
        .sum();
    report.total_recycles = widgets.iter().map(|w| w.recycles).sum();
    report.total_size_churn = widgets.iter().map(|w| w.width_churn + w.height_churn).sum();
    report.worst_widget = widgets
        .first()
        .filter(|w| w.distraction() > 0)
        .map(|w| w.kind.clone());

    let steps = report.steps.max(1) as f64;
    report.travel_per_100_lines = report.total_travel as f64 / steps * 100.0;
    report.content_travel_per_100_lines = report.total_content_travel as f64 / steps * 100.0;
    report.flicker_per_100_lines = report.total_flicker as f64 / steps * 100.0;
    let distraction: u32 = widgets.iter().map(|w| w.distraction()).sum();
    report.distraction_per_100_lines = distraction as f64 / steps * 100.0;
    report.unstable_step_fraction = unstable_steps as f64 / steps;
    report.widgets = widgets;

    // Information / coverage metrics: how much is actually on screen, averaged over
    // every frame. This is the counterweight to the stability metrics - a layout
    // that shows nothing would be perfectly "stable" but useless.
    let frame_count = frames.len().max(1) as f64;
    let total_widgets: usize = frames.iter().map(|f| f.len()).sum();
    let total_cells: u64 = frames
        .iter()
        .flat_map(|f| f.iter())
        .map(|r| r.width as u64 * r.height as u64)
        .sum();
    report.avg_widgets_visible = total_widgets as f64 / frame_count;
    report.avg_visible_cells = total_cells as f64 / frame_count;

    let mut visible_by_kind: BTreeMap<&'static str, usize> = BTreeMap::new();
    for frame in frames {
        let mut seen_this_frame: Vec<&'static str> = Vec::new();
        for r in frame {
            if !seen_this_frame.contains(&r.kind) {
                seen_this_frame.push(r.kind);
                *visible_by_kind.entry(r.kind).or_default() += 1;
            }
        }
    }
    report.distinct_kinds_seen = visible_by_kind.len();
    let mut kind_visibility: Vec<KindVisibility> = visible_by_kind
        .into_iter()
        .map(|(kind, frames_visible)| KindVisibility {
            kind: kind.to_string(),
            frames_visible,
            visible_fraction: frames_visible as f64 / frame_count,
        })
        .collect();
    kind_visibility.sort_by(|a, b| {
        b.visible_fraction
            .partial_cmp(&a.visible_fraction)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.kind.cmp(&b.kind))
    });
    report.mean_kind_visibility = if kind_visibility.is_empty() {
        0.0
    } else {
        kind_visibility
            .iter()
            .map(|k| k.visible_fraction)
            .sum::<f64>()
            / kind_visibility.len() as f64
    };
    report.kind_visibility = kind_visibility;

    // Overlap detection: no two visible widgets should share any cell in a frame.
    let mut overlap_frames = 0usize;
    let mut max_overlap_pairs = 0usize;
    for frame in frames {
        let mut pairs = 0usize;
        for i in 0..frame.len() {
            for j in (i + 1)..frame.len() {
                if rects_overlap(&frame[i], &frame[j]) {
                    pairs += 1;
                }
            }
        }
        if pairs > 0 {
            overlap_frames += 1;
            max_overlap_pairs = max_overlap_pairs.max(pairs);
        }
    }
    report.overlap_frames = overlap_frames;
    report.max_overlap_pairs = max_overlap_pairs;

    report
}

/// True when two placed rects share at least one cell.
fn rects_overlap(a: &PlacedRect, b: &PlacedRect) -> bool {
    let (ax1, ay1) = (a.x as u32, a.y as u32);
    let (bx1, by1) = (b.x as u32, b.y as u32);
    let ax2 = ax1 + a.width as u32;
    let ay2 = ay1 + a.height as u32;
    let bx2 = bx1 + b.width as u32;
    let by2 = by1 + b.height as u32;
    (ax1 < bx2 && bx1 < ax2) && (ay1 < by2 && by1 < ay2)
}

/// Drive the real layout algorithm over a synthetic content-width profile, scrolling
/// one content line at a time, and return the placements observed at each scroll
/// position. `content_widths[i]` is the rendered text width of content line `i`.
///
/// This faithfully exercises the production [`calculate_placements`] including the
/// sticky carry-over pass, so the resulting metrics reflect the real algorithm.
pub fn simulate_scroll(
    content_widths: &[u16],
    area_width: u16,
    viewport_height: u16,
    data: &InfoWidgetData,
) -> Vec<Vec<PlacedRect>> {
    simulate_scroll_mode(
        content_widths,
        area_width,
        viewport_height,
        data,
        SimMode::Anchored,
    )
}

/// Placement strategy to simulate, for A/B comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimMode {
    /// Production behaviour: carry anchors across frames (HUD pinning + hide-in-place).
    Anchored,
    /// "Maximize info every instant": re-solve from scratch each frame with no
    /// memory, so every frame independently fills the largest available pockets.
    /// This is the greedy ideal the stable layout is measured against.
    Greedy,
    /// Anchored carry PLUS look-ahead sizing: each row's free width is the minimum
    /// over a `±window` band of content lines, so a docked widget is pre-sized to
    /// clear long lines before they scroll under it. Eliminates the resize/blink.
    LookAhead(u16),
    /// Look-ahead sizing with NO anchor carry (re-solve each frame). Isolates how
    /// much stability comes from the smoothed profile alone vs the anchor logic.
    LookAheadFresh(u16),
    /// Anchored carry PLUS content anchoring. Since the resident model made
    /// content anchoring unconditional, this is now identical to `Anchored` and
    /// kept only so existing A/B benches keep compiling.
    ContentAnchored,
}

/// Build the per-row free-width profile for `scroll`. When `window > 0`, each row's
/// value is the *minimum* free width over the content lines `[line-window,
/// line+window]`, i.e. the width that stays safe across `±window` scroll steps.
/// `window == 0` reproduces the instantaneous profile.
fn right_widths_for_scroll(
    content_widths: &[u16],
    area_width: u16,
    scroll: usize,
    view: usize,
    window: u16,
) -> Vec<u16> {
    let w = window as usize;
    let total = content_widths.len();
    let used_at = |line: usize| -> u16 {
        content_widths
            .get(line)
            .copied()
            .unwrap_or(0)
            .min(area_width)
    };
    let mut right = Vec::with_capacity(view);
    for row in 0..view {
        let center = scroll + row;
        // Worst-case (max) used width across the look-ahead band -> min free width.
        let lo = center.saturating_sub(w);
        let hi = (center + w).min(total.saturating_sub(1));
        let mut max_used = 0u16;
        for line in lo..=hi {
            max_used = max_used.max(used_at(line));
        }
        right.push(area_width.saturating_sub(max_used));
    }
    right
}

/// Like [`simulate_scroll`] but lets the caller pick the placement strategy so the
/// stable (anchored) and greedy (max-info) layouts can be compared head to head.
pub fn simulate_scroll_mode(
    content_widths: &[u16],
    area_width: u16,
    viewport_height: u16,
    data: &InfoWidgetData,
    mode: SimMode,
) -> Vec<Vec<PlacedRect>> {
    let mut frames: Vec<Vec<PlacedRect>> = Vec::new();
    if area_width == 0 || viewport_height == 0 || content_widths.is_empty() {
        return frames;
    }

    let total_lines = content_widths.len();
    let view = viewport_height as usize;
    let max_scroll = total_lines.saturating_sub(view);
    let area = Rect::new(0, 0, area_width, viewport_height);
    let window = match mode {
        SimMode::LookAhead(w) | SimMode::LookAheadFresh(w) => w,
        _ => 0,
    };
    let greedy = matches!(mode, SimMode::Greedy | SimMode::LookAheadFresh(_));

    // Carry anchors across frames exactly like the live renderer does, so the
    // HUD pinning / hide-in-place behaviour is exercised identically.
    let mut anchors: Vec<WidgetAnchor> = Vec::new();

    for scroll in 0..=max_scroll {
        let right_widths = right_widths_for_scroll(content_widths, area_width, scroll, view, 0);
        let right_reliable = if window > 0 {
            right_widths_for_scroll(content_widths, area_width, scroll, view, window)
        } else {
            Vec::new()
        };
        let margins = Margins {
            right_widths,
            left_widths: Vec::new(),
            centered: false,
            right_reliable,
            left_reliable: Vec::new(),
            scroll_top: scroll,
        };
        // Greedy mode forgets all anchors each frame, so every frame independently
        // maximizes coverage (the old "fill the biggest pocket now" philosophy).
        let prev: &[WidgetAnchor] = if greedy { &[] } else { &anchors };
        let outcome = calculate_placements_anchored(area, &margins, data, true, prev);
        frames.push(
            outcome
                .visible
                .iter()
                .map(PlacedRect::from_placement)
                .collect(),
        );
        anchors = outcome.anchors;
    }

    frames
}

/// Convenience: simulate a scroll and return the aggregate report.
pub fn measure_scroll(
    content_widths: &[u16],
    area_width: u16,
    viewport_height: u16,
    data: &InfoWidgetData,
) -> StabilityReport {
    let frames = simulate_scroll(content_widths, area_width, viewport_height, data);
    analyze_frames(&frames)
}

/// Convenience: simulate a scroll in a specific mode and return the report.
pub fn measure_scroll_mode(
    content_widths: &[u16],
    area_width: u16,
    viewport_height: u16,
    data: &InfoWidgetData,
    mode: SimMode,
) -> StabilityReport {
    let frames = simulate_scroll_mode(content_widths, area_width, viewport_height, data, mode);
    analyze_frames(&frames)
}

fn abs_diff(a: u16, b: u16) -> u16 {
    a.abs_diff(b)
}

/// Map a captured widget-kind string back to a stable `&'static str` so live
/// frame captures (which carry owned `String` kinds) can flow through
/// [`analyze_frames`]. Unknown kinds collapse to `"other"`.
pub fn intern_kind(kind: &str) -> &'static str {
    use super::info_widget::WidgetKind;
    for k in WidgetKind::all_by_priority() {
        if k.as_str() == kind {
            return k.as_str();
        }
    }
    "other"
}

#[cfg(test)]
#[path = "info_widget_stability_tests.rs"]
mod tests;
