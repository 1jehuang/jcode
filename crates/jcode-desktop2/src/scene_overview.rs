//! The session overview layer: the card strip, its preview, and its hint.
//!
//! Split from [`crate::scene`] because the overview is a mode drawn over the
//! page rather than part of it: it has its own constants, its own layering
//! rules, and none of the transcript's machinery. `build_scene` calls
//! [`draw_overview`] last, which is what lets the field wash the page it
//! replaces.

use crate::scene::draw_spinner;
use crate::text::ParagraphStyle;
use crate::{Model, layout, text};
use vello::Scene;
use vello::kurbo::{Affine, Rect, RoundedRect};

/// Size of a card's session label.
const CARD_LABEL_SIZE: f32 = 12.0;
/// Smallest a card's name may be set before it is dropped entirely: below this
/// it is illegible, and illegible text is noise rather than a label.
const CARD_LABEL_MIN: f32 = 7.0;
/// Size of a row's workspace name, above its cards.
const ROW_LABEL_SIZE: f32 = 13.0;
/// Corner radius of a card. Soft enough to read as a tile, square enough to
/// read as a window rather than a pill.
const CARD_CORNER: f64 = 7.0;
/// Border thickness for an unfocused card, and for the focused one. The
/// focused ring is the compositor's focus border, which is the whole visual
/// signal the strip carries.
const CARD_RING: f64 = 1.25;
const CARD_RING_FOCUS: f64 = 2.5;
/// How far past its edge the focused card's halo reaches.
const CARD_HALO: f64 = 5.0;
/// How much a busy card's border breathes, as a fraction of its ring weight.
const BUSY_PULSE: f64 = 0.06;
/// Period of that breath, in seconds.
const BUSY_PERIOD: f32 = 1.6;
/// How far the page is veiled behind the field, at full zoom. Short of opaque
/// on purpose: the transcript underneath is context, not clutter, and seeing
/// it is what keeps the overview a layer rather than a separate screen.
const VEIL_OPACITY: f64 = 0.28;
/// Type size and leading for the conversation drawn inside a card. Tiny: this
/// is a thumbnail of a session's shape, in the same sense as a compositor's
/// window preview, so it is recognised rather than read.
const THUMB_SIZE: f32 = 7.5;
const THUMB_LEADING: f64 = 1.55;
/// Inset from a card's edge to its thumbnail text.
const THUMB_PAD: f64 = 7.0;
/// Smallest card that carries a thumbnail at all. Below this the text would be
/// two clipped words, which reads as damage rather than as content.
const MIN_THUMB_WIDTH: f64 = 92.0;
const MIN_THUMB_HEIGHT: f64 = 54.0;
/// Room kept clear on a busy card's first lines, for its spinner.
const SPINNER_CLEARANCE: f64 = 22.0;
/// How much of a card's height the thumbnail may fill before it stops, leaving
/// the rest to the name band underneath.
const THUMB_BAND: f64 = 0.62;
/// Ink for a card's thumbnail. Only slightly under full strength: the first
/// attempt set this at two thirds and the preview was a grey smudge, which is
/// worse than no preview because it costs the same space and says nothing.
const THUMB_OPACITY: f64 = 0.9;
/// Smallest card that carries a busy spinner. Below this the spinner would be
/// larger than the session it belongs to.
const MIN_SPINNER_WIDTH: f64 = 44.0;

/// Cut a line to fit, keeping the front.
///
/// Deliberately not [`elide`]: dropping the middle of a sentence produces
/// "why is the halftone...n in logical units?", which the eye reads as a
/// rendering fault rather than as an abbreviation. A thumbnail line only has to
/// be recognisable, and the front of a message is what identifies it, so the
/// tail is the part that can be spent.
fn truncate(text: &str, max_chars: usize) -> String {
    let text = text.trim();
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    if max_chars <= 1 {
        return "\u{2026}".to_string();
    }
    let mut out: String = text.chars().take(max_chars - 1).collect();
    // Trailing space before an ellipsis reads as a gap in the text.
    while out.ends_with(' ') {
        out.pop();
    }
    out.push('\u{2026}');
    out
}

