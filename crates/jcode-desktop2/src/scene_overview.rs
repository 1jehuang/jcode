//! The session overview layer: the card strip, its preview, and its hint.
//!
//! Split from [`crate::scene`] because the overview is a mode drawn over the
//! page rather than part of it: it has its own constants, its own layering
//! rules, and none of the transcript's machinery. `build_scene` calls
//! [`draw_overview`] last, which is what lets the field wash the page it
//! replaces.

use crate::scene::{draw_spinner, elide};
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
const VEIL_OPACITY: f64 = 0.82;
/// Type size, leading, and ink for the hovered session's preview. Small and
/// faint: it is the page *behind* the decision, not the decision.
const PREVIEW_SIZE: f32 = 11.0;
const PREVIEW_LEADING: f64 = 1.7;
const PREVIEW_OPACITY: f64 = 0.72;
/// Fraction of the window height the preview may occupy, measured from the
/// top. Bounded so it can never reach the rows, whichever session is hovered.
const PREVIEW_BAND: f64 = 0.22;
/// Smallest card that carries a busy spinner. Below this the spinner would be
/// larger than the session it belongs to.
const MIN_SPINNER_WIDTH: f64 = 44.0;

/// Draw the highlighted session's conversation behind the field.
///
/// The cards say how big each session is and what it is called, which is
/// enough to *navigate* and not enough to *choose*: "clover" and "pebble" are
/// only names until you can see what is in them. Hovering a card puts that
/// session's last exchanges on the page underneath, so picking is recognition
/// rather than recall.
///
/// Set faint and behind the veil on purpose: this is context for a decision
/// being made in the foreground, and a preview that competed with the cards
/// would make the field unreadable at exactly the moment it is being used.
fn draw_preview(
    scene: &mut Scene,
    text: &mut text::TextSystem,
    model: &Model,
    frame: &layout::Frame,
    scale: f64,
    phase: f64,
) {
    let Some(focus) = model.overview.focus() else {
        return;
    };
    // The session we are attached to is already on the page underneath, so
    // previewing it would draw the same conversation twice.
    if model.session_id.as_deref() == Some(focus) {
        return;
    }
    let Some(transcript) = model.peeks.get(focus) else {
        return;
    };

    // Top-down from the head of the page, oldest of the tail first, so the
    // preview reads in conversation order. It lives at the top because that is
    // the band the rows leave clear, and because the foot already carries the
    // hint.
    let mut y = frame.body_top;
    let width = frame.column() as f32;
    let ceiling = frame.height * PREVIEW_BAND;
    for message in transcript.messages() {
        if y >= ceiling {
            break;
        }
        let source = message.source.trim();
        if source.is_empty() {
            continue;
        }
        // One line per message: the preview is a shape to recognise, not a
        // transcript to read, and a wrapped paragraph would push the older
        // exchanges (the ones that identify the session) off the page.
        let budget = (frame.column() / (f64::from(PREVIEW_SIZE) * 0.6)) as usize;
        let line = elide(&source.replace('\n', " "), budget.max(16));
        text.draw_paragraph_scaled(
            scene,
            &line,
            (frame.left, y),
            width,
            ParagraphStyle {
                font_size: PREVIEW_SIZE,
                // A user's line is set darker than a reply, the only structure
                // the preview keeps: it is what makes the alternation legible
                // as a conversation rather than as a paragraph of noise.
                color: if message.role == crate::transcript::Role::User {
                    model.theme.muted
                } else {
                    model.theme.faint
                }
                .with_alpha((PREVIEW_OPACITY * phase) as f32),
                line_height: PREVIEW_LEADING as f32,
                ..Default::default()
            },
            scale,
        );
        y += f64::from(PREVIEW_SIZE) * PREVIEW_LEADING;
    }
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
        &model.strip.entries(),
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
    // Just opaque enough that the cards and their labels win the foreground,
    // and no more. A full cover made the gesture feel like navigating away.
    let veil = (VEIL_OPACITY * phase) as f32;
    scene.fill(
        vello::peniko::Fill::NonZero,
        Affine::scale(scale),
        theme.background.with_alpha(veil),
        None,
        &Rect::new(0.0, 0.0, frame.width, frame.height),
    );

    // The hovered session's own conversation, on the page the veil just
    // cleared: drawn before the cards so it is unambiguously behind them.
    draw_preview(scene, text, model, frame, scale, phase);

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

        // The label goes inside the card, centred. It is scaled to what the
        // card can actually hold instead of elided into ellipses: "m..." on
        // every card is strictly worse than a small "mushroom", because the
        // name is the only thing distinguishing one session from the next.
        // Clamped so it never becomes unreadable, and a card too narrow even
        // for that carries no label at all rather than a row of dots.
        let name = crate::overview::short_id(&card.session_id);
        // Monospace at this size runs about 0.62em per character.
        let fitted = (rect.width() * 0.85 / (name.chars().count().max(1) as f64 * 0.62)) as f32;
        let size = fitted.clamp(CARD_LABEL_MIN, CARD_LABEL_SIZE);
        if fitted >= CARD_LABEL_MIN {
            text.draw_paragraph_scaled(
                scene,
                &name,
                (rect.x0, (rect.y0 + rect.y1) / 2.0 - f64::from(size) * 0.6),
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
