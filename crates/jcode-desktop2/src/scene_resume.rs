//! The resume overlay: stored sessions on the left, the hovered one previewed
//! on the right, the current conversation still legible behind both.
//!
//! Split from [`crate::scene`] for the same reason [`crate::scene_overview`]
//! is: this is a mode drawn *over* the page, with its own layering rules and
//! none of the transcript's machinery. `build_scene` calls [`draw_resume`]
//! last so the card washes whatever it covers.
//!
//! The veil is deliberately light. A picker exists to answer "which of these
//! do I want", and the strongest cue for that is the session you are in right
//! now: seeing it behind the panel is what makes the choice a comparison
//! rather than a memory test.

use crate::scene::elide;
use crate::text::ParagraphStyle;
use crate::{Model, layout, resume, text};
use vello::Scene;
use vello::kurbo::{Affine, Rect, RoundedRect};

/// How far the page is dimmed behind the overlay. Lighter than the overview's
/// veil: the overview replaces the page, while this one sits on it.
const VEIL_OPACITY: f64 = 0.42;
/// Opacity of the card itself over that veil.
///
/// All but opaque: the conversation shows *around* the card, not through it.
/// A translucent card let the page's own words run behind the session rows,
/// which is the one thing a list of names cannot survive.
const CARD_OPACITY: f32 = 0.995;
/// Indent of a session row under its project heading.
const ROW_INDENT: f64 = 14.0;
/// Leading for the preview's lines.
const PREVIEW_LEADING: f32 = 1.55;

/// Draw the picker, if it is open.
pub fn draw_resume(
    scene: &mut Scene,
    text: &mut text::TextSystem,
    model: &Model,
    frame: &layout::Frame,
    scale: f64,
) {
    if !model.resume.is_open() {
        return;
    }
    let theme = &model.theme;
    // The veil: the whole window, so the overlay's own inset margin is dimmed
    // page rather than a bright frame around a dark card.
    scene.fill(
        vello::peniko::Fill::NonZero,
        Affine::scale(scale),
        theme.background.with_alpha(VEIL_OPACITY as f32),
        None,
        &Rect::new(0.0, 0.0, frame.width, frame.height),
    );

    let rows = model.resume.rows();
    let card = frame.resume_card_for(rows.len());
    scene.fill(
        vello::peniko::Fill::NonZero,
        Affine::scale(scale),
        theme.field.with_alpha(CARD_OPACITY),
        None,
        &RoundedRect::from_rect(card, layout::RESUME_RADIUS),
    );
    scene.stroke(
        &vello::kurbo::Stroke::new(layout::COMPOSER_BORDER),
        Affine::scale(scale),
        theme.field_border,
        None,
        &RoundedRect::from_rect(card, layout::RESUME_RADIUS),
    );

    // A hairline between the list and the preview, so the two columns read as
    // two things: without it the preview's first line looks like a very long
    // session row.
    if let Some(preview) = frame.resume_preview_for(rows.len()) {
        let x = (frame.resume_panel_for(rows.len()).x1 + preview.x0) / 2.0;
        scene.fill(
            vello::peniko::Fill::NonZero,
            Affine::scale(scale),
            theme.rule,
            None,
            &Rect::new(
                x,
                card.y0 + layout::RESUME_PAD,
                x + 1.0,
                card.y1 - layout::RESUME_PAD,
            ),
        );
    }

    draw_search(scene, text, model, frame, scale);
    draw_list(scene, text, model, &rows, frame, scale);
    draw_preview(scene, text, model, frame, scale);
}

/// The search field: what the user has typed, or the instruction when empty.
///
/// A caption rather than a real text well: the overlay owns the keyboard while
/// it is up, so there is nowhere else typing could go and a second border
/// would only add furniture.
fn draw_search(
    scene: &mut Scene,
    text: &mut text::TextSystem,
    model: &Model,
    frame: &layout::Frame,
    scale: f64,
) {
    let theme = &model.theme;
    let box_ = frame.resume_search_for(model.resume.rows().len());
    let query = model.resume.query();
    let (label, color) = match query.is_empty() {
        true => (
            match model.resume.is_scanning() {
                true => "reading sessions...".to_string(),
                false => "type to search · enter resumes · esc closes".to_string(),
            },
            theme.faint,
        ),
        false => (format!("{query}\u{2502}"), theme.text),
    };
    text.draw_paragraph_scaled(
        scene,
        &label,
        (box_.x0, box_.y0 + 2.0),
        (box_.width().max(1.0)) as f32,
        ParagraphStyle {
            font_size: layout::RESUME_ROW_SIZE,
            color,
            ..Default::default()
        },
        scale,
    );
    // A hairline under the search line, which is what separates it from the
    // list without spending a whole row of padding on the gap.
    scene.fill(
        vello::peniko::Fill::NonZero,
        Affine::scale(scale),
        theme.rule,
        None,
        &Rect::new(box_.x0, box_.y1 - 1.0, box_.x1, box_.y1),
    );
}