/// Draw a session's conversation inside its own card.
///
/// This is what makes the field an actual view of several sessions at once
/// rather than a set of labelled boxes: a compositor's overview shows you the
/// *windows*, not their titles, and the equivalent for a conversation is the
/// last few things said in it. Every card carries its own, so "which one was
/// the refactor" is answered by looking rather than by visiting each in turn.
///
/// Set faint and clipped to the card, one line per message, alternating ink by
/// role. The alternation is the only structure kept: at this size it is the
/// texture of a conversation that identifies it, and wrapped paragraphs would
/// push the messages that distinguish one session from another off the tile.
fn draw_card_thumbnail(
    scene: &mut Scene,
    text: &mut text::TextSystem,
    model: &Model,
    card: &crate::overview::Card,
    rect: Rect,
    phase: f64,
    scale: f64,
) {
    if rect.width() < MIN_THUMB_WIDTH || rect.height() < MIN_THUMB_HEIGHT {
        return;
    }
    let Some(transcript) = model.peeks.get(&card.session_id) else {
        return;
    };
    // A busy card carries a spinner in its top-right corner, so the text stops
    // short of it: overprinting the two made the first line unreadable on
    // exactly the cards whose state the user most wants to check.
    let width = rect.width()
        - THUMB_PAD * 2.0
        - match card.busy && rect.width() > MIN_SPINNER_WIDTH {
            true => SPINNER_CLEARANCE,
            false => 0.0,
        };
    // The band the thumbnail may use, leaving the lower part of the tile to the
    // session's name: a preview that ran under the label would make both
    // illegible, and the name is what the user acts on.
    let ceiling = rect.y0 + rect.height() * THUMB_BAND;
    let mut y = rect.y0 + THUMB_PAD;
    // Clipped to the tile so a long line can never bleed onto a neighbour: at
    // this density two cards' text running together would read as one session.
    scene.push_layer(
        vello::peniko::Fill::NonZero,
        vello::peniko::Mix::Normal,
        1.0,
        Affine::scale(scale),
        &RoundedRect::from_rect(rect, CARD_CORNER),
    );
    // Newest last, like the page itself, but only as many as the band holds:
    // the tail is what a session is *doing*, so it is what a preview owes.
    let budget = (width / (f64::from(THUMB_SIZE) * 0.6)) as usize;
    for message in transcript.messages() {
        if y + f64::from(THUMB_SIZE) > ceiling {
            break;
        }
        let source = message.source.trim();
        if source.is_empty() {
            continue;
        }
        let line = truncate(&source.replace('\n', " "), budget.max(8));
        text.draw_paragraph_scaled(
            scene,
            &line,
            (rect.x0 + THUMB_PAD, y),
            width as f32,
            ParagraphStyle {
                font_size: THUMB_SIZE,
                color: if message.role == crate::transcript::Role::User {
                    model.theme.muted
                } else {
                    model.theme.faint
                }
                .with_alpha((THUMB_OPACITY * phase) as f32),
                line_height: THUMB_LEADING as f32,
                ..Default::default()
            },
            scale,
        );
        y += f64::from(THUMB_SIZE) * THUMB_LEADING;
    }
    scene.pop_layer();
}

