//! The hero block: the wordmark sits over the donut and the tagline under it,
//! stacked like the website's landing section. Pixel-level, because the whole
//! point is optical: the geometry can be right while the ink is not.
//!
//! Split out of `visual.rs`, which had grown past the test-size budget. These
//! tests share only the `Rendered` helper with the rest of the visual suite.

use super::visual::Rendered;
use crate::states;

fn hero_frame() -> (Rendered, crate::layout::Hero) {
    let model = states::by_name("attached_empty").expect("node");
    let r = Rendered::new(&model).expect("a GPU render");
    let hero = r.frame.hero().expect("a hero at test size");
    (r, hero)
}

/// Inked rows inside a logical band, used to find where the donut's ink
/// actually starts and stops.
fn ink_band(r: &Rendered, x0: f64, x1: f64, y0: f64, y1: f64) -> Option<(f64, f64)> {
    let s = r.frame.scale;
    let px = |v: f64, max: u32| (v * s).round().clamp(0.0, f64::from(max - 1)) as u32;
    let (a, b) = (px(x0, r.width), px(x1, r.width));
    let mut first = None;
    let mut last = None;
    for y in px(y0, r.height)..=px(y1, r.height) {
        if (a..=b).any(|x| r.luma(x, y) < 0.9) {
            first = first.or(Some(y));
            last = Some(y);
        }
    }
    Some((f64::from(first?) / s, f64::from(last?) / s))
}

#[test]
#[ignore = "requires a GPU"]
fn the_wordmark_sits_above_the_donut_and_the_tagline_below_it() {
    let (r, hero) = hero_frame();
    let wordmark = ink_band(
        &r,
        r.frame.left,
        r.frame.right,
        hero.wordmark_top - 2.0,
        hero.donut.y0,
    )
    .expect("wordmark ink");
    let tagline = ink_band(
        &r,
        r.frame.left,
        r.frame.right,
        hero.tagline_top - 2.0,
        r.frame.body_bottom,
    )
    .expect("tagline ink");
    assert!(
        wordmark.1 < tagline.0,
        "the hero stack is out of order: wordmark {wordmark:?}, tagline {tagline:?}"
    );
    // Neither line may collide with the donut's own ink.
    let donut = ink_band(
        &r,
        hero.donut.x0,
        hero.donut.x1,
        hero.donut.y0,
        hero.donut.y1,
    )
    .expect("donut ink");
    assert!(donut.0 > wordmark.1, "the donut runs into the wordmark");
    assert!(donut.1 < tagline.0, "the donut runs into the tagline");
}

/// The halftone must be dark enough to read as ink, not a grey haze. The
/// old pitch scaled with the box, which thinned the dots on a small donut
/// until it looked washed out.
#[test]
#[ignore = "requires a GPU"]
fn the_halftone_is_actually_dark() {
    let (r, hero) = hero_frame();
    let darkest = r.darkest_in(hero.donut.x0, hero.donut.y0, hero.donut.x1, hero.donut.y1);
    assert!(
        darkest < 0.15,
        "the donut's darkest ink is only {darkest:.3}; the halftone is washed out"
    );
}

/// The hole is the whole reason for the tilt: it must survive the halftone.
#[test]
#[ignore = "requires a GPU"]
fn the_donut_has_a_visible_hole() {
    let (r, hero) = hero_frame();
    let cx = (hero.donut.x0 + hero.donut.x1) / 2.0;
    let cy = (hero.donut.y0 + hero.donut.y1) / 2.0;
    // Probe a window sized relative to the donut, so this measures the hole
    // rather than a fixed patch that a small donut fills with ring.
    let probe = hero.donut.width() * 0.04;
    let lightest = r.darkest_in(cx - probe, cy - probe, cx + probe, cy + probe);
    assert!(
        lightest > 0.9,
        "the donut's hole is inked ({lightest:.3}); the pose lost its hole"
    );
}

/// The hero is centred on the column, like the website's centred flexbox.
#[test]
#[ignore = "requires a GPU"]
fn the_hero_is_horizontally_centred() {
    let (r, hero) = hero_frame();
    let s = r.frame.scale;
    let y = ((hero.wordmark_top + 8.0) * s).round() as u32;
    let row: Vec<u32> = (0..r.width).filter(|x| r.luma(*x, y) < 0.8).collect();
    let (first, last) = (
        *row.first().expect("wordmark ink on this row"),
        *row.last().expect("wordmark ink"),
    );
    let centre = f64::from(first + last) / 2.0 / s;
    let column_centre = (r.frame.left + r.frame.right) / 2.0;
    assert!(
        // Tolerance of roughly one glyph: the wordmark is centred by
        // Parley on its advance widths, so the inked extents are a glyph's
        // side bearings off dead centre.
        (centre - column_centre).abs() < f64::from(crate::layout::HERO_WORDMARK_SIZE) / 2.0,
        "wordmark centred at {centre:.1}, column centre {column_centre:.1}"
    );
}

/// The hero must stand down completely once there is a transcript: dots
/// behind real output would be unreadable noise.
#[test]
#[ignore = "requires a GPU"]
fn the_hero_leaves_no_ink_once_there_is_a_transcript() {
    let empty = states::by_name("attached_empty").expect("node");
    let streaming = states::by_name("streaming").expect("node");
    let r = Rendered::new(&empty).expect("a GPU render");
    let hero = r.frame.hero().expect("a hero");
    let with_text = Rendered::new(&streaming).expect("a GPU render");
    // Sample the donut's own hole region, which the transcript's own lines
    // cannot reach in the streaming node's short text.
    let cx = (hero.donut.x0 + hero.donut.x1) / 2.0;
    let ring_top = hero.donut.y0 + hero.donut.height() * 0.12;
    let inked_when_empty = r.darkest_in(cx - 20.0, ring_top, cx + 20.0, ring_top + 6.0);
    let inked_with_text = with_text.darkest_in(cx - 20.0, ring_top, cx + 20.0, ring_top + 6.0);
    assert!(inked_when_empty < 0.5, "no donut ink to begin with");
    assert!(
        inked_with_text > 0.9,
        "the donut is still inking behind the transcript ({inked_with_text:.3})"
    );
}
