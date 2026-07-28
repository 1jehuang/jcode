//! The session overview layer: the blob field, its preview, and its hint.
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
use vello::kurbo::{Affine, Circle, Rect};

/// Size of a blob's session label, and of the cluster's name above it.
const BLOB_LABEL_SIZE: f32 = 11.0;
/// Smallest a blob's name may be set before it is dropped entirely: below this
/// it is illegible, and illegible text is noise rather than a label.
const BLOB_LABEL_MIN: f32 = 7.0;
const CLUSTER_LABEL_SIZE: f32 = 13.0;
/// Gap between the bottom of a cluster's blobs and its name.
const CLUSTER_LABEL_GAP: f64 = 8.0;
/// Ring thickness for an unfocused blob, and for the focused one.
const BLOB_RING: f64 = 1.25;
const BLOB_RING_FOCUS: f64 = 2.5;
/// How far past its radius the focused blob's halo reaches.
const BLOB_HALO: f64 = 7.0;
/// How much a busy blob's ring breathes, as a fraction of its radius.
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
/// top. Bounded so it can never reach the cluster names and the hint at the
/// foot, whichever session is hovered.
const PREVIEW_BAND: f64 = 0.3;
/// Smallest blob that carries a busy spinner. Below this the spinner would be
/// larger than the session it belongs to.
const MIN_SPINNER_RADIUS: f64 = 22.0;

/// Draw the highlighted session's conversation behind the field.
///
/// The blobs say how big each session is and what it is called, which is
/// enough to *navigate* and not enough to *choose*: "clover" and "pebble" are
/// only names until you can see what is in them. Hovering a blob puts that
/// session's last exchanges on the page underneath, so picking is recognition
/// rather than recall.
///
/// Set faint and behind the veil on purpose: this is context for a decision
/// being made in the foreground, and a preview that competed with the blobs
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
    // where the field is emptiest (the packing centres on the current session)
    // and because the foot already carries the cluster names and the hint.
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