/// Draw the session overview: every live session as a card in a row of
/// workspaces, the compositor's own overview.
///
/// The field fades and scales in together, from the current card's position
/// outward, so opening reads as the window zooming out of the conversation
/// you are in rather than as a panel appearing over it. That is the whole
/// illusion, and it is why the phase drives *geometry* here and not just an
/// alpha ramp.
pub(crate) fn draw_overview(
    scene: &mut Scene,
    text: &mut text::TextSystem,
    model: &Model,
    frame: &layout::Frame,
    scale: f64,
    now: std::time::Instant,
) {
    let phase = model.overview.phase();
    if phase <= 0.0 {
        return;
    }
    let theme = &model.theme;
    let field = crate::overview::layout(
        &model.strips.panels(),
        model.overview.focus().or(model.session_id.as_deref()),
        model.session_id.as_deref(),
        crate::overview::area(frame),
    );
    if field.cards.is_empty() {
        return;
    }

    // Veil the page rather than replace it. The conversation stays visible
    // underneath, so the field reads as a layer over the work you were doing
    // instead of as a different screen you have been taken to: you never lose
    // your place, and the switch is a glance rather than a context change.
    //
    // Only a light scrim is needed because the field now has its own paper.
    let veil = (VEIL_OPACITY * phase) as f32;
    scene.fill(
        vello::peniko::Fill::NonZero,
        Affine::scale(scale),
        theme.background.with_alpha(veil),
        None,
        &Rect::new(0.0, 0.0, frame.width, frame.height),
    );

    // A real bounded surface, inset from every window edge. This is the visual
    // contract of the button: sessions are rendered inside an overlay while
    // the transcript the user was working on remains visible around it.
    let (x0, y0, x1, y1) = crate::overview::area(frame);
    let panel = RoundedRect::new(x0, y0, x1, y1, 12.0);
    scene.fill(
        vello::peniko::Fill::NonZero,
        Affine::scale(scale),
        theme.background.with_alpha((0.96 * phase) as f32),
        None,
        &panel,
    );
    scene.stroke(
        &vello::kurbo::Stroke::new(1.0),
        Affine::scale(scale),
        theme.rule.with_alpha(phase as f32),
        None,
        &panel,
    );

    // Everything flies out from the card you came from, so the session on
    // screen stays under the eye through the whole transition.
    let origin = field
        .cards
        .iter()
        .find(|card| card.current)
        .or_else(|| field.focused())
        .map(|card| card.center())
        .unwrap_or((frame.width / 2.0, frame.height / 2.0));
    let place = |point: (f64, f64)| {
        (
            origin.0 + (point.0 - origin.0) * phase,
            origin.1 + (point.1 - origin.1) * phase,
        )
    };
    // A rect scaled about the origin, so the strip grows out of the card the
    // window came from exactly as the compositor's overview does.
    let place_rect = |rect: (f64, f64, f64, f64)| {
        let (x0, y0) = place((rect.0, rect.1));
        let (x1, y1) = place((rect.2, rect.3));
        Rect::new(x0, y0, x1, y1)
    };

    // A workspace's name sits above its row, aligned to the row's left edge
    // like the strip's own group labels: the label belongs to the place, not
    // to any one card in it.
    for row in &field.rows {
        let (left, top) = place((row.left, row.top));
        text.draw_paragraph_scaled(
            scene,
            &row.label,
            (left, top),
            240.0,
            ParagraphStyle {
                font_size: ROW_LABEL_SIZE,
                color: theme.faint.with_alpha(phase as f32),
                letter_spacing_em: 0.14,
                line_height: 1.0,
                ..Default::default()
            },
            scale,
        );
    }

    for card in &field.cards {
        let rect = place_rect(card.rect);
        if rect.width() <= 2.0 || rect.height() <= 2.0 {
            continue;
        }
        let tile = RoundedRect::from_rect(rect, CARD_CORNER * phase);

        // The focused card carries a halo, so the highlight survives being
        // next to a much wider neighbour: a ring alone reads as "big", while
        // a halo reads as "chosen".
        if card.focused {
            scene.fill(
                vello::peniko::Fill::NonZero,
                Affine::scale(scale),
                theme.wash.with_alpha(phase as f32),
                None,
                &RoundedRect::from_rect(
                    rect.inflate(CARD_HALO, CARD_HALO),
                    CARD_CORNER + CARD_HALO,
                ),
            );
        }
        // Fill: the session you are in is inked, the rest are paper, so "where
        // am I" is answered before any label is read.
        scene.fill(
            vello::peniko::Fill::NonZero,
            Affine::scale(scale),
            if card.current {
                theme.wash.with_alpha(phase as f32)
            } else {
                theme.background.with_alpha(phase as f32)
            },
            None,
            &tile,
        );
        // Only the highlight gets a heavy border: a thick border on a busy
        // card was indistinguishable from the focused one, so a field with
        // work running in it appeared to have two selections. A busy card's
        // border breathes instead, which pinned captures freeze.
        let pulse = if card.busy {
            1.0 + BUSY_PULSE * crate::overview::breath(model.activity.elapsed(now), BUSY_PERIOD)
        } else {
            1.0
        };
        scene.stroke(
            &vello::kurbo::Stroke::new(
                if card.focused {
                    CARD_RING_FOCUS
                } else {
                    CARD_RING
                } * pulse,
            ),
            Affine::scale(scale),
            if card.focused { theme.text } else { theme.rule }.with_alpha(phase as f32),
            None,
            &tile,
        );
        // Work is signalled by a mark rather than by the border's weight: a
        // spinner in the card's corner, the same halftone comet the composer
        // uses, so "this session is working" looks the same everywhere in the
        // app and cannot be confused with "this session is selected".
        if card.busy && rect.width() > MIN_SPINNER_WIDTH {
            draw_spinner(
                scene,
                &model.activity,
                (rect.x1 - 14.0, rect.y0 + 14.0),
                theme.muted.with_alpha(phase as f32),
                scale,
                now,
            );
        }

        // The session's own conversation, inside its tile. Drawn after the
        // fill and border so it sits on the card, and before the label so a
        // name is never overprinted by the text it describes.
        draw_card_thumbnail(scene, text, model, card, rect, phase, scale);

        // The label goes inside the card, centred. It is scaled to what the
        // card can actually hold instead of elided into ellipses: "m..." on
        // every card is strictly worse than a small "mushroom", because the
        // name is the only thing distinguishing one session from the next.
        // Clamped so it never becomes unreadable, and a card too narrow even
        // for that carries no label at all rather than a row of dots.
        let name = crate::overview::short_id(&card.session_id);
        // Whether the tile above the name is carrying a preview. Recomputed
        // from the same conditions the thumbnail draws under, so the label can
        // never sit in a band the preview has taken.
        let has_thumbnail = rect.width() >= MIN_THUMB_WIDTH
            && rect.height() >= MIN_THUMB_HEIGHT
            && model
                .peeks
                .get(&card.session_id)
                .is_some_and(|tail| !tail.messages().is_empty());
        // Monospace at this size runs about 0.62em per character.
        let fitted = (rect.width() * 0.85 / (name.chars().count().max(1) as f64 * 0.62)) as f32;
        let size = fitted.clamp(CARD_LABEL_MIN, CARD_LABEL_SIZE);
        if fitted >= CARD_LABEL_MIN {
            text.draw_paragraph_scaled(
                scene,
                &name,
                // Centred on an empty card, and moved down into the band the
                // thumbnail leaves when there is a conversation to show: the
                // name has to stay legible, and the preview above it is what
                // it is naming.
                (
                    rect.x0,
                    match has_thumbnail {
                        true => {
                            rect.y0
                                + rect.height() * THUMB_BAND
                                + (rect.height() * (1.0 - THUMB_BAND) - f64::from(size)) / 2.0
                        }
                        false => (rect.y0 + rect.y1) / 2.0 - f64::from(size) * 0.6,
                    },
                ),
                rect.width() as f32,
                ParagraphStyle {
                    font_size: size,
                    color: if card.focused {
                        theme.text
                    } else {
                        theme.muted
                    }
                    .with_alpha(phase as f32),
                    align: text::Align::Center,
                    line_height: 1.1,
                    ..Default::default()
                },
                scale,
            );
        }
    }

    // One line of instruction at the very foot of the page, only while the
    // field is settled: during the zoom it would be text arriving and leaving
    // in 140ms. Pinned to the bottom margin rather than to the composer's
    // caption row, which sits in the middle of the field and would put the
    // hint straight through a card.
    if phase > 0.85 {
        let hint_top = frame.height - layout::FOOTNOTE_HEIGHT * 1.5;
        text.draw_paragraph_scaled(
            scene,
            "arrows or hjkl to move   release super to switch   esc to stay",
            (frame.left, hint_top),
            frame.column() as f32,
            ParagraphStyle {
                font_size: layout::CAPTION_SIZE,
                color: theme.faint,
                align: text::Align::Center,
                letter_spacing_em: 0.1,
                ..Default::default()
            },
            scale,
        );
    }
}