/// Which slice of the rows is on screen, given where the highlight sits.
///
/// Kept as a function of (cursor, total, visible) so it is testable and so the
/// pointer and the renderer scroll identically: a click that lands on a
/// different row than the one drawn is the bug this prevents.
pub fn window_start(cursor: usize, total: usize, visible: usize) -> usize {
    if total <= visible {
        return 0;
    }
    // Keep the highlight off the very edge where there is list to spare, so
    // the user can see what they are moving towards.
    let margin = (visible / 4).min(3);
    let ideal = cursor.saturating_sub(margin);
    ideal.min(total - visible)
}

fn draw_list(
    scene: &mut Scene,
    text: &mut text::TextSystem,
    model: &Model,
    rows: &[resume::Row],
    frame: &layout::Frame,
    scale: f64,
) {
    let theme = &model.theme;
    let visible = frame.resume_visible_rows_for(rows.len());
    if rows.is_empty() {
        let list = frame.resume_list_for(rows.len());
        let message = match model.resume.is_scanning() {
            true => "reading sessions...",
            false => "no stored sessions match",
        };
        text.draw_paragraph_scaled(
            scene,
            message,
            (list.x0, list.y0 + 4.0),
            (list.width().max(1.0)) as f32,
            ParagraphStyle {
                font_size: layout::RESUME_ROW_SIZE,
                color: theme.faint,
                ..Default::default()
            },
            scale,
        );
        return;
    }
    let start = window_start(model.resume.cursor(), rows.len(), visible);
    for (slot, row) in rows.iter().skip(start).take(visible).enumerate() {
        let band = frame.resume_row_for(rows.len(), slot);
        let selected = start + slot == model.resume.cursor();
        if selected {
            scene.fill(
                vello::peniko::Fill::NonZero,
                Affine::scale(scale),
                theme.wash,
                None,
                &RoundedRect::from_rect(band, layout::RESUME_RADIUS / 2.0),
            );
        }
        match row {
            resume::Row::Group {
                label,
                count,
                expanded,
                ..
            } => {
                // The disclosure marker carries the state, so a collapsed
                // project is distinguishable without reading its count.
                let marker = if *expanded { "\u{25be}" } else { "\u{25b8}" };
                let width = (band.width() - ROW_INDENT).max(1.0) as f32;
                text.draw_paragraph_scaled(
                    scene,
                    &format!("{marker} {label}"),
                    (band.x0, band.y0 + 4.0),
                    width,
                    ParagraphStyle {
                        font_size: layout::RESUME_GROUP_SIZE,
                        color: theme.muted,
                        bold: true,
                        letter_spacing_em: 0.06,
                        ..Default::default()
                    },
                    scale,
                );
                text.draw_paragraph_scaled(
                    scene,
                    &count.to_string(),
                    (band.x0, band.y0 + 4.0),
                    (band.width().max(1.0)) as f32,
                    ParagraphStyle {
                        font_size: layout::RESUME_META_SIZE,
                        color: theme.faint,
                        align: text::Align::End,
                        ..Default::default()
                    },
                    scale,
                );
            }
            resume::Row::Session { index } => {
                let Some(record) = model.resume.records().get(*index) else {
                    continue;
                };
                let attached = model.session_id.as_deref() == Some(record.session_id.as_str());
                let width = (band.width() - ROW_INDENT).max(1.0) as f32;
                // The session you are in is marked rather than hidden: it has
                // to be visible for the list to make sense as a whole, and
                // resuming it is a no-op the user should not be offered blind.
                let label = match attached {
                    true => format!("{} \u{00b7} here", record.label()),
                    false => record.label(),
                };
                let budget =
                    (f64::from(width) / (f64::from(layout::RESUME_ROW_SIZE) * 0.58)) as usize;
                text.draw_paragraph_scaled(
                    scene,
                    &elide(&label, budget.max(8)),
                    (band.x0 + ROW_INDENT, band.y0 + 4.0),
                    width,
                    ParagraphStyle {
                        font_size: layout::RESUME_ROW_SIZE,
                        color: if selected { theme.text } else { theme.muted },
                        ..Default::default()
                    },
                    scale,
                );
                text.draw_paragraph_scaled(
                    scene,
                    &resume::human_bytes(record.bytes),
                    (band.x0, band.y0 + 5.0),
                    (band.width().max(1.0)) as f32,
                    ParagraphStyle {
                        font_size: layout::RESUME_META_SIZE,
                        color: theme.faint,
                        align: text::Align::End,
                        ..Default::default()
                    },
                    scale,
                );
            }
        }
    }
}

