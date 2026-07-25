//! Frame construction: the pure `Model` -> `Scene` function.
//!
//! Kept separate from the event loop so a frame is a pure function of the
//! model, which is what makes the state-space captures and pixel tests
//! possible.

use crate::text::ParagraphStyle;
use crate::{Model, donut, layout, text};
use vello::Scene;
use vello::kurbo::{Affine, Circle, Rect, RoundedRect};
use vello::peniko::Color;

/// Halftone dots across the donut box, matching the website's screen density.
const HALFTONE_CELLS: f64 = 76.0;
/// Classic 45-degree halftone screen angle.
const SCREEN_ANGLE: f64 = std::f64::consts::FRAC_PI_4;
/// Dot radius as a fraction of the dot pitch at full luminance.
const DOT_FILL: f64 = 0.62;
/// Luminance below which a dot is not worth drawing.
const DOT_FLOOR: f32 = 0.04;
/// Gamma applied to luminance before sizing a dot.
const DOT_GAMMA: f32 = 0.85;

/// Draw the halftone donut into `box_`, sampling `field` as a luminance image.
/// The dot lattice is in logical units so the screen density is identical on
/// 1x and HiDPI, exactly like the website's CSS-pixel lattice.
fn draw_donut(
    scene: &mut Scene,
    field: &donut::Donut,
    box_: Rect,
    ink: Color,
    scale: f64,
) {
    let side = box_.width().min(box_.height());
    if side < layout::DONUT_MIN_SIDE {
        return;
    }
    let pitch = side / HALFTONE_CELLS;
    let (sin_a, cos_a) = SCREEN_ANGLE.sin_cos();
    let cx = box_.x0 + box_.width() / 2.0;
    let cy = box_.y0 + box_.height() / 2.0;
    // A rotated lattice must cover the square, so extend it by sqrt(2).
    let ext = (HALFTONE_CELLS * std::f64::consts::FRAC_1_SQRT_2).ceil() as i32 + 1;
    let grid = field.grid() as f32;

    for j in -ext..=ext {
        for i in -ext..=ext {
            let px = cx + (i as f64 * cos_a - j as f64 * sin_a) * pitch;
            let py = cy + (i as f64 * sin_a + j as f64 * cos_a) * pitch;
            // Sample in the donut's own square, centred in the box.
            let u = (px - (cx - side / 2.0)) / side;
            let v = (py - (cy - side / 2.0)) / side;
            if !(-0.02..1.02).contains(&u) || !(-0.02..1.02).contains(&v) {
                continue;
            }
            let lum = field.sample(u as f32 * grid, v as f32 * grid);
            if lum <= DOT_FLOOR {
                continue;
            }
            let radius = f64::from(lum.powf(DOT_GAMMA)) * pitch * DOT_FILL;
            scene.fill(
                vello::peniko::Fill::NonZero,
                Affine::scale(scale),
                ink,
                None,
                &Circle::new((px, py), radius),
            );
        }
    }
}

/// The one style used for composer text. Wrapping, drawing, caret placement,
/// and hit-testing must all use the same style or their geometry diverges, so
/// there is exactly one definition of it.
pub fn composer_text_style(model: &Model) -> ParagraphStyle {
    ParagraphStyle {
        font_size: layout::BODY_SIZE,
        color: model.theme.text,
        line_height: (layout::COMPOSER_LINE_HEIGHT / f64::from(layout::BODY_SIZE)) as f32,
        ..Default::default()
    }
}

