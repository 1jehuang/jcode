//! Text layout via Parley, rendered as Vello glyph runs.

use parley::{
    Alignment, FontContext, GlyphRun, Layout, LayoutContext, PositionedLayoutItem, StyleProperty,
};
use vello::Scene;
use vello::kurbo::Affine;
use vello::peniko::{Brush, Color, Fill};

/// Design-language font stack: JetBrains Mono everywhere (see
/// ~/jcode-website/STYLE.md), monospace fallback.
const FONT_STACK: &str =
    "JetBrains Mono, JetBrainsMono Nerd Font, JetBrainsMono Nerd Font Mono, monospace";

/// Owns the font and layout contexts (both are expensive; reuse them).
pub struct TextSystem {
    fonts: FontContext,
    layouts: LayoutContext<Brush>,
}

impl Default for TextSystem {
    fn default() -> Self {
        Self {
            fonts: FontContext::new(),
            layouts: LayoutContext::new(),
        }
    }
}

/// Options for a paragraph. Defaults follow the style guide body copy.
#[derive(Clone, Copy)]
pub struct ParagraphStyle {
    pub font_size: f32,
    pub color: Color,
    pub bold: bool,
    /// Extra letterspacing in em (captions/hints use 0.12-0.2em).
    pub letter_spacing_em: f32,
    pub line_height: f32,
}

impl Default for ParagraphStyle {
    fn default() -> Self {
        Self {
            font_size: 15.0,
            color: vello::peniko::Color::from_rgb8(0x11, 0x11, 0x11),
            bold: false,
            letter_spacing_em: 0.0,
            line_height: 1.65,
        }
    }
}

impl TextSystem {
    /// Apply the design-language defaults for `style` to a layout builder.
    /// Shared by drawing and measurement so a measured caret position can
    /// never disagree with the drawn glyphs.
    fn push_defaults(builder: &mut parley::RangedBuilder<'_, Brush>, style: ParagraphStyle) {
        builder.push_default(StyleProperty::FontFamily(parley::FontFamily::Source(
            std::borrow::Cow::Borrowed(FONT_STACK),
        )));
        builder.push_default(StyleProperty::FontSize(style.font_size));
        if style.bold {
            builder.push_default(StyleProperty::FontWeight(parley::FontWeight::BOLD));
        }
        if style.letter_spacing_em > 0.0 {
            builder.push_default(StyleProperty::LetterSpacing(
                style.letter_spacing_em * style.font_size,
            ));
        }
        builder.push_default(StyleProperty::LineHeight(
            parley::LineHeight::FontSizeRelative(style.line_height),
        ));
        builder.push_default(StyleProperty::Brush(Brush::Solid(style.color)));
    }

    /// Width in logical pixels of `text` on one line, used to place the caret
    /// at a cursor offset. Measured with the same font and size as the drawn
    /// text so the caret lands exactly between glyphs.
    pub fn measure_width(&mut self, text: &str, style: ParagraphStyle, scale: f64) -> f64 {
        if text.is_empty() {
            return 0.0;
        }
        let scale32 = scale as f32;
        let mut builder = self
            .layouts
            .ranged_builder(&mut self.fonts, text, scale32, true);
        Self::push_defaults(&mut builder, style);
        let mut layout: Layout<Brush> = builder.build(text);
        layout.break_all_lines(None);
        // `full_width` includes trailing whitespace; `width` trims it, which
        // would place the caret before a trailing space instead of after it.
        f64::from(layout.full_width()) / scale
    }

    /// Byte offset in `text` nearest to `x` logical pixels from its start.
    /// Used to place the caret from a mouse click: picks the closest *gap*
    /// between characters, so clicking the right half of a glyph lands after
    /// it, like any normal text field.
    pub fn offset_at_x(&mut self, text: &str, x: f64, style: ParagraphStyle, scale: f64) -> usize {
        if text.is_empty() || x <= 0.0 {
            return 0;
        }
        let mut best = 0usize;
        let mut best_distance = f64::MAX;
        // Walk the char boundaries and measure each prefix. Text in the
        // composer is short, so this stays cheap and exactly matches the
        // drawn glyphs (same font, size, and scale).
        for (offset, _) in text
            .char_indices()
            .chain(std::iter::once((text.len(), ' ')))
        {
            let width = self.measure_width(&text[..offset], style, scale);
            let distance = (width - x).abs();
            if distance < best_distance {
                best_distance = distance;
                best = offset;
            }
        }
        best
    }

    /// Measure a paragraph without drawing it. Returns the wrapped height in
    /// logical pixels, so callers can bottom-align or paginate text.
    pub fn measure_paragraph(
        &mut self,
        text: &str,
        max_width: f32,
        style: ParagraphStyle,
        scale: f64,
    ) -> f64 {
        let mut scratch = Scene::new();
        self.draw_paragraph_scaled(&mut scratch, text, (0.0, 0.0), max_width, style, scale)
    }

