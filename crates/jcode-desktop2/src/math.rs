//! Display math, typeset properly and drawn as glyphs.
//!
//! Text front-ends get a Unicode approximation of a formula from render-core,
//! which is the right answer for a terminal and the wrong one here: the desktop
//! can draw arbitrary glyphs at arbitrary positions, so a fraction can be a
//! real stacked fraction rather than three rows of characters that happen to
//! line up. [`jcode_math`] does the TeX box layout against the OpenType MATH
//! table of a vendored math font; this module owns the font handle and turns
//! the resulting positions into Vello draw calls.

use jcode_math::{Item, LayoutBox, MathLayoutEngine, MathLines};
use vello::Scene;
use vello::kurbo::{Affine, Rect};
use vello::peniko::{Blob, Color, Fill, FontData};

/// Extra leading between the lines of a multi-line display, as a fraction of
/// the font size. Lines of one display are parts of one statement, so they are
/// set tighter than prose.
const LINE_GAP: f64 = 0.35;

/// Display math is set slightly larger than body copy. An equation is the
/// subject of the paragraph that introduces it, and at body size a fraction's
/// denominator falls below comfortable reading size.
pub const DISPLAY_SCALE: f64 = 1.15;

/// A typeset formula: its lines with their baselines resolved, plus the size
/// the block must reserve.
#[derive(Debug, Clone)]
pub struct Formula {
    lines: Vec<PlacedLine>,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone)]
struct PlacedLine {
    boxed: LayoutBox,
    /// Baseline offset from the top of the formula.
    baseline: f64,
}

/// Owns the math font. Constructing the Vello font parses the face, so it is
/// built once and shared, like [`crate::text::TextSystem`].
pub struct MathSystem {
    font: FontData,
}

impl Default for MathSystem {
    fn default() -> Self {
        Self {
            font: FontData::new(Blob::new(std::sync::Arc::new(jcode_math::STIX_TWO_MATH)), 0),
        }
    }
}

impl MathSystem {
    /// The process-wide math system. Parsing the face and reading its MATH
    /// table is per-font work, not per-formula work, so it is done once.
    pub fn shared() -> &'static Self {
        static SHARED: std::sync::OnceLock<MathSystem> = std::sync::OnceLock::new();
        SHARED.get_or_init(MathSystem::default)
    }

    /// Typeset display math at `font_size` logical pixels.
    pub fn typeset(&self, source: &str, font_size: f64) -> Formula {
        let engine = MathLayoutEngine::stix();
        let laid: MathLines = engine.layout_display(source, font_size * DISPLAY_SCALE);
        let gap = font_size * DISPLAY_SCALE * LINE_GAP;
        let mut lines = Vec::with_capacity(laid.lines.len());
        let mut y = 0.0_f64;
        for (index, boxed) in laid.lines.into_iter().enumerate() {
            if index > 0 {
                y += gap;
            }
            y += boxed.ascent;
            let baseline = y;
            y += boxed.descent;
            lines.push(PlacedLine { boxed, baseline });
        }
        let width = lines.iter().fold(0.0_f64, |a, l| a.max(l.boxed.width));
        Formula {
            lines,
            width,
            height: y,
        }
    }

    /// Typeset math that participates in a prose line. Unlike display math it
    /// keeps operator limits beside operators and stays at the body font size.
    pub fn typeset_inline(&self, source: &str, font_size: f64) -> Formula {
        let boxed = MathLayoutEngine::stix().layout_inline(source, font_size);
        let width = boxed.width;
        let height = boxed.ascent + boxed.descent;
        Formula {
            lines: vec![PlacedLine {
                baseline: boxed.ascent,
                boxed,
            }],
            width,
            height,
        }
    }

    /// Draw a typeset formula with its top-left corner at `origin`, in logical
    /// units.
    ///
    /// `revealed` is a count of glyphs to draw, matching the streaming reveal
    /// used for text: a formula that arrives mid-stream fills in rather than
    /// appearing whole while the sentence around it is still being written.
    pub fn draw(
        &self,
        scene: &mut Scene,
        formula: &Formula,
        origin: (f64, f64),
        color: Color,
        scale: f64,
        revealed: f64,
    ) {
        let mut drawn = 0.0_f64;
        for line in &formula.lines {
            let baseline = origin.1 + line.baseline;
            // A Vello glyph batch carries one font size, and a formula mixes
            // sizes by design (scripts are smaller), so glyphs are grouped by
            // size rather than emitted one draw call each.
            let mut batches: Vec<(f64, Vec<vello::Glyph>)> = Vec::new();
            for item in &line.boxed.items {
                match item {
                    Item::Glyph(glyph) => {
                        if drawn >= revealed {
                            break;
                        }
                        drawn += 1.0;
                        let x = ((origin.0 + glyph.x) * scale) as f32;
                        let y = ((baseline + glyph.y) * scale) as f32;
                        let size = glyph.size * scale;
                        match batches.iter_mut().find(|(s, _)| *s == size) {
                            Some((_, batch)) => batch.push(vello::Glyph {
                                id: u32::from(glyph.id),
                                x,
                                y,
                            }),
                            None => batches.push((
                                size,
                                vec![vello::Glyph {
                                    id: u32::from(glyph.id),
                                    x,
                                    y,
                                }],
                            )),
                        }
                    }
                    Item::Rule(rule) => {
                        // Rules are not part of the glyph reveal budget. In
                        // particular, boxed formula borders are stored after
                        // their glyphs; gating them at `drawn == revealed`
                        // made those borders disappear even at full reveal.
                        scene.fill(
                            Fill::NonZero,
                            Affine::scale(scale),
                            color,
                            None,
                            &Rect::new(
                                origin.0 + rule.x,
                                baseline + rule.y,
                                origin.0 + rule.x + rule.width,
                                baseline + rule.y + rule.height.max(0.5),
                            ),
                        );
                    }
                }
            }
            for (size, batch) in batches {
                scene
                    .draw_glyphs(&self.font)
                    .font_size(size as f32)
                    .brush(color)
                    .draw(Fill::NonZero, batch.into_iter());
            }
        }
    }
}

