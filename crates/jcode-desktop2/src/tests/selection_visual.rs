//! Pixel invariants for the transcript highlight.
//!
//! Split from `visual` to keep that file within the code-size budget, and
//! because these prove a different thing: not that a rectangle is in the right
//! place, but that the band and the surface under it are actually
//! distinguishable. That is how this feature fails silently, since a user card
//! and a paper-tuned band are nearly the same grey.

use super::visual::Rendered;
use crate::states;
/// The transcript highlight must actually paint, and the highlighted words
/// must still be readable on top of it. Only real pixels can prove this: the
/// geometry tests know the band's rectangle, but not whether the band and the
/// surface under it are distinguishable, which is the way this feature fails
/// silently (a user card and a paper-tuned band are nearly the same grey).
#[test]
#[ignore = "requires a GPU"]
fn the_transcript_highlight_is_visible_on_both_surfaces() {
    let model = states::by_name("transcript_selection").expect("node");
    assert!(model.selection.is_some(), "node carries no selection");
    let Some(r) = Rendered::new(&model) else {
        return;
    };
    let f = r.frame;
    let s = f.scale;
    let mut band_pixels = 0u32;
    let mut ink_pixels = 0u32;
    for y in ((f.body_top * s) as u32)..((f.body_bottom * s) as u32) {
        for x in ((f.left * s) as u32)..((f.right * s) as u32) {
            let luma = r.luma(x, y);
            // Mid-tones are the band: paper is near 1.0 and ink near 0.0.
            if (0.5..0.92).contains(&luma) {
                band_pixels += 1;
            }
            if luma < 0.35 {
                ink_pixels += 1;
            }
        }
    }
    assert!(
        band_pixels > 500,
        "the transcript highlight barely painted ({band_pixels} px)"
    );
    assert!(
        ink_pixels > 500,
        "highlighted text was buried by the band ({ink_pixels} px of ink)"
    );
}

/// And without a selection the transcript must not look highlighted, or the
/// band above would be proving nothing.
#[test]
#[ignore = "requires a GPU"]
fn no_transcript_band_is_drawn_without_a_selection() {
    let selected = states::by_name("transcript_selection").expect("node");
    let plain = states::by_name("turn_done").expect("node");
    assert!(plain.selection.is_none());
    let (Some(with), Some(without)) = (Rendered::new(&selected), Rendered::new(&plain)) else {
        return;
    };
    let f = with.frame;
    let s = f.scale;
    let band_pixels = |r: &Rendered| {
        let mut count = 0u32;
        for y in ((f.body_top * s) as u32)..((f.body_bottom * s) as u32) {
            for x in ((f.left * s) as u32)..((f.right * s) as u32) {
                if (0.5..0.92).contains(&r.luma(x, y)) {
                    count += 1;
                }
            }
        }
        count
    };
    assert!(
        band_pixels(&with) > band_pixels(&without) * 2,
        "the highlighted frame is not visibly more banded than the plain one"
    );
}
