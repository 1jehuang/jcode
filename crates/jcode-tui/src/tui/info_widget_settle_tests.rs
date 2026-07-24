//! Tests for [`SettlementTracker`]: negative space must hold still for
//! [`SETTLE_AFTER_FRAMES`] frames before it may host a new widget, churn resets
//! the counter, scrolled-in rows inherit prior observations, and a width change
//! flushes everything.

use super::{SETTLE_AFTER_FRAMES, SettlementTracker};
use crate::tui::info_widget_layout::Margins;

fn margins(right: Vec<u16>, scroll_top: usize) -> Margins {
    Margins {
        right_widths: right,
        scroll_top,
        ..Default::default()
    }
}

#[test]
fn stable_rows_settle_after_threshold() {
    let mut t = SettlementTracker::default();
    let m = margins(vec![30, 30, 30], 0);
    for _ in 0..SETTLE_AFTER_FRAMES {
        let s = t.observe(&m, 80);
        assert!(s.right.iter().all(|&w| w == 0), "must not settle early");
    }
    let s = t.observe(&m, 80);
    assert_eq!(s.right, vec![30, 30, 30], "settles after threshold");
}

#[test]
fn changing_row_resets_its_counter_only() {
    let mut t = SettlementTracker::default();
    for _ in 0..=SETTLE_AFTER_FRAMES {
        t.observe(&margins(vec![30, 30], 0), 80);
    }
    // Row 1 churns (streaming tail); row 0 stays settled.
    let s = t.observe(&margins(vec![30, 10], 0), 80);
    assert_eq!(s.right[0], 30);
    assert_eq!(s.right[1], 0, "churned row must unsettle");
    // Row 1 must re-earn settlement from scratch: the churn frame reset its
    // counter to 0, so it needs SETTLE_AFTER_FRAMES more identical observations.
    for _ in 0..SETTLE_AFTER_FRAMES - 1 {
        let s = t.observe(&margins(vec![30, 10], 0), 80);
        assert_eq!(s.right[1], 0);
    }
    let s = t.observe(&margins(vec![30, 10], 0), 80);
    assert_eq!(s.right[1], 10);
}

#[test]
fn settlement_is_keyed_by_transcript_line_not_screen_row() {
    let mut t = SettlementTracker::default();
    // Settle transcript lines 0..3 at scroll 0.
    for _ in 0..=SETTLE_AFTER_FRAMES {
        t.observe(&margins(vec![30, 30, 30], 0), 80);
    }
    // Scroll down by 1: lines 1..3 are already settled at their new screen rows;
    // line 3 is fresh and must not be.
    let s = t.observe(&margins(vec![30, 30, 30], 1), 80);
    assert_eq!(s.right[0], 30, "line 1 stays settled after scroll");
    assert_eq!(s.right[1], 30, "line 2 stays settled after scroll");
    assert_eq!(s.right[2], 0, "line 3 is unseen, not settled");
}

#[test]
fn width_change_flushes_all_observations() {
    let mut t = SettlementTracker::default();
    for _ in 0..=SETTLE_AFTER_FRAMES {
        t.observe(&margins(vec![30, 30], 0), 80);
    }
    // Terminal resized: every line re-wrapped, nothing may stay settled.
    let s = t.observe(&margins(vec![30, 30], 0), 100);
    assert!(s.right.iter().all(|&w| w == 0), "resize must flush");
}