/// The highlighted session's conversation, on the right.
///
/// The whole reason the panel is a panel: a list of names is only navigable,
/// while a list plus the words in the session under the cursor is *choosable*.
/// Fetched through the same peek path the overview uses, so a stored session
/// and a live one preview identically.
fn draw_preview(
    scene: &mut Scene,
    text: &mut text::TextSystem,
    model: &Model,
    frame: &layout::Frame,
    scale: f64,
) {
    let Some(region) = frame.resume_preview_for(model.resume.rows().len()) else {
        return;
    };
    let theme = &model.theme;
    let Some(record) = model.resume.selected() else {
        // A project heading is highlighted: name what is under it rather than
        // leaving the column blank, which reads as a failed fetch.
        text.draw_paragraph_scaled(
            scene,
            "a project · open it to preview its sessions",
            (region.x0, region.y0),
            (region.width().max(1.0)) as f32,
            ParagraphStyle {
                font_size: layout::RESUME_ROW_SIZE,
                color: theme.faint,
                ..Default::default()
            },
            scale,
        );
        return;
    };

    // Header: the session, then the project it ran in. The directory is the
    // fact that decides whether a conversation is the one you want.
    let mut y = region.y0;
    y += text.draw_paragraph_scaled(
        scene,
        &record.label(),
        (region.x0, y),
        (region.width().max(1.0)) as f32,
        ParagraphStyle {
            font_size: layout::RESUME_ROW_SIZE + 1.5,
            color: theme.text,
            bold: true,
            ..Default::default()
        },
        scale,
    );
    let dir = record.working_dir.as_deref().unwrap_or("unknown project");
    y += text.draw_paragraph_scaled(
        scene,
        &format!("{dir} · {}", resume::human_bytes(record.bytes)),
        (region.x0, y + 2.0),
        (region.width().max(1.0)) as f32,
        ParagraphStyle {
            font_size: layout::RESUME_META_SIZE,
            color: theme.faint,
            ..Default::default()
        },
        scale,
    ) + 8.0;

    let Some(transcript) = model.peeks.get(&record.session_id) else {
        text.draw_paragraph_scaled(
            scene,
            "reading this conversation...",
            (region.x0, y),
            (region.width().max(1.0)) as f32,
            ParagraphStyle {
                font_size: layout::RESUME_ROW_SIZE,
                color: theme.faint,
                ..Default::default()
            },
            scale,
        );
        return;
    };

    // Oldest of the tail first, so the preview reads in conversation order,
    // and wrapped rather than elided: this column is wide enough to read, and
    // this is the text the choice is actually made on.
    for message in transcript.messages() {
        if y >= region.y1 - f64::from(layout::RESUME_ROW_SIZE) {
            break;
        }
        let source = message.source.trim();
        if source.is_empty() {
            continue;
        }
        let user = message.role == crate::transcript::Role::User;
        y += text.draw_paragraph_scaled(
            scene,
            source,
            (region.x0, y),
            (region.width().max(1.0)) as f32,
            ParagraphStyle {
                font_size: layout::RESUME_ROW_SIZE,
                // The alternation is the only structure kept: it is what makes
                // the block legible as a conversation rather than as prose.
                color: if user { theme.text } else { theme.muted },
                line_height: PREVIEW_LEADING,
                ..Default::default()
            },
            scale,
        ) + 6.0;
    }
}

#[cfg(test)]
mod tests {
    /// The window must always contain the highlight, whichever end of a long
    /// list it is at: a cursor drawn off the panel is a selection the user
    /// cannot see.
    #[test]
    fn the_window_always_contains_the_cursor() {
        for total in [0usize, 1, 5, 20, 400] {
            for visible in [1usize, 3, 12] {
                for cursor in 0..total.max(1) {
                    let start = super::window_start(cursor, total, visible);
                    assert!(start <= cursor, "window started past the cursor");
                    assert!(
                        cursor < start + visible,
                        "cursor {cursor} outside window {start}..{}",
                        start + visible
                    );
                    if total > visible {
                        assert!(
                            start + visible <= total,
                            "window ran past the end of the list"
                        );
                    }
                }
            }
        }
    }

    /// A list that fits is never scrolled: the first row stays at the top
    /// rather than drifting as the highlight moves.
    #[test]
    fn a_short_list_does_not_scroll() {
        for cursor in 0..5 {
            assert_eq!(super::window_start(cursor, 5, 10), 0);
        }
    }
}
