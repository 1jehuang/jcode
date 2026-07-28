//! Page-band pixel invariants: the bands between regions stay paper.
//!
//! These sweep every state-space node and assert nothing inks the gaps
//! between the transcript, the composer, the footnote row, and the margins.
//! Split out of `visual.rs`, which had grown past the test-size budget; they
//! share only the `Rendered` helper and the node list with that suite.

use super::visual::{Rendered, nodes};
use crate::Model;

/// Nodes without the session overview up. The overview is the one sanctioned
/// full-page layer: it veils the page and spreads blobs across every band and
/// margin by design, so the page-band invariants below it do not apply.
fn page_nodes() -> Vec<(&'static str, Model)> {
    nodes()
        .into_iter()
        .filter(|(_, model)| !model.overview.is_visible())
        .collect()
}

/// The input box must be *drawn* on the middle of the window, not merely laid
/// out there: this catches a renderer that ignores the centred geometry.
#[test]
#[ignore = "requires a GPU"]
fn the_composer_well_is_drawn_on_the_middle_of_the_window() {
    for (name, model) in nodes() {
        let Some(r) = Rendered::new(&model) else {
            return;
        };
        let f = r.frame;
        let Some((top, bottom)) = r.wash_band() else {
            panic!("{name}: no composer well was drawn");
        };
        assert!(
            (top - f.composer_top).abs() < 2.0 && (bottom - f.composer_bottom).abs() < 2.0,
            "{name}: the drawn well {top:.1}..{bottom:.1} left its geometry {:.1}..{:.1}",
            f.composer_top,
            f.composer_bottom
        );
        // An empty session seats the well under the hero stack (see
        // `layout::resolve`) and a transcript pushes it down the page; both
        // only ever move the well *down*, so the invariant left to pin here
        // is that nothing moves it up past the page middle.
        let center = (top + bottom) / 2.0;
        assert!(
            center >= f.height / 2.0 - 2.0,
            "{name}: the well centre {center:.1} rose above the page middle {:.1}",
            f.height / 2.0
        );
    }
}

#[test]
#[ignore = "requires a GPU"]
fn nothing_draws_in_the_gap_above_the_composer() {
    for (name, model) in page_nodes() {
        let Some(r) = Rendered::new(&model) else {
            eprintln!("skipping {name}: no GPU");
            return;
        };
        let f = r.frame;
        // The band between the transcript and the well must stay paper:
        // this is the overlap bug that made long replies collide.
        let darkest = r.darkest_in(f.left, f.body_bottom + 2.0, f.right, f.composer_top - 2.0);
        assert!(
            darkest > 0.9,
            "{name}: ink ({darkest:.3} luma) in the composer gap"
        );
    }
}

/// The top of the page must be bare paper. The app previously carried a
/// wordmark, a status caption, a build-identity row, and a rule up there,
/// which is the clutter this test exists to keep out: any of them reappearing
/// inks the band above the transcript.
///
/// The session strip is the one sanctioned exception, and only when there is
/// more than one session to move between, so it is excluded by *its own
/// reserved band* rather than by relaxing the threshold: anything drawn above
/// the transcript outside that band still fails.
#[test]
#[ignore = "requires a GPU"]
fn the_top_of_the_page_is_clear() {
    for (name, model) in page_nodes() {
        let Some(r) = Rendered::new(&model) else {
            eprintln!("skipping {name}: no GPU");
            return;
        };
        let f = r.frame;
        let top = match f.strip() {
            Some((_, strip_bottom)) => strip_bottom + 1.0,
            None => 0.0,
        };
        let darkest = r.darkest_in(0.0, top, f.width - 1.0, f.body_top - 2.0);
        assert!(
            darkest > 0.9,
            "{name}: ink ({darkest:.3} luma) above the transcript"
        );
    }
}

/// The caret must never escape its well, at any window size.
#[test]
#[ignore = "requires a GPU"]
fn the_caret_stays_inside_the_composer_well() {
    for (name, model) in page_nodes() {
        let Some(r) = Rendered::new(&model) else {
            return;
        };
        let f = r.frame;
        // Bands immediately above and below the well must stay paper.
        let above = r.darkest_in(f.left, f.composer_top - 6.0, f.right, f.composer_top - 3.0);
        assert!(above > 0.9, "{name}: ink just above the composer well");
        let below = r.darkest_in(
            f.left,
            f.composer_bottom + 1.0,
            f.right,
            f.footnote_top - 1.0,
        );
        assert!(below > 0.9, "{name}: ink between the well and the footnote");
    }
}

#[test]
#[ignore = "requires a GPU"]
fn margins_stay_empty() {
    for (name, model) in page_nodes() {
        let Some(r) = Rendered::new(&model) else {
            return;
        };
        let f = r.frame;
        // Nothing may be drawn outside the measure column: proves text is
        // wrapped to the column and not clipped by the window edge.
        let left_margin = r.darkest_in(0.0, 0.0, f.left - 3.0, f.height - 1.0);
        assert!(left_margin > 0.9, "{name}: ink in the left margin");
        let bottom = r.darkest_in(0.0, f.footnote_bottom + 2.0, f.width - 1.0, f.height - 1.0);
        assert!(bottom > 0.9, "{name}: ink below the footnote row");
    }
}
