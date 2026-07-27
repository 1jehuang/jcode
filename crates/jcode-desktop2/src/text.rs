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
    /// Horizontal alignment within the wrap width. Start for body copy; the
    /// hero block centres, like the website's landing section.
    pub align: Align,
}

/// Horizontal alignment, kept as our own enum so scene code does not depend on
/// Parley's type directly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Align {
    #[default]
    Start,
    Center,
    /// Trailing edge of the wrap width (right, for LTR text).
    End,
}

impl Align {
    fn to_parley(self) -> Alignment {
        match self {
            Self::Start => Alignment::Start,
            Self::Center => Alignment::Center,
            Self::End => Alignment::End,
        }
    }
}

impl Default for ParagraphStyle {
    fn default() -> Self {
        Self {
            font_size: 15.0,
            color: vello::peniko::Color::from_rgb8(0x11, 0x11, 0x11),
            bold: false,
            letter_spacing_em: 0.0,
            line_height: 1.65,
            align: Align::Start,
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
        if style.letter_spacing_em != 0.0 {
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
    /// logical pixels.
    #[cfg_attr(not(test), allow(dead_code))]
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

    /// Width of a single unwrapped line in logical units. Used where an
    /// element must sit immediately after some text (the strip's bars after
    /// their group label), so the gap is the real one rather than a guess.
    pub fn measure_width(&mut self, text: &str, style: ParagraphStyle, scale: f64) -> f64 {
        let layout = self.layout_paragraph(text, f32::MAX, style, scale);
        f64::from(layout.width()) / scale
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
        layout.align(style.align.to_parley(), parley::AlignmentOptions::default());
        layout
    }

    /// Build a layout with per-range styling applied on top of the paragraph
    /// defaults. `apply` receives the builder so callers can push ranged
    /// properties (colour, weight, italic) for individual spans.
    ///
    /// This is what makes rich transcript text possible in a *single* layout:
    /// wrapping has to see the whole paragraph, so drawing each styled span as
    /// its own paragraph would break lines at every style boundary.
    pub fn layout_rich(
        &mut self,
        text: &str,
        max_width: f32,
        style: ParagraphStyle,
        scale: f64,
        apply: &mut dyn FnMut(&mut parley::RangedBuilder<'_, Brush>),
    ) -> Layout<Brush> {
        let scale32 = scale as f32;
        let mut builder = self
            .layouts
            .ranged_builder(&mut self.fonts, text, scale32, true);
        Self::push_defaults(&mut builder, style);
        apply(&mut builder);
        let mut layout: Layout<Brush> = builder.build(text);
        layout.break_all_lines(Some((max_width * scale32).max(1.0)));
        layout.align(style.align.to_parley(), parley::AlignmentOptions::default());
        layout
    }

    /// Draw an already-built layout at `origin` (logical units). Pairs with
    /// [`Self::layout_paragraph`] so geometry and glyphs share one layout.
    pub fn draw_layout(scene: &mut Scene, layout: &Layout<Brush>, origin: (f64, f64), scale: f64) {
        Self::draw_layout_revealed(scene, layout, origin, scale, f64::INFINITY);
    }

    /// Draw a layout with only its first `revealed` glyphs on screen, the
    /// leading edge fading and drifting in (see [`crate::stream`]).
    ///
    /// `revealed` is a *fractional glyph ordinal* within this layout, and
    /// `f64::INFINITY` means "all of it", which is the path every non-streaming
    /// caller takes and which costs exactly what the plain draw used to: the
    /// ramp is only entered for the handful of glyphs at the tip.
    pub fn draw_layout_revealed(
        scene: &mut Scene,
        layout: &Layout<Brush>,
        origin: (f64, f64),
        scale: f64,
        revealed: f64,
    ) {
        let origin = (origin.0 * scale, origin.1 * scale);
        // Glyphs are counted across the whole layout, not per run, so the
        // reveal sweeps continuously through a styled paragraph instead of
        // restarting at every bold span.
        let mut ordinal = 0.0;
        for line in layout.lines() {
            for item in line.items() {
                if let PositionedLayoutItem::GlyphRun(glyph_run) = item {
                    if revealed.is_finite() && ordinal >= revealed {
                        return;
                    }
                    ordinal = draw_glyph_run(scene, &glyph_run, origin, scale, revealed, ordinal);
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

/// Draw one glyph run, starting at glyph ordinal `ordinal`, and return the
/// ordinal after it.
///
/// Glyphs at the leading edge differ in alpha and vertical offset, and a Vello
/// glyph batch carries one brush and one transform, so the run is emitted as
/// batches of glyphs sharing a quantised ramp step. Settled text is a single
/// batch, which is why a long reply does not become thousands of draw calls.
fn draw_glyph_run(
    scene: &mut Scene,
    glyph_run: &GlyphRun<'_, Brush>,
    origin: (f64, f64),
    scale: f64,
    revealed: f64,
    ordinal: f64,
) -> f64 {
    let run = glyph_run.run();
    let style = glyph_run.style();
    let mut x = glyph_run.offset();
    let y = glyph_run.baseline();
    let mut ordinal = ordinal;
    // Batches of glyphs that share a ramp step, flushed when the step changes.
    let mut batch: Vec<vello::Glyph> = Vec::new();
    let mut batch_step: Option<u8> = None;

    let flush = |scene: &mut Scene, batch: &mut Vec<vello::Glyph>, step: Option<u8>| {
        let Some(step) = step else { return };
        if batch.is_empty() {
            return;
        }
        let alpha = f32::from(step) / f32::from(RAMP_STEPS);
        let brush = fade_brush(&style.brush, alpha);
        let rise = crate::stream::glyph_rise(alpha) * scale;
        scene
            .draw_glyphs(run.font())
            .font_size(run.font_size())
            .transform(Affine::translate((origin.0, origin.1 - rise)))
            .normalized_coords(run.normalized_coords())
            .brush(&brush)
            .draw(Fill::NonZero, batch.drain(..));
    };

    for glyph in glyph_run.glyphs() {
        let glyph_x = x + glyph.x;
        x += glyph.advance;
        let Some(alpha) = crate::stream::glyph_alpha(ordinal, revealed) else {
            break;
        };
        ordinal += 1.0;
        // Quantise so settled text collapses into one batch and the ramp is
        // still smooth: the eye cannot resolve 1/24 of an alpha step.
        let step = (alpha * f32::from(RAMP_STEPS)).round().clamp(0.0, 255.0) as u8;
        if batch_step != Some(step) {
            flush(scene, &mut batch, batch_step);
            batch_step = Some(step);
        }
        batch.push(vello::Glyph {
            id: glyph.id,
            x: glyph_x,
            y: y - glyph.y,
        });
    }
    flush(scene, &mut batch, batch_step);
    ordinal
}

/// Total glyphs in a layout. The reveal needs this to turn "how far through
/// this message are we" into "how many glyphs of this block are on screen",
/// and a layout does not expose a glyph count directly.
pub fn glyph_count(layout: &Layout<Brush>) -> usize {
    layout
        .lines()
        .flat_map(|line| line.items())
        .map(|item| match item {
            PositionedLayoutItem::GlyphRun(run) => run.glyphs().count(),
            PositionedLayoutItem::InlineBox(_) => 0,
        })
        .sum()
}

/// Quantisation steps of the fade ramp.
const RAMP_STEPS: u8 = 24;

/// A brush at `alpha` times its own opacity. Only solid brushes are used by
/// this app's text, and a gradient tip would be a different feature.
fn fade_brush(brush: &Brush, alpha: f32) -> Brush {
    match brush {
        Brush::Solid(color) => Brush::Solid(color.multiply_alpha(alpha)),
        other => other.clone(),
    }
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
