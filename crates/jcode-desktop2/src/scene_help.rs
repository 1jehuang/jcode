//! Rendering for Desktop2's local, keyboard-modal help card.

use crate::help;
use crate::text::ParagraphStyle;
use crate::{Model, layout, text};
use vello::Scene;
use vello::kurbo::{Affine, Rect, RoundedRect};

const VEIL_OPACITY: f32 = 0.18;
const CARD_OPACITY: f32 = 0.995;

pub fn draw_help(
    scene: &mut Scene,
    text: &mut text::TextSystem,
    model: &Model,
    frame: &layout::Frame,
    scale: f64,
) {
    if !model.help_open {
        return;
    }

    let theme = &model.theme;
    let geometry = help::Layout::new(frame.width, frame.height);
    scene.fill(
        vello::peniko::Fill::NonZero,
        Affine::scale(scale),
        theme.text.with_alpha(VEIL_OPACITY),
        None,
        &Rect::new(0.0, 0.0, frame.width, frame.height),
    );
    let card = RoundedRect::from_rect(geometry.card, help::CARD_RADIUS);
    scene.fill(
        vello::peniko::Fill::NonZero,
        Affine::scale(scale),
        theme.field.with_alpha(CARD_OPACITY),
        None,
        &card,
    );
    scene.stroke(
        &vello::kurbo::Stroke::new(layout::COMPOSER_BORDER),
        Affine::scale(scale),
        theme.field_border,
        None,
        &card,
    );

    let title_left = geometry.card.x0 + help::CARD_PAD;
    let title_width = (geometry.card.width() - help::CARD_PAD * 2.0).max(1.0) as f32;
    text.draw_paragraph_scaled(
        scene,
        "Desktop2 help",
        (title_left, geometry.card.y0 + help::CARD_PAD - 3.0),
        title_width,
        ParagraphStyle {
            font_size: 18.0,
            color: theme.text,
            bold: true,
            line_height: 1.2,
            ..Default::default()
        },
        scale,
    );
    text.draw_paragraph_scaled(
        scene,
        "F1 or Escape closes · slash commands below stay local",
        (title_left, geometry.card.y0 + help::CARD_PAD + 27.0),
        title_width,
        ParagraphStyle {
            font_size: layout::CAPTION_SIZE,
            color: theme.muted,
            line_height: 1.2,
            ..Default::default()
        },
        scale,
    );

    // The body is explicitly clipped. On a short window the card remains
    // centred and usable instead of letting its final rows paint through the
    // rounded edge or over the page beneath it.
    scene.push_layer(
        vello::peniko::Fill::NonZero,
        vello::peniko::Mix::Normal,
        1.0,
        Affine::scale(scale),
        &geometry.body,
    );
    for column_index in 0..geometry.columns {
        let column = geometry.column(column_index);
        let mut y = column.y0;
        for section in help::sections(geometry.columns, column_index) {
            text.draw_paragraph_scaled(
                scene,
                section.title,
                (column.x0, y),
                column.width().max(1.0) as f32,
                ParagraphStyle {
                    font_size: 9.5,
                    color: theme.faint,
                    bold: true,
                    letter_spacing_em: 0.12,
                    line_height: 1.2,
                    ..Default::default()
                },
                scale,
            );
            y += help::SECTION_HEADING_HEIGHT;
            let key_width = (column.width() * 0.44).clamp(110.0, 168.0);
            for row in section.rows {
                text.draw_paragraph_scaled(
                    scene,
                    row.key,
                    (column.x0, y),
                    key_width.max(1.0) as f32,
                    ParagraphStyle {
                        font_size: 10.5,
                        color: theme.text,
                        bold: true,
                        line_height: 1.2,
                        ..Default::default()
                    },
                    scale,
                );
                let description_left = column.x0 + key_width + 10.0;
                text.draw_paragraph_scaled(
                    scene,
                    row.description,
                    (description_left, y),
                    (column.x1 - description_left).max(1.0) as f32,
                    ParagraphStyle {
                        font_size: 10.5,
                        color: theme.muted,
                        line_height: 1.2,
                        ..Default::default()
                    },
                    scale,
                );
                y += help::ROW_HEIGHT;
            }
            y += 11.0;
        }
    }
    scene.pop_layer();
}