/// Build the frame. `size` is the surface size in physical pixels and `scale`
/// is the window scale factor; geometry comes from [`layout::Frame`] in logical
/// units, so the design reads identically on 1x and HiDPI displays.
pub fn build_scene(
    scene: &mut Scene,
    text: &mut text::TextSystem,
    model: &Model,
    size: (u32, u32),
    scale: f64,
) {
    let theme = &model.theme;
    // Size the composer from where the text really wraps, via the same helper
    // the event loop uses, so pointer hit-testing can never see a different
    // frame than the renderer.
    let frame = crate::App::frame_for_model_with(size, scale, model, text);
    let scale = frame.scale;
    let column = frame.column() as f32;

    let fill = |scene: &mut Scene, color: Color, shape: &Rect| {
        scene.fill(
            vello::peniko::Fill::NonZero,
            Affine::scale(scale),
            color,
            None,
            shape,
        );
    };
    let fill_round = |scene: &mut Scene, color: Color, shape: &RoundedRect| {
        scene.fill(
            vello::peniko::Fill::NonZero,
            Affine::scale(scale),
            color,
            None,
            shape,
        );
    };

    // Paper.
    fill(
        scene,
        theme.background,
        &Rect::new(0.0, 0.0, frame.width, frame.height),
    );


    // Composer: a real input field. Paper fill plus a hairline border, rather
    // than a grey slab: a filled block reads as disabled or as a code block,
    // while an outlined field reads as somewhere to type. The border thickens
    // when the window has focus, so focus is legible without a colour accent.
    let well = RoundedRect::new(
        frame.left,
        frame.composer_top,
        frame.right,
        frame.composer_bottom,
        layout::COMPOSER_RADIUS,
    );
    fill_round(scene, theme.field, &well);
    let (border_color, border_width) = if model.focused {
        (theme.field_border_focus, layout::COMPOSER_BORDER_FOCUS)
    } else {
        (theme.field_border, layout::COMPOSER_BORDER)
    };
    scene.stroke(
        &vello::kurbo::Stroke::new(border_width),
        Affine::scale(scale),
        border_color,
        None,
        &well,
    );

    // Transcript: ink on paper, bottom-aligned against the composer so new
    // lines rise from the well rather than dangling from the masthead.
    let placeholder = model.transcript.trim().is_empty();

    // On an empty session the transcript region is dead space, so the hero
    // donut from the website lives there: the same halftone torus, and
    // draggable in the same way. It stands down the moment there is real
    // content, so it can never compete with the transcript.
    if let Some(field) = model.donut.as_ref().filter(|_| placeholder) {
        draw_donut(scene, field, frame.donut_box(), theme.text, scale);
    }

    let transcript = if placeholder {
        "type a message and press enter"
    } else {
        model.transcript.trim_start_matches('\n')
    };
    let body_style = ParagraphStyle {
        font_size: layout::BODY_SIZE,
        color: if placeholder { theme.faint } else { theme.text },
        line_height: layout::BODY_LEADING as f32,
        ..Default::default()
    };
    // Measure the *wrapped* height so long replies never bleed into the well.
    let available = frame.body_bottom - frame.body_top;
    let lines: Vec<&str> = transcript.lines().collect();
    // `scroll` counts lines held back from the tail, so 0 follows live output.
    let end = lines
        .len()
        .saturating_sub(model.scroll)
        .max(1)
        .min(lines.len().max(1));
    let lines = &lines[..end];
    let mut first_line = lines.len().saturating_sub(frame.visible_body_lines());
    let mut tail = lines[first_line..].join("\n");
    let mut tail_height = text.measure_paragraph(&tail, column, body_style, scale);
    while tail_height > available && first_line < lines.len().saturating_sub(1) {
        first_line += 1;
        tail = lines[first_line..].join("\n");
        tail_height = text.measure_paragraph(&tail, column, body_style, scale);
    }
    let origin_y = if placeholder {
        frame.body_top
    } else {
        (frame.body_bottom - tail_height).max(frame.body_top)
    };
    text.draw_paragraph_scaled(
        scene,
        &tail,
        (frame.left, origin_y),
        column,
        body_style,
        scale,
    );

    // Prompt line inside the well: a real input box. The caret is drawn at
    // the measured width of the text before the cursor, so it sits between
    // glyphs and moves with Ctrl+A/E, word motion, and the arrows.
    let prompt_style = composer_text_style(model);
    let prompt_x = frame.composer_text_left();
    let prompt_y = frame.composer_top + layout::COMPOSER_TEXT_OFFSET;
    let prompt_width = frame.composer_text_width() as f32;

    // Prompt marker: a fixed origin for the text, so an empty field still
    // shows where typing starts. It dims while busy instead of the field
    // swapping to a "working..." label, which used to hide anything already
    // typed for the next turn.
    text.draw_paragraph_scaled(
        scene,
        ">",
        (frame.left + layout::COMPOSER_PAD_X, prompt_y),
        layout::COMPOSER_MARKER_ADVANCE as f32,
        ParagraphStyle {
            color: if model.busy { theme.faint } else { theme.muted },
            ..prompt_style
        },
        scale,
    );

    {
        // One Parley layout drives wrapping, the selection bands, the glyphs,
        // and the caret, so the three can never disagree: the highlight lines
        // up with the text because it *is* the text's own geometry.
        let source = model.editor.text();
        let input = crate::input::InputLayout::new(
            text,
            source,
            frame.composer_text_width(),
            prompt_style,
            scale,
        );
        // Scroll the well to the caret's line when the text is taller than the
        // well, so typing never runs out of sight.
        let origin_y =
            prompt_y - input.scroll_offset(model.editor.cursor(), frame.composer_lines());
        let clip_top = frame.composer_top;
        let clip_bottom = frame.composer_bottom;

        // Selection bands, under the glyphs so text on them stays legible.
        if let Some((sel_start, sel_end)) = model.editor.selection() {
            for band in input.selection_rects(sel_start, sel_end) {
                let top = origin_y + band.y0;
                let bottom = origin_y + band.y1;
                if bottom <= clip_top || top >= clip_bottom {
                    continue;
                }
                fill(
                    scene,
                    theme.selection,
                    &Rect::new(
                        (prompt_x + band.x0).min(frame.right),
                        top.max(clip_top),
                        (prompt_x + band.x1).min(frame.right),
                        bottom.min(clip_bottom),
                    ),
                );
            }
        }

        if model.editor.is_empty() {
            text.draw_paragraph_scaled(
                scene,
                if model.busy {
                    "working... esc to interrupt"
                } else {
                    "message jcode"
                },
                (prompt_x, prompt_y),
                prompt_width,
                ParagraphStyle {
                    color: theme.faint,
                    ..prompt_style
                },
                scale,
            );
        } else {
            // Draw the whole layout in one pass: Parley already wrapped it to
            // the well, so per-row drawing would only reintroduce drift.
            crate::text::TextSystem::draw_layout(
                scene,
                input.layout(),
                (prompt_x, origin_y),
                scale,
            );
        }

        // An unfocused window must not show a blinking caret: it would claim
        // keystrokes land here when they do not.
        if model.focused && model.caret.visible() {
            let bar = input.caret_rect(model.editor.cursor(), layout::CARET_WIDTH);
            let top = (origin_y + bar.y0).max(clip_top);
            let bottom = (origin_y + bar.y1).min(clip_bottom);
            let caret_x = (prompt_x + bar.x0).min(frame.right - layout::CARET_WIDTH);
            if bottom > top {
                fill(
                    scene,
                    theme.text,
                    &Rect::new(caret_x, top, caret_x + layout::CARET_WIDTH, bottom),
                );
            }
        }
    }

    // A transient notice, or a scrollback indicator, as a caption under the
    // well. Never covers content.
    // Priority: a transient notice, then a scrollback indicator, then the
    // connection status. Status lives here instead of a masthead so the top of
    // the page stays clear, while a failure to attach is still visible.
    let footnote = model
        .notice
        .clone()
        .or_else(|| (model.scroll > 0).then(|| format!("scrolled back {} lines", model.scroll)))
        .or_else(|| model.status_footnote());
    let footnote = footnote.map(|line| {
        let chars = (frame.column() / (f64::from(layout::CAPTION_SIZE) * 0.72)) as usize;
        elide(&line, chars.max(12))
    });
    if let Some(footnote) = footnote {
        text.draw_paragraph_scaled(
            scene,
            &footnote,
            (frame.left, frame.footnote_top),
            frame.column() as f32,
            ParagraphStyle {
                font_size: layout::CAPTION_SIZE,
                color: theme.faint,
                letter_spacing_em: 0.1,
                ..Default::default()
            },
            scale,
        );
    }
}

/// Middle-elide `text` to at most `max_chars` characters, keeping the head and
/// tail (the informative ends of paths, ids, and error strings).
pub fn elide(text: &str, max_chars: usize) -> String {
    let text = text.trim();
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max_chars {
        return text.to_string();
    }
    if max_chars <= 3 {
        return "...".to_string();
    }
    let keep = max_chars - 3;
    let head = keep.div_ceil(2);
    let tail = keep - head;
    let mut out: String = chars[..head].iter().collect();
    out.push_str("...");
    out.extend(&chars[chars.len() - tail..]);
    out
}

#[cfg(test)]
mod tests {
    use super::elide;

    #[test]
    fn elide_keeps_short_text() {
        assert_eq!(elide("attached", 20), "attached");
    }

    #[test]
    fn elide_respects_budget_and_keeps_ends() {
        let out = elide("disconnected: no such file or directory (os error 2)", 24);
        assert_eq!(out.chars().count(), 24);
        assert!(out.starts_with("disconn"));
        assert!(out.ends_with("2)"));
    }

    #[test]
    fn elide_handles_tiny_budget() {
        assert_eq!(elide("abcdef", 2), "...");
    }
}