impl Formula {
    /// How many glyphs the formula contains, for the streaming reveal's
    /// per-block accounting.
    pub fn glyphs(&self) -> usize {
        self.lines
            .iter()
            .map(|line| {
                line.boxed
                    .items
                    .iter()
                    .filter(|item| matches!(item, Item::Glyph(_)))
                    .count()
            })
            .sum()
    }
}

/// The process-wide math system, for call sites that only typeset.
pub fn shared() -> &'static MathSystem {
    MathSystem::shared()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A formula must reserve real space, or the block below it draws on top.
    #[test]
    fn a_formula_has_size() {
        let math = MathSystem::default();
        let formula = math.typeset("\\frac{a+b}{c}", 15.0);
        assert!(formula.width > 0.0 && formula.height > 0.0);
        assert!(formula.glyphs() >= 4, "{} glyphs", formula.glyphs());
    }

    /// Multi-line displays stack with their baselines separated, rather than
    /// piling onto one line.
    #[test]
    fn lines_stack_downward() {
        let math = MathSystem::default();
        let formula = math.typeset("a = b \\\\ c = d", 15.0);
        assert_eq!(formula.lines.len(), 2);
        assert!(
            formula.lines[1].baseline > formula.lines[0].baseline,
            "second line did not move down"
        );
        assert!(formula.height > formula.lines[1].baseline);
    }

    /// Drawing must emit ink. This is the check that the font handle, the
    /// glyph ids from layout, and Vello's glyph API actually agree: a mismatch
    /// there produces a silently empty scene.
    #[test]
    fn drawing_emits_glyphs() {
        let math = MathSystem::default();
        let formula = math.typeset("\\sum_{i=1}^{n} x_i^2", 15.0);
        let mut scene = Scene::new();
        math.draw(
            &mut scene,
            &formula,
            (0.0, 0.0),
            Color::BLACK,
            1.0,
            f64::INFINITY,
        );
        assert!(
            !scene.encoding().resources.glyph_runs.is_empty()
                && !scene.encoding().resources.glyphs.is_empty(),
            "drawing a formula produced an empty scene"
        );
    }

    /// A partially revealed formula draws less than a whole one, so a formula
    /// arriving mid-stream fills in with the text around it.
    #[test]
    fn reveal_limits_what_is_drawn() {
        let math = MathSystem::default();
        let formula = math.typeset("x + y + z", 15.0);
        let mut partial = Scene::new();
        math.draw(&mut partial, &formula, (0.0, 0.0), Color::BLACK, 1.0, 1.0);
        let mut whole = Scene::new();
        math.draw(
            &mut whole,
            &formula,
            (0.0, 0.0),
            Color::BLACK,
            1.0,
            f64::INFINITY,
        );
        assert!(
            partial.encoding().resources.glyphs.len() < whole.encoding().resources.glyphs.len(),
            "partial reveal drew as much as the whole formula"
        );
    }

    #[test]
    fn fully_revealed_box_draws_all_four_rules() {
        let math = MathSystem::default();
        let formula = math.typeset(r"\boxed{x=1}", 15.0);
        let mut scene = Scene::new();
        math.draw(
            &mut scene,
            &formula,
            (0.0, 0.0),
            Color::BLACK,
            1.0,
            formula.glyphs() as f64,
        );
        assert!(
            scene.encoding().n_paths >= 4,
            "box borders were missing at full reveal"
        );
    }
}
