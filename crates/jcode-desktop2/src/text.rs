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

    /// Build a wrapped paragraph layout without drawing it, so callers can read
    /// caret and selection geometry from the very layout that will be drawn.
    /// `max_width` is in logical units.
    pub fn layout_paragraph(
        &mut self,
        text: &str,
        max_width: f32,
        style: ParagraphStyle,
        scale: f64,
    ) -> Layout<Brush> {
        let scale32 = scale as f32;
        let mut builder = self
            .layouts
            .ranged_builder(&mut self.fonts, text, scale32, true);
        Self::push_defaults(&mut builder, style);
        let mut layout: Layout<Brush> = builder.build(text);
        layout.break_all_lines(Some((max_width * scale32).max(1.0)));
        layout.align(Alignment::Start, parley::AlignmentOptions::default());
        layout
    }

    /// Draw an already-built layout at `origin` (logical units). Pairs with
    /// [`Self::layout_paragraph`] so geometry and glyphs share one layout.
    pub fn draw_layout(scene: &mut Scene, layout: &Layout<Brush>, origin: (f64, f64), scale: f64) {
        let origin = (origin.0 * scale, origin.1 * scale);
        for line in layout.lines() {
            for item in line.items() {
                if let PositionedLayoutItem::GlyphRun(glyph_run) = item {
                    draw_glyph_run(scene, &glyph_run, origin);
                }
            }
        }
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
        // One layout path for measuring, drawing, and geometry, so the caret
        // and selection can never disagree with the glyphs.
        let layout = self.layout_paragraph(text, max_width, style, scale);
        Self::draw_layout(scene, &layout, origin, scale);
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

    /// A paragraph is laid out in *logical* units, so the same text at the same
    /// logical width must wrap into the same lines at any scale factor. If this
    /// drifts, text reflows when a window moves between displays.
    #[test]
    fn wrapping_is_scale_independent() {
        let mut text = TextSystem::default();
        let sample = "alpha bravo charlie delta echo foxtrot golf hotel india";
        let base = text.layout_paragraph(sample, 180.0, style(), 1.0).len();
        assert!(base > 1, "sample did not wrap");
        for scale in [1.25, 1.5, 1.75, 2.0, 3.0] {
            let scaled = text.layout_paragraph(sample, 180.0, style(), scale).len();
            assert_eq!(scaled, base, "line count changed at scale {scale}");
        }
    }

    /// Measured height is in logical units too, so bottom-aligning the
    /// transcript cannot drift on a HiDPI display.
    #[test]
    fn measured_height_is_scale_independent() {
        let mut text = TextSystem::default();
        let sample = "alpha bravo charlie delta echo foxtrot golf hotel india juliet";
        let base = text.measure_paragraph(sample, 180.0, style(), 1.0);
        assert!(base > 0.0, "measured nothing");
        for scale in [1.25, 1.75, 2.0, 3.0] {
            let scaled = text.measure_paragraph(sample, 180.0, style(), scale);
            assert!(
                (scaled - base).abs() < base * 0.1,
                "height drifted at scale {scale}: {base:.1} vs {scaled:.1}"
            );
        }
    }

    /// More text at a fixed width means more height: the property the
    /// transcript relies on to paginate.
    #[test]
    fn height_grows_with_the_number_of_lines() {
        let mut text = TextSystem::default();
        let mut previous = 0.0;
        for count in 1..8 {
            let body = (0..count)
                .map(|n| format!("line {n}"))
                .collect::<Vec<_>>()
                .join("\n");
            let height = text.measure_paragraph(&body, 400.0, style(), 1.75);
            assert!(
                height > previous,
                "{count} lines measured {height:.1}, not taller than {previous:.1}"
            );
            previous = height;
        }
    }

    /// Narrower text wraps into at least as many lines: the wrap width is
    /// honoured rather than ignored.
    #[test]
    fn a_narrower_column_wraps_into_more_lines() {
        let mut text = TextSystem::default();
        let sample = "alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo";
        let mut previous = 0usize;
        for width in [600.0, 300.0, 150.0, 80.0] {
            let lines = text.layout_paragraph(sample, width, style(), 1.75).len();
            assert!(
                lines >= previous,
                "narrowing to {width} produced fewer lines: {lines} vs {previous}"
            );
            previous = lines;
        }
        assert!(previous > 1, "the narrowest column did not wrap");
    }

    /// Degenerate widths and text must lay out rather than panic.
    #[test]
    fn degenerate_layout_does_not_panic() {
        let mut text = TextSystem::default();
        for body in ["", "\n", "a", "ünïcödé", &"x".repeat(400)] {
            for width in [0.0, 1.0, 40.0, 5000.0] {
                let _ = text.layout_paragraph(body, width, style(), 1.75);
                let _ = text.measure_paragraph(body, width, style(), 1.75);
            }
        }
    }

    #[test]
    fn empty_text_measures_zero_lines_of_content() {
        let mut text = TextSystem::default();
        let layout = text.layout_paragraph("", 400.0, style(), 1.75);
        assert!(layout.len() <= 1, "empty text produced several lines");
    }
}
