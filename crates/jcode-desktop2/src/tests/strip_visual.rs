//! Pixel-level invariants for the session strip.
//!
//! Split out of `visual` so neither file grows unbounded: the strip is its own
//! surface with its own rules (bar count, focus weight, band ownership), and it
//! is the only chrome allowed above the transcript.
//!
//! Requires a GPU, so these are `#[ignore]`d; run with
//! `cargo test -p jcode-desktop2 -- --ignored`.

use super::visual::Rendered;
use crate::states;

/// R8: the strip must actually read like the waybar module it is modelled on.
/// Not "some pixels appeared", but the specific signal the module carries: one
/// bar per session, and the focused one unmistakably heavier than the rest.
#[test]
#[ignore = "requires a GPU"]
fn the_strip_reads_like_the_waybar_module() {
    let model = states::by_name("session_strip").expect("session_strip node");
    let Some(rendered) = Rendered::new(&model) else {
        return;
    };
    let (top, bottom) = rendered
        .frame
        .strip()
        .expect("the strip node did not get a strip band");

    let items = crate::strip::layout_items(&model.strip, rendered.frame.left, rendered.frame.right);

    let mut focused_ink = None;
    let mut unfocused_ink = Vec::new();
    for item in &items {
        if let crate::strip::Item::Block {
            x, width, focused, ..
        } = item
        {
            // Sample the middle of the bar so a rounded corner or the label
            // beside it cannot be mistaken for the bar's own ink.
            let ink = 1.0
                - rendered.darkest_in(x + width / 2.0, top + 4.0, x + width / 2.0, bottom - 4.0);
            if *focused {
                focused_ink = Some(ink);
            } else {
                unfocused_ink.push(ink);
            }
        }
    }

    let focused_ink = focused_ink.expect("no focused bar was laid out");
    assert!(
        focused_ink > 0.5,
        "the focused bar barely inked ({focused_ink})"
    );
    assert!(
        !unfocused_ink.is_empty(),
        "the strip drew only one bar, so it says nothing about where you are"
    );
    for ink in &unfocused_ink {
        assert!(
            focused_ink > *ink,
            "the focused bar ({focused_ink}) was not heavier than an unfocused one ({ink})"
        );
    }
}

/// R9: the strip owns its band. Nothing else may paint into it, or the
/// indicator becomes unreadable exactly when there is a lot on screen.
#[test]
#[ignore = "requires a GPU"]
fn only_the_strip_inks_in_the_strip_band() {
    let mut model = states::by_name("session_strip").expect("session_strip node");
    // A transcript far longer than the page: the case that used to overflow.
    let messages: Vec<crate::transcript::Message> = (1..=200)
        .map(|n| {
            crate::transcript::Message::assistant(format!(
                "transcript line {n} with enough text to wrap the measure column"
            ))
        })
        .collect();
    model.transcript = crate::transcript::Transcript::from(&messages[..]);
    let Some(rendered) = Rendered::new(&model) else {
        return;
    };
    let (_, bottom) = rendered.frame.strip().expect("strip band");

    // The gap between the strip's bars and the transcript must stay clear.
    let gap_ink = rendered.ink_rows(
        rendered.frame.left,
        bottom + 1.0,
        rendered.frame.right,
        rendered.frame.body_top - 1.0,
    );
    assert_eq!(
        gap_ink, 0,
        "{gap_ink} rows of something else were drawn in the strip's gap"
    );
}
