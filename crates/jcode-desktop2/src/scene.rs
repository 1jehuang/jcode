//! Frame construction: the pure `Model` -> `Scene` function.
//!
//! Kept separate from the event loop so a frame is a pure function of the
//! model, which is what makes the state-space captures and pixel tests
//! possible.

use crate::text::ParagraphStyle;
use crate::{Model, layout, text};
use vello::Scene;
use vello::kurbo::{Affine, Rect, RoundedRect};
use vello::peniko::Color;

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
    // Hairlines stay one physical pixel regardless of scale.
    let hairline = |scene: &mut Scene, y: f64| {
        fill(
            scene,
            theme.rule,
            &Rect::new(frame.left, y, frame.right, y + frame.hairline()),
        );
    };

    // Paper.
    fill(
        scene,
        theme.background,
        &Rect::new(0.0, 0.0, frame.width, frame.height),
    );

    // Masthead: wordmark, then status as a caption beside it.
    text.draw_paragraph_scaled(
        scene,
        "jcode",
        (frame.left, frame.masthead_top),
        column,
        ParagraphStyle {
            font_size: layout::WORDMARK_SIZE,
            bold: true,
            color: theme.text,
            letter_spacing_em: 0.02,
            ..Default::default()
        },
        scale,
    );
    // Elide rather than wrap, so the masthead stays one line and never
    // crosses its own rule.
    let status_style = ParagraphStyle {
        font_size: layout::CAPTION_SIZE,
        color: if model.session_id.is_some() {
            theme.muted
        } else {
            theme.faint
        },
        letter_spacing_em: 0.1,
        ..Default::default()
    };
    let status_width = frame.status_width();
    let status_chars = (status_width / (f64::from(status_style.font_size) * 0.72)) as usize;
    let status = elide(&model.status, status_chars.max(12));
    text.draw_paragraph_scaled(
        scene,
        &status,
        (frame.status_left(), frame.masthead_top + 4.0),
        status_width as f32,
        status_style,
        scale,
    );
    // Meta row: version, update state, and the signed-in account. The same
    // three facts the TUI shows on its idle screen, elided rather than wrapped
    // so the masthead stays two lines tall at any width.
    let meta_style = ParagraphStyle {
        font_size: layout::CAPTION_SIZE,
        color: theme.faint,
        letter_spacing_em: 0.04,
        ..Default::default()
    };
    let meta_chars = (f64::from(column) / (f64::from(meta_style.font_size) * 0.62)) as usize;
    let meta = elide(&model.meta.caption(), meta_chars.max(12));
    text.draw_paragraph_scaled(
        scene,
        &meta,
        (frame.left, frame.masthead_meta),
        column,
        meta_style,
        scale,
    );
    hairline(scene, frame.masthead_rule);

    // Composer: a quiet well pinned to the bottom.
    fill_round(
        scene,
        theme.wash,
        &RoundedRect::new(
            frame.left,
            frame.composer_top,
            frame.right,
            frame.composer_bottom,
            layout::COMPOSER_RADIUS,
        ),
    );

    // Transcript: ink on paper, bottom-aligned against the composer so new
    // lines rise from the well rather than dangling from the masthead.
    let placeholder = model.transcript.trim().is_empty();
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
    let prompt_x = frame.left + layout::COMPOSER_PAD_X;
    let prompt_y = frame.composer_top + layout::COMPOSER_TEXT_OFFSET;
    let prompt_width = (frame.column() - layout::COMPOSER_PAD_X * 2.0) as f32;

    if model.busy {
        text.draw_paragraph_scaled(
            scene,
            "working...",
            (prompt_x, prompt_y),
            prompt_width,
            ParagraphStyle {
                color: theme.muted,
                ..prompt_style
            },
            scale,
        );
    } else {
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
                "message jcode",
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

        if model.caret.visible() {
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
    let footnote = model
        .notice
        .clone()
        .or_else(|| (model.scroll > 0).then(|| format!("scrolled back {} lines", model.scroll)));
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
