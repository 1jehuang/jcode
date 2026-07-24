//! Text layout via Parley, rendered as Vello glyph runs.

use parley::{
    Alignment, FontContext, GlyphRun, Layout, LayoutContext, PositionedLayoutItem, StyleProperty,
};
use vello::Scene;
use vello::kurbo::Affine;
use vello::peniko::{Brush, Color, Fill};

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

impl TextSystem {
    /// Layout and draw a plain paragraph at `origin` wrapped to `max_width`.
    pub fn draw_paragraph(
        &mut self,
        scene: &mut Scene,
        text: &str,
        origin: (f64, f64),
        max_width: f32,
        font_size: f32,
        color: Color,
    ) {
        let mut builder = self.layouts.ranged_builder(&mut self.fonts, text, 1.0, true);
        builder.push_default(StyleProperty::FontSize(font_size));
        builder.push_default(StyleProperty::LineHeight(
            parley::LineHeight::FontSizeRelative(1.4),
        ));
        builder.push_default(StyleProperty::Brush(Brush::Solid(color)));
        let mut layout: Layout<Brush> = builder.build(text);
        layout.break_all_lines(Some(max_width));
        layout.align(Alignment::Start, parley::AlignmentOptions::default());
        for line in layout.lines() {
            for item in line.items() {
                if let PositionedLayoutItem::GlyphRun(glyph_run) = item {
                    draw_glyph_run(scene, &glyph_run, origin);
                }
            }
        }
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
                    id: glyph.id as u32,
                    x: glyph_x,
                    y: y - glyph.y,
                }
            }),
        );
}