    /// Layout and draw a paragraph at `origin`, wrapped to `max_width`.
    /// All inputs are in logical (device-independent) units; `scale` is the
    /// window scale factor. Returns the layout height in logical pixels.
    /// Text is laid out and rasterized at physical size, so glyphs stay crisp
    /// instead of being scaled up from a 1x layout.
    pub fn draw_paragraph_scaled(
        &mut self,
        scene: &mut Scene,
        text: &str,
        origin: (f64, f64),
        max_width: f32,
        style: ParagraphStyle,
        scale: f64,
    ) -> f64 {
        let scale32 = scale as f32;
        let mut builder = self
            .layouts
            .ranged_builder(&mut self.fonts, text, scale32, true);
        Self::push_defaults(&mut builder, style);
        let mut layout: Layout<Brush> = builder.build(text);
        layout.break_all_lines(Some(max_width * scale32));
        layout.align(Alignment::Start, parley::AlignmentOptions::default());
        let origin = (origin.0 * scale, origin.1 * scale);
        for line in layout.lines() {
            for item in line.items() {
                if let PositionedLayoutItem::GlyphRun(glyph_run) = item {
                    draw_glyph_run(scene, &glyph_run, origin);
                }
            }
        }
        f64::from(layout.height()) / scale
    }
}

fn draw_glyph_run(scene: &mut Scene, glyph_run: &GlyphRun<'_, Brush>, origin: (f64, f64)) {
    let run = glyph_run.run();
    let style = glyph_run.style();
    let mut x = glyph_run.offset();
    let y = glyph_run.baseline();
    scene
        .draw_glyphs(run.font())
        .font_size(run.font_size())
        .transform(Affine::translate((origin.0, origin.1)))
        .normalized_coords(run.normalized_coords())
        .brush(&style.brush)
        .draw(
            Fill::NonZero,
            glyph_run.glyphs().map(|glyph| {
                let glyph_x = x + glyph.x;
                x += glyph.advance;
                vello::Glyph {
                    id: glyph.id,
                    x: glyph_x,
                    y: y - glyph.y,
                }
            }),
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style() -> ParagraphStyle {
        ParagraphStyle {
            font_size: 13.5,
            ..Default::default()
        }
    }

    /// Measurement is in *logical* units, so the same text must measure the
    /// same width at any scale factor. If measurement ignored the scale, the
    /// caret and the soft-wrap budget would drift on HiDPI displays while
    /// looking correct at 1x.
    #[test]
    fn measured_width_is_scale_independent() {
        let mut text = TextSystem::default();
        let sample = "the quick brown fox";
        let base = text.measure_width(sample, style(), 1.0);
        assert!(base > 0.0, "measured nothing");
        for scale in [1.25, 1.5, 1.75, 2.0, 3.0] {
            let scaled = text.measure_width(sample, style(), scale);
            let drift = (scaled - base).abs();
            assert!(
                drift < base * 0.05,
                "width drifted at scale {scale}: {base:.1} vs {scaled:.1} logical px"
            );
        }
    }

    /// Hit-testing shares the measurement path, so a click must map to the same
    /// offset at every scale.
    #[test]
    fn hit_testing_is_scale_independent() {
        let mut text = TextSystem::default();
        let sample = "alpha bravo charlie";
        for offset in [0usize, 6, 12, sample.len()] {
            let x = text.measure_width(&sample[..offset], style(), 1.0);
            for scale in [1.0, 1.75, 2.0] {
                assert_eq!(
                    text.offset_at_x(sample, x, style(), scale),
                    offset,
                    "a click at offset {offset} missed at scale {scale}"
                );
            }
        }
    }

    #[test]
    fn measuring_scales_monotonically_with_text_length() {
        let mut text = TextSystem::default();
        let mut previous = 0.0;
        for count in 1..12 {
            let width = text.measure_width(&"m".repeat(count), style(), 1.75);
            assert!(
                width > previous,
                "{count} chars measured {width:.1}, not wider than {previous:.1}"
            );
            previous = width;
        }
    }

    #[test]
    fn empty_text_measures_zero() {
        let mut text = TextSystem::default();
        assert_eq!(text.measure_width("", style(), 1.75), 0.0);
    }

    #[test]
    fn a_trailing_space_widens_the_measurement() {
        // Guards the `full_width` vs `width` distinction: trimming trailing
        // whitespace puts the caret before a trailing space.
        let mut text = TextSystem::default();
        let bare = text.measure_width("word", style(), 1.0);
        let spaced = text.measure_width("word ", style(), 1.0);
        assert!(
            spaced > bare,
            "a trailing space was trimmed: {bare:.1} vs {spaced:.1}"
        );
    }
}
