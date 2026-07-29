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
        // A word longer than the column has no break opportunity, so with the
        // CSS-default `Normal` it overflows its wrap width instead of wrapping:
        // a pasted URL, a long path, or a run of keymashed characters would
        // paint straight out of the composer well and off the page. Break
        // anywhere, so the wrap width is a real bound on every layout.
        builder.push_default(StyleProperty::OverflowWrap(
            parley::style::OverflowWrap::Anywhere,
        ));
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
    #[cfg_attr(not(test), allow(dead_code))]
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
    // How far right the glyphs actually reached, so a decoration matches them.
    let mut drawn_to = x;

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
        // The trailing edge of what is actually on screen, so a decoration under
        // a half-revealed run stops with the glyphs instead of running ahead of
        // them to the end of the run.
        drawn_to = x;
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
    draw_decorations(scene, glyph_run, origin, drawn_to);
    ordinal
}

/// Draw a run's underline and strikethrough rules.
///
/// Parley *resolves* both decorations, including the font's own offset and
/// thickness, but it has nothing to draw them with: a Vello glyph batch is
/// glyphs and nothing else. So a `~~strikethrough~~` or a link's underline was
/// resolved and then silently dropped, and the desktop had no way to mark a
/// link except by spending a colour the print theme does not have.
///
/// The rule takes the decoration's own brush when it has one, so it can be
/// dimmer than the text, and the run's font metrics otherwise, so it sits where
/// the typeface says it should rather than at a guessed fraction of the size.
fn draw_decorations(
    scene: &mut Scene,
    glyph_run: &GlyphRun<'_, Brush>,
    origin: (f64, f64),
    end_x: f32,
) {
    for (rect, brush) in decoration_rules(glyph_run, end_x) {
        scene.fill(
            Fill::NonZero,
            Affine::translate(origin),
            &brush,
            None,
            &rect,
        );
    }
}

/// The rule rectangles a run's decorations contribute, in the run's own
/// coordinate space, paired with the brush each is drawn with.
///
/// Split out from the drawing so a test can assert *where* a rule lands rather
/// than only that some geometry was emitted: the bug this guards against, a
/// rule running the full width of a half-revealed run, is invisible to a
/// "something was drawn" check.
fn decoration_rules(
    glyph_run: &GlyphRun<'_, Brush>,
    end_x: f32,
) -> Vec<(vello::kurbo::Rect, Brush)> {
    let style = glyph_run.style();
    if style.underline.is_none() && style.strikethrough.is_none() {
        return Vec::new();
    }
    let start_x = f64::from(glyph_run.offset());
    let end_x = f64::from(end_x);
    if end_x <= start_x {
        return Vec::new();
    }
    let metrics = glyph_run.run().metrics();
    let baseline = f64::from(glyph_run.baseline());
    let rule = |decoration: &parley::Decoration<Brush>, offset: f32, size: f32| {
        // A zero or negative thickness would draw nothing (or an inverted
        // rect); fall back to a hairline so a decoration is never silently
        // dropped by a font with missing metrics.
        let size = f64::from(decoration.size.unwrap_or(size)).max(1.0);
        let offset = f64::from(decoration.offset.unwrap_or(offset));
        // Parley's offsets are measured up from the baseline, while the scene's
        // y grows downward.
        let top = baseline - offset - size;
        (
            vello::kurbo::Rect::new(start_x, top, end_x, top + size),
            decoration.brush.clone(),
        )
    };
    let mut rules = Vec::new();
    if let Some(underline) = style.underline.as_ref() {
        rules.push(rule(
            underline,
            metrics.underline_offset,
            metrics.underline_size,
        ));
    }
    if let Some(strikethrough) = style.strikethrough.as_ref() {
        rules.push(rule(
            strikethrough,
            metrics.strikethrough_offset,
            metrics.strikethrough_size,
        ));
    }
    rules
}

/// Every decoration rule in a layout, in layout coordinates. Test-facing view
/// of what [`draw_decorations`] paints.
#[cfg(test)]
fn layout_decoration_rules(layout: &Layout<Brush>, revealed: f64) -> Vec<vello::kurbo::Rect> {
    let mut rules = Vec::new();
    let mut ordinal = 0.0;
    for line in layout.lines() {
        for item in line.items() {
            let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                continue;
            };
            if revealed.is_finite() && ordinal >= revealed {
                return rules;
            }
            let mut drawn_to = glyph_run.offset();
            let mut x = glyph_run.offset();
            for glyph in glyph_run.glyphs() {
                x += glyph.advance;
                if crate::stream::glyph_alpha(ordinal, revealed).is_none() {
                    break;
                }
                ordinal += 1.0;
                drawn_to = x;
            }
            rules.extend(
                decoration_rules(&glyph_run, drawn_to)
                    .into_iter()
                    .map(|(rect, _)| rect),
            );
        }
    }
    rules
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

    /// Parley resolves underline and strikethrough but has nothing to draw them
    /// with: a Vello glyph batch is glyphs only. So both were resolved and then
    /// silently dropped, and a link or a `~~deletion~~` looked exactly like the
    /// prose around it. The scene must gain geometry beyond the glyphs.
    #[test]
    fn decorations_reach_the_scene() {
        let mut text = TextSystem::default();
        let sample = "underlined";
        let plain = {
            let mut scene = Scene::new();
            let layout = text.layout_paragraph(sample, 400.0, style(), 1.0);
            TextSystem::draw_layout(&mut scene, &layout, (0.0, 0.0), 1.0);
            scene.encoding().n_path_segments
        };
        for property in [
            StyleProperty::Underline(true),
            StyleProperty::Strikethrough(true),
        ] {
            let mut scene = Scene::new();
            let layout = text.layout_rich(sample, 400.0, style(), 1.0, &mut |builder| {
                builder.push(property.clone(), 0..sample.len());
            });
            TextSystem::draw_layout(&mut scene, &layout, (0.0, 0.0), 1.0);
            assert!(
                scene.encoding().n_path_segments > plain,
                "{property:?} drew no rule, so it is invisible"
            );
        }
    }

    /// A decoration under streaming text must stop where the glyphs stop. A rule
    /// running ahead of the reveal would announce the rest of the word before it
    /// arrives.
    #[test]
    fn a_decoration_stops_with_the_reveal() {
        let mut text = TextSystem::default();
        let sample = "a fairly long underlined stretch of text";
        let layout = text.layout_rich(sample, 400.0, style(), 1.0, &mut |builder| {
            builder.push(StyleProperty::Underline(true), 0..sample.len());
        });
        let width = |revealed: f64| {
            layout_decoration_rules(&layout, revealed)
                .iter()
                .map(vello::kurbo::Rect::width)
                .sum::<f64>()
        };
        let partial = width(4.0);
        let whole = width(f64::INFINITY);
        assert!(partial > 0.0, "a partly revealed run drew no rule at all");
        assert!(
            partial < whole,
            "the rule was drawn full width under a partly revealed run"
        );
    }
}