/// Draw the session overview: every live session as a blob in a 2D field.
///
/// The field fades and scales in together, from the focused blob's position
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
    if field.blobs.is_empty() {
        return;
    }

    // Veil the page rather than replace it. The conversation stays visible
    // underneath, so the field reads as a layer over the work you were doing
    // instead of as a different screen you have been taken to: you never lose
    // your place, and the switch is a glance rather than a context change.
    //
    // Just opaque enough that the blobs and their labels win the foreground,
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
    // cleared: drawn before the blobs so it is unambiguously behind them.
    draw_preview(scene, text, model, frame, scale, phase);

    // Everything flies out from the blob you came from, so the session on
    // screen stays under the eye through the whole transition.
    let origin = field
        .blobs
        .iter()
        .find(|blob| blob.current)
        .or_else(|| field.focused())
        .map(|blob| blob.center)
        .unwrap_or((frame.width / 2.0, frame.height / 2.0));
    let place = |point: (f64, f64)| {
        (
            origin.0 + (point.0 - origin.0) * phase,
            origin.1 + (point.1 - origin.1) * phase,
        )
    };

    // A project's name is anchored to the bottom of its cluster's bounding
    // circle, clear of every blob in it. Hanging it off the centroid put it
    // inside the group whenever the blobs were not evenly spread, which is
    // most of the time and all of the time in a crowded field.
    for cluster in &field.clusters {
        let center = place(cluster.center);
        // Clamped into the field, so a cluster sitting at the bottom edge
        // still gets a name: a project whose label silently fell off the page
        // is the one case where the field lies about what it contains.
        let (_, _, _, area_bottom) = crate::overview::area(frame);
        let top = (center.1 + cluster.radius * phase + CLUSTER_LABEL_GAP)
            .min(area_bottom - f64::from(CLUSTER_LABEL_SIZE))
            // Never into the preview's band at the head of the page: the two
            // are both faint small type, so overlapping them makes each
            // unreadable rather than merely crowded.
            .max(frame.height * PREVIEW_BAND);
        text.draw_paragraph_scaled(
            scene,
            &cluster.label,
            (center.0 - 120.0, top),
            240.0,
            ParagraphStyle {
                font_size: CLUSTER_LABEL_SIZE,
                color: theme.faint.with_alpha(phase as f32),
                align: text::Align::Center,
                letter_spacing_em: 0.14,
                line_height: 1.0,
                ..Default::default()
            },
            scale,
        );
    }

    for blob in &field.blobs {
        let center = place(blob.center);
        // A busy session breathes, so work happening in a conversation you are
        // not looking at is visible from across the field. The pulse runs on
        // the activity's clock, which pinned captures freeze.
        let pulse = if blob.busy {
            1.0 + BUSY_PULSE * crate::overview::breath(model.activity.elapsed(now), BUSY_PERIOD)
        } else {
            1.0
        };
        let radius = blob.radius * phase * pulse;
        if radius <= 1.0 {
            continue;
        }
        let circle = Circle::new(center, radius);

        // The focused blob carries a halo, so the highlight survives being
        // next to a much bigger neighbour: a ring alone reads as "big", while
        // a halo reads as "chosen".
        if blob.focused {
            scene.fill(
                vello::peniko::Fill::NonZero,
                Affine::scale(scale),
                theme.wash.with_alpha(phase as f32),
                None,
                &Circle::new(center, radius + BLOB_HALO),
            );
        }
        // Fill: the session you are in is inked, the rest are paper, so "where
        // am I" is answered before any label is read.
        scene.fill(
            vello::peniko::Fill::NonZero,
            Affine::scale(scale),
            if blob.current {
                theme.wash.with_alpha(phase as f32)
            } else {
                theme.background.with_alpha(phase as f32)
            },
            None,
            &circle,
        );
        // Only the highlight gets a heavy ring: a thick ring on a busy blob was
        // indistinguishable from the focused one, so a field with work running
        // in it appeared to have two selections.
        scene.stroke(
            &vello::kurbo::Stroke::new(if blob.focused {
                BLOB_RING_FOCUS
            } else {
                BLOB_RING
            }),
            Affine::scale(scale),
            if blob.focused { theme.text } else { theme.rule }.with_alpha(phase as f32),
            None,
            &circle,
        );
        // Work is signalled by a mark rather than by the ring's weight: a
        // spinner in the blob's shoulder, the same halftone comet the composer
        // uses, so "this session is working" looks the same everywhere in the
        // app and cannot be confused with "this session is selected".
        if blob.busy && radius > MIN_SPINNER_RADIUS {
            draw_spinner(
                scene,
                &model.activity,
                (center.0 + radius * 0.66, center.1 - radius * 0.66),
                theme.muted.with_alpha(phase as f32),
                scale,
                now,
            );
        }

        // The label goes inside the blob, centred: a caption hung underneath
        // would collide with the neighbour below it as soon as the field is
        // dense, which is exactly when the labels matter. It is elided to what
        // the circle can actually hold, so a long name on a small session is
        // shortened rather than drawn out over the paper on both sides.
        // Scale the type to the circle instead of eliding a short name into
        // ellipses: "m..." on every blob is strictly worse than a small
        // "mushroom", because the name is the only thing distinguishing one
        // session from the next. Clamped so it never becomes unreadable, and
        // a blob too small even for that carries no label at all rather than
        // a row of dots.
        let name = crate::overview::short_id(&blob.session_id);
        // Monospace at this size runs about 0.62em per character.
        let fitted = (radius * 1.7 / (name.chars().count().max(1) as f64 * 0.62)) as f32;
        let size = fitted.clamp(BLOB_LABEL_MIN, BLOB_LABEL_SIZE);
        let label_width = radius * 1.9;
        if fitted >= BLOB_LABEL_MIN {
            text.draw_paragraph_scaled(
                scene,
                &name,
                (
                    center.0 - label_width / 2.0,
                    center.1 - f64::from(size) * 0.6,
                ),
                label_width as f32,
                ParagraphStyle {
                    font_size: size,
                    color: if blob.focused {
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
    // hint straight through a blob.
    if phase > 0.85 {
        let hint_top = frame.height - layout::FOOTNOTE_HEIGHT * 1.5;
        text.draw_paragraph_scaled(
            scene,
            "arrows or hjkl to move   release alt to switch   esc to stay",
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
