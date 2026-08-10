//! Pixel-level invariants for the overview's card thumbnails.
//!
//! The field's whole claim is that you can see several sessions at once. That
//! is a claim about ink: geometry tests can prove a card is in the right place
//! while its contents are an unreadable smudge, which is the state the first
//! attempt at this shipped in. So these measure contrast inside the cards.
//!
//! Requires a GPU, so these are `#[ignore]`d; run with
//! `cargo test -p jcode-desktop2 -- --ignored`.

use super::visual::Rendered;
use crate::states;

/// The surface has to be big enough for cards that clear the thumbnail floor;
/// below it the previews are correctly suppressed and there is nothing to
/// measure.
const SURFACE: (u32, u32, f64) = (1600, 1100, 1.5);
/// Inset from a card's edge before any ink is measured. The focused card's ring
/// is 2.5 logical units of near-black, so a smaller inset would sample the
/// border on that one card and report it as text: every band would pass, and
/// the comparison between bands would be a comparison of the same ring.
const RING_CLEARANCE: f64 = 5.0;

fn field(model: &crate::Model) -> (Rendered, crate::overview::Field) {
    let (width, height, scale) = SURFACE;
    let rendered = Rendered::at(model, width, height, scale).expect("a GPU render");
    let field = crate::overview::layout(
        &model.strips.panels(),
        model.overview.focus().or(model.session_id.as_deref()),
        model.session_id.as_deref(),
        crate::overview::area(&rendered.frame),
    );
    (rendered, field)
}

/// Every card shows its own conversation, and shows it legibly.
///
/// The failure this exists to catch is the one that is easy to ship: a preview
/// so faint it costs a card's space and tells the user nothing, which is worse
/// than no preview at all because it cannot be distinguished from one.
#[test]
#[ignore = "requires a GPU"]
fn every_card_shows_its_own_conversation() {
    let model = states::by_name("overview_thumbnails").expect("node");
    let (rendered, field) = field(&model);
    let mut measured = 0;
    for card in &field.cards {
        let (x0, y0, x1, y1) = card.rect;
        if x1 - x0 < 100.0 || y1 - y0 < 60.0 {
            continue;
        }
        // The upper band, which is the thumbnail's; the name lives below it and
        // would otherwise be what passes this test.
        let ink = rendered.darkest_in(
            x0 + RING_CLEARANCE,
            y0 + RING_CLEARANCE,
            x1 - RING_CLEARANCE,
            y0 + (y1 - y0) * 0.55,
        );
        assert!(
            ink < 0.72,
            "{}: its card carries no readable preview (darkest {ink:.3})",
            card.session_id
        );
        measured += 1;
    }
    assert!(
        measured >= 2,
        "the surface held {measured} previewable cards, so nothing was really tested"
    );
}

/// A card's name stays readable with a conversation above it. The preview is
/// context; the name is the thing the user acts on, so it may never be the
/// thing that gets crowded out.
#[test]
#[ignore = "requires a GPU"]
fn the_name_survives_having_a_preview_above_it() {
    let model = states::by_name("overview_thumbnails").expect("node");
    let (rendered, field) = field(&model);
    for card in &field.cards {
        let (x0, y0, x1, y1) = card.rect;
        if x1 - x0 < 100.0 || y1 - y0 < 60.0 {
            continue;
        }
        let band = rendered.darkest_in(
            x0 + RING_CLEARANCE,
            y0 + (y1 - y0) * 0.62,
            x1 - RING_CLEARANCE,
            y1 - RING_CLEARANCE,
        );
        let preview = rendered.darkest_in(
            x0 + RING_CLEARANCE,
            y0 + RING_CLEARANCE,
            x1 - RING_CLEARANCE,
            y0 + (y1 - y0) * 0.55,
        );
        assert!(
            band < 0.6,
            "{}: no name in the band under its preview (darkest {band:.3})",
            card.session_id
        );
        // The name has to win the card: if the preview were as heavy, the tile
        // would read as a wall of text with no handle on it.
        assert!(
            band < preview,
            "{}: its preview ({preview:.3}) is as heavy as its name ({band:.3})",
            card.session_id
        );
    }
}
