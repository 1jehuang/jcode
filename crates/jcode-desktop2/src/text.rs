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
        f64::from(layout.width()) / scale
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

/// Per-character animation state supplied by the caller, keyed by byte offset
/// within the paragraph: opacity in 0.0..=1.0 and a vertical offset in logical
/// pixels (positive draws the glyph below its settled position).
/// A paragraph to draw: text plus its box and style. Grouped so the revealed
/// variant does not need a long positional argument list.
#[derive(Clone, Copy)]
pub struct Paragraph<'a> {
    pub text: &'a str,
    pub origin: (f64, f64),
    pub max_width: f32,
    pub style: ParagraphStyle,
}

pub struct Reveal<'a> {
    pub at: &'a dyn Fn(usize) -> (f32, f64),
}

impl TextSystem {
    /// Draw a paragraph whose characters are individually faded and offset by
    /// `reveal`, used for the streaming transcript. Layout is identical to
    /// [`TextSystem::draw_paragraph_scaled`] (same builder, same wrapping), so
    /// animating text never reflows as it settles: only its paint changes.
    pub fn draw_paragraph_revealed(
        &mut self,
        scene: &mut Scene,
        paragraph: Paragraph<'_>,
        scale: f64,
        reveal: &Reveal<'_>,
    ) -> f64 {
        let Paragraph {
            text,
            origin,
            max_width,
            style,
        } = paragraph;
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
                let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                    continue;
                };
                draw_revealed_run(scene, &glyph_run, origin, style.color, scale, reveal);
            }
        }
        f64::from(layout.height()) / scale
    }
}

/// Draw one glyph run cluster by cluster, so each character can carry its own
/// alpha and rise. Clusters (not glyphs) are the unit here because they are
/// what map back to source bytes.
fn draw_revealed_run(
    scene: &mut Scene,
    glyph_run: &GlyphRun<'_, Brush>,
    origin: (f64, f64),
    color: Color,
    scale: f64,
    reveal: &Reveal<'_>,
) {
    let run = glyph_run.run();
    let baseline = glyph_run.baseline();
    let mut x = glyph_run.offset();
    for cluster in run.visual_clusters() {
        let advance = cluster.advance();
        let byte = cluster.text_range().start;
        let (alpha, rise) = (reveal.at)(byte);
        if alpha > 0.001 {
            let dy = (rise * scale) as f32;
            let mut glyph_x = x;
            scene
                .draw_glyphs(run.font())
                .font_size(run.font_size())
                .transform(Affine::translate((origin.0, origin.1)))
                .normalized_coords(run.normalized_coords())
                .brush(&Brush::Solid(color.multiply_alpha(alpha)))
                .draw(
                    Fill::NonZero,
                    cluster.glyphs().map(|glyph| {
                        let out = vello::Glyph {
                            id: glyph.id,
                            x: glyph_x + glyph.x,
                            y: baseline - glyph.y + dy,
                        };
                        glyph_x += glyph.advance;
                        out
                    }),
                );
        }
        x += advance;
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
