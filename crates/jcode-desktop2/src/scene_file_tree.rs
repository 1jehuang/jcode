//! Rendering for the persistent project explorer.

use crate::{Model, file_tree, paint, text::ParagraphStyle};
use vello::Scene;
use vello::kurbo::{Affine, BezPath, Line, Rect};

pub fn draw(
    scene: &mut Scene,
    painter: &mut paint::Painter,
    model: &Model,
    size: (u32, u32),
    scale: f64,
) {
    if model.file_tree.root().is_none() {
        return;
    }
    let width = file_tree::WIDTH.min(f64::from(size.0) / scale);
    let height = f64::from(size.1) / scale;
    scene.fill(
        vello::peniko::Fill::NonZero,
        Affine::scale(scale),
        model.theme.background,
        None,
        &Rect::new(0.0, 0.0, width, height),
    );
    scene.stroke(
        &vello::kurbo::Stroke::new(1.0 / scale),
        Affine::scale(scale),
        model.theme.rule,
        None,
        &Line::new((width, 0.0), (width, height)),
    );

    painter.text.draw_paragraph_scaled(
        scene,
        "EXPLORER",
        (16.0, 12.0),
        (width - 32.0) as f32,
        ParagraphStyle {
            font_size: 10.0,
            line_height: 1.0,
            color: model.theme.muted,
            ..Default::default()
        },
        scale,
    );
    painter.text.draw_paragraph_scaled(
        scene,
        model.file_tree.root_label(),
        (16.0, 32.0),
        (width - 32.0) as f32,
        ParagraphStyle {
            font_size: 12.0,
            line_height: 1.0,
            color: model.theme.text,
            ..Default::default()
        },
        scale,
    );

    let max_rows = ((height - file_tree::HEADER_HEIGHT) / file_tree::ROW_HEIGHT)
        .floor()
        .max(0.0) as usize;
    for (index, (entry, depth)) in model.file_tree.visible(max_rows).into_iter().enumerate() {
        let y = file_tree::HEADER_HEIGHT + index as f64 * file_tree::ROW_HEIGHT;
        let x = 15.0 + depth as f64 * file_tree::INDENT;
        if entry.directory {
            let mut caret = BezPath::new();
            if model.file_tree.is_expanded(&entry.path) {
                caret.move_to((x, y + 9.0));
                caret.line_to((x + 8.0, y + 9.0));
                caret.line_to((x + 4.0, y + 14.0));
            } else {
                caret.move_to((x + 1.0, y + 7.0));
                caret.line_to((x + 7.0, y + 11.0));
                caret.line_to((x + 1.0, y + 15.0));
            }
            caret.close_path();
            scene.fill(
                vello::peniko::Fill::NonZero,
                Affine::scale(scale),
                model.theme.muted,
                None,
                &caret,
            );
        }
        let label_x = x + 14.0;
        let available = (width - label_x - 8.0).max(1.0) as f32;
        painter.text.draw_paragraph_scaled(
            scene,
            &entry.name,
            (label_x, y + 3.0),
            available,
            ParagraphStyle {
                font_size: 12.0,
                line_height: 1.0,
                color: if entry.directory {
                    model.theme.text
                } else {
                    model.theme.muted
                },
                ..Default::default()
            },
            scale,
        );
    }
}
