//! The math font: glyph lookup, metrics, and the OpenType MATH constants.
//!
//! Everything the layout engine needs about shapes lives behind this type, in
//! **em units** (a fraction of the font size) rather than font design units.
//! Layout therefore never sees units-per-em and can be read as a description of
//! the formula rather than a pile of scaling arithmetic.

use ttf_parser::math::Table as MathTable;
use ttf_parser::{Face, GlyphId};

/// STIX Two Math, the reference OpenType MATH font. Vendored so a formula
/// looks the same on a machine with no math font installed, which is every
/// machine we have actually run on.
pub const STIX_TWO_MATH: &[u8] = include_bytes!("../assets/STIXTwoMath-Regular.ttf");

/// A loaded math font. Borrowing the face rather than owning it keeps this
/// free to construct per layout pass.
pub struct MathFontFace<'a> {
    face: Face<'a>,
    math: Option<MathTable<'a>>,
    units_per_em: f64,
}

/// A glyph resolved from the font, with the metrics layout needs. All values
/// are in em units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glyph {
    pub id: u16,
    /// Horizontal advance.
    pub advance: f64,
    /// Distance from the baseline to the top of the ink.
    pub ascent: f64,
    /// Distance from the baseline down to the bottom of the ink (positive).
    pub descent: f64,
    /// OpenType italic correction: the overhang a slanted glyph needs before a
    /// following superscript, which is why `\int^b` does not collide.
    pub italic_correction: f64,
}

impl Glyph {
    pub fn height(&self) -> f64 {
        self.ascent + self.descent
    }
}

/// One part of a stretchy delimiter assembly, in em units.
#[derive(Debug, Clone, Copy)]
pub struct AssemblyPart {
    pub id: u16,
    pub start_connector: f64,
    pub end_connector: f64,
    pub full_advance: f64,
    pub extender: bool,
}

/// The OpenType MATH constants used by layout, in em units. Defaults are the
/// values from the TeXbook's Computer Modern parameters, so a font with no
/// MATH table still lays out sensibly instead of collapsing.
#[derive(Debug, Clone, Copy)]
pub struct MathConstants {
    pub axis_height: f64,
    pub script_percent: f64,
    pub script_script_percent: f64,
    pub fraction_rule_thickness: f64,
    pub fraction_numerator_shift_up: f64,
    pub fraction_numerator_display_shift_up: f64,
    pub fraction_denominator_shift_down: f64,
    pub fraction_denominator_display_shift_down: f64,
    pub fraction_numerator_gap_min: f64,
    pub fraction_numerator_display_gap_min: f64,
    pub fraction_denominator_gap_min: f64,
    pub fraction_denominator_display_gap_min: f64,
    pub stack_top_shift_up: f64,
    pub stack_top_display_shift_up: f64,
    pub stack_bottom_shift_down: f64,
    pub stack_bottom_display_shift_down: f64,
    pub stack_gap_min: f64,
    pub stack_display_gap_min: f64,
    pub superscript_shift_up: f64,
    pub superscript_shift_up_cramped: f64,
    pub superscript_bottom_min: f64,
    pub superscript_baseline_drop_max: f64,
    pub subscript_shift_down: f64,
    pub subscript_top_max: f64,
    pub subscript_baseline_drop_min: f64,
    pub sub_superscript_gap_min: f64,
    pub superscript_bottom_max_with_subscript: f64,
    pub space_after_script: f64,
    pub upper_limit_gap_min: f64,
    pub upper_limit_baseline_rise_min: f64,
    pub lower_limit_gap_min: f64,
    pub lower_limit_baseline_drop_min: f64,
    pub radical_vertical_gap: f64,
    pub radical_display_vertical_gap: f64,
    pub radical_rule_thickness: f64,
    pub radical_extra_ascender: f64,
    pub radical_kern_before_degree: f64,
    pub radical_kern_after_degree: f64,
    pub radical_degree_bottom_raise_percent: f64,
    pub overbar_vertical_gap: f64,
    pub overbar_rule_thickness: f64,
    pub overbar_extra_ascender: f64,
    pub underbar_vertical_gap: f64,
    pub underbar_rule_thickness: f64,
    pub underbar_extra_descender: f64,
    pub accent_base_height: f64,
    pub display_operator_min_height: f64,
    pub delimited_sub_formula_min_height: f64,
    pub min_connector_overlap: f64,
}

impl Default for MathConstants {
    fn default() -> Self {
        Self {
            axis_height: 0.25,
            script_percent: 0.7,
            script_script_percent: 0.5,
            fraction_rule_thickness: 0.04,
            fraction_numerator_shift_up: 0.394,
            fraction_numerator_display_shift_up: 0.677,
            fraction_denominator_shift_down: 0.345,
            fraction_denominator_display_shift_down: 0.686,
            fraction_numerator_gap_min: 0.04,
            fraction_numerator_display_gap_min: 0.12,
            fraction_denominator_gap_min: 0.04,
            fraction_denominator_display_gap_min: 0.12,
            stack_top_shift_up: 0.45,
            stack_top_display_shift_up: 0.78,
            stack_bottom_shift_down: 0.48,
            stack_bottom_display_shift_down: 0.74,
            stack_gap_min: 0.12,
            stack_display_gap_min: 0.28,
            superscript_shift_up: 0.36,
            superscript_shift_up_cramped: 0.29,
            superscript_bottom_min: 0.125,
            superscript_baseline_drop_max: 0.375,
            subscript_shift_down: 0.2,
            subscript_top_max: 0.344,
            subscript_baseline_drop_min: 0.05,
            sub_superscript_gap_min: 0.16,
            superscript_bottom_max_with_subscript: 0.4,
            space_after_script: 0.04,
            upper_limit_gap_min: 0.111,
            upper_limit_baseline_rise_min: 0.3,
            lower_limit_gap_min: 0.111,
            lower_limit_baseline_drop_min: 0.6,
            radical_vertical_gap: 0.05,
            radical_display_vertical_gap: 0.12,
            radical_rule_thickness: 0.04,
            radical_extra_ascender: 0.04,
            radical_kern_before_degree: 0.28,
            radical_kern_after_degree: -0.56,
            radical_degree_bottom_raise_percent: 0.6,
            overbar_vertical_gap: 0.12,
            overbar_rule_thickness: 0.04,
            overbar_extra_ascender: 0.04,
            underbar_vertical_gap: 0.12,
            underbar_rule_thickness: 0.04,
            underbar_extra_descender: 0.04,
            accent_base_height: 0.45,
            display_operator_min_height: 1.4,
            delimited_sub_formula_min_height: 1.0,
            min_connector_overlap: 0.05,
        }
    }
}

impl<'a> MathFontFace<'a> {
    /// Parse a font. Returns `None` if the bytes are not a usable face.
    pub fn new(data: &'a [u8]) -> Option<Self> {
        let face = Face::parse(data, 0).ok()?;
        let math = face.tables().math;
        let units_per_em = f64::from(face.units_per_em());
        Some(Self {
            face,
            math,
            units_per_em,
        })
    }

    /// The bundled STIX Two Math face.
    pub fn stix() -> Self {
        Self::new(STIX_TWO_MATH).expect("bundled STIX Two Math font is valid")
    }

    /// Design units to em units.
    fn em(&self, units: f64) -> f64 {
        units / self.units_per_em
    }

    /// The font's ascender, for sizing a line that contains no glyphs.
    pub fn ascender(&self) -> f64 {
        self.em(f64::from(self.face.ascender()))
    }

    pub fn descender(&self) -> f64 {
        -self.em(f64::from(self.face.descender()))
    }

    /// The MATH constants, in em units, falling back to TeX's defaults for a
    /// font with no MATH table.
    pub fn constants(&self) -> MathConstants {
        let mut out = MathConstants::default();
        let Some(math) = self.math else { return out };
        if let Some(c) = math.constants {
            let v = |value: ttf_parser::math::MathValue<'_>| self.em(f64::from(value.value));
            out.axis_height = v(c.axis_height());
            out.script_percent = f64::from(c.script_percent_scale_down()) / 100.0;
            out.script_script_percent = f64::from(c.script_script_percent_scale_down()) / 100.0;
            out.fraction_rule_thickness = v(c.fraction_rule_thickness());
            out.fraction_numerator_shift_up = v(c.fraction_numerator_shift_up());
            out.fraction_numerator_display_shift_up =
                v(c.fraction_numerator_display_style_shift_up());
            out.fraction_denominator_shift_down = v(c.fraction_denominator_shift_down());
            out.fraction_denominator_display_shift_down =
                v(c.fraction_denominator_display_style_shift_down());
            out.fraction_numerator_gap_min = v(c.fraction_numerator_gap_min());
            out.fraction_numerator_display_gap_min = v(c.fraction_num_display_style_gap_min());
            out.fraction_denominator_gap_min = v(c.fraction_denominator_gap_min());
            out.fraction_denominator_display_gap_min = v(c.fraction_denom_display_style_gap_min());
            out.stack_top_shift_up = v(c.stack_top_shift_up());
            out.stack_top_display_shift_up = v(c.stack_top_display_style_shift_up());
            out.stack_bottom_shift_down = v(c.stack_bottom_shift_down());
            out.stack_bottom_display_shift_down = v(c.stack_bottom_display_style_shift_down());
            out.stack_gap_min = v(c.stack_gap_min());
            out.stack_display_gap_min = v(c.stack_display_style_gap_min());
            out.superscript_shift_up = v(c.superscript_shift_up());
            out.superscript_shift_up_cramped = v(c.superscript_shift_up_cramped());
            out.superscript_bottom_min = v(c.superscript_bottom_min());
            out.superscript_baseline_drop_max = v(c.superscript_baseline_drop_max());
            out.subscript_shift_down = v(c.subscript_shift_down());
            out.subscript_top_max = v(c.subscript_top_max());
            out.subscript_baseline_drop_min = v(c.subscript_baseline_drop_min());
            out.sub_superscript_gap_min = v(c.sub_superscript_gap_min());
            out.superscript_bottom_max_with_subscript =
                v(c.superscript_bottom_max_with_subscript());
            out.space_after_script = v(c.space_after_script());
            out.upper_limit_gap_min = v(c.upper_limit_gap_min());
            out.upper_limit_baseline_rise_min = v(c.upper_limit_baseline_rise_min());
            out.lower_limit_gap_min = v(c.lower_limit_gap_min());
            out.lower_limit_baseline_drop_min = v(c.lower_limit_baseline_drop_min());
            out.radical_vertical_gap = v(c.radical_vertical_gap());
            out.radical_display_vertical_gap = v(c.radical_display_style_vertical_gap());
            out.radical_rule_thickness = v(c.radical_rule_thickness());
            out.radical_extra_ascender = v(c.radical_extra_ascender());
            out.radical_kern_before_degree = v(c.radical_kern_before_degree());
            out.radical_kern_after_degree = v(c.radical_kern_after_degree());
            out.radical_degree_bottom_raise_percent =
                f64::from(c.radical_degree_bottom_raise_percent()) / 100.0;
            out.overbar_vertical_gap = v(c.overbar_vertical_gap());
            out.overbar_rule_thickness = v(c.overbar_rule_thickness());
            out.overbar_extra_ascender = v(c.overbar_extra_ascender());
            out.underbar_vertical_gap = v(c.underbar_vertical_gap());
            out.underbar_rule_thickness = v(c.underbar_rule_thickness());
            out.underbar_extra_descender = v(c.underbar_extra_descender());
            out.accent_base_height = v(c.accent_base_height());
            out.display_operator_min_height = self.em(f64::from(c.display_operator_min_height()));
            out.delimited_sub_formula_min_height =
                self.em(f64::from(c.delimited_sub_formula_min_height()));
        }
        if let Some(variants) = math.variants {
            out.min_connector_overlap = self.em(f64::from(variants.min_connector_overlap));
        }
        out
    }

    /// Resolve a character to a glyph with its metrics. Returns `None` when the
    /// font has no glyph for it, so the caller can fall back rather than draw
    /// a silent `.notdef` box.
    pub fn glyph(&self, ch: char) -> Option<Glyph> {
        let id = self.face.glyph_index(ch)?;
        Some(self.glyph_by_id(id))
    }

    pub fn glyph_by_id(&self, id: GlyphId) -> Glyph {
        let advance = self
            .face
            .glyph_hor_advance(id)
            .map_or(0.0, |a| self.em(f64::from(a)));
        let bbox = self.face.glyph_bounding_box(id);
        let (ascent, descent) = match bbox {
            Some(b) => (self.em(f64::from(b.y_max)), -self.em(f64::from(b.y_min))),
            None => (0.0, 0.0),
        };
        Glyph {
            id: id.0,
            advance,
            ascent,
            descent,
            italic_correction: self.italic_correction(id),
        }
    }

    fn italic_correction(&self, id: GlyphId) -> f64 {
        self.math
            .and_then(|math| math.glyph_info)
            .and_then(|info| info.italic_corrections)
            .and_then(|corrections| corrections.get(id))
            .map_or(0.0, |value| self.em(f64::from(value.value)))
    }

    /// The smallest vertical variant of `ch` that reaches `target` em, if the
    /// font has one. This is how `\left(` grows around a fraction: the font
    /// ships pre-drawn taller parentheses and we pick one, rather than scaling
    /// a glyph and thinning its strokes.
    pub fn vertical_variant(&self, ch: char, target: f64) -> Option<Glyph> {
        let id = self.face.glyph_index(ch)?;
        let variants = self.math?.variants?;
        let construction = variants.vertical_constructions.get(id)?;
        let target_units = target * self.units_per_em;
        let mut chosen = None;
        for variant in construction.variants {
            if f64::from(variant.advance_measurement) >= target_units {
                chosen = Some(variant.variant_glyph);
                break;
            }
            chosen = Some(variant.variant_glyph);
        }
        let chosen = chosen?;
        if chosen == id {
            return None;
        }
        Some(self.glyph_by_id(chosen))
    }

    /// The recipe for building an arbitrarily tall version of `ch` out of
    /// repeating parts, used when the font's largest pre-drawn variant is still
    /// too short (a fence around a tall matrix, say).
    pub fn vertical_assembly(&self, ch: char) -> Option<Vec<AssemblyPart>> {
        let id = self.face.glyph_index(ch)?;
        let variants = self.math?.variants?;
        let assembly = variants.vertical_constructions.get(id)?.assembly?;
        let parts: Vec<AssemblyPart> = assembly
            .parts
            .into_iter()
            .map(|part| AssemblyPart {
                id: part.glyph_id.0,
                start_connector: self.em(f64::from(part.start_connector_length)),
                end_connector: self.em(f64::from(part.end_connector_length)),
                full_advance: self.em(f64::from(part.full_advance)),
                extender: part.part_flags.extender(),
            })
            .collect();
        (!parts.is_empty()).then_some(parts)
    }

    /// The vertical extent of a glyph id, for measuring assembled parts.
    pub fn glyph_extents(&self, id: u16) -> (f64, f64) {
        let glyph = self.glyph_by_id(GlyphId(id));
        (glyph.ascent, glyph.descent)
    }

    /// The advance width of a glyph id.
    pub fn glyph_advance(&self, id: u16) -> f64 {
        self.glyph_by_id(GlyphId(id)).advance
    }

    /// The larger display-size variant of a big operator, e.g. the tall `\sum`
    /// a displayed sum gets and an inline one does not.
    pub fn display_variant(&self, ch: char) -> Option<Glyph> {
        let min = self.constants().display_operator_min_height;
        self.vertical_variant(ch, min)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bundled font must actually carry a MATH table: without it every
    /// constant silently falls back to the Computer Modern defaults and the
    /// layout stops matching the shapes being drawn.
    #[test]
    fn bundled_font_has_math_table() {
        let face = MathFontFace::stix();
        assert!(face.math.is_some(), "STIX Two Math has no MATH table");
        let constants = face.constants();
        assert!(
            constants.axis_height > 0.1 && constants.axis_height < 0.5,
            "implausible axis height: {}",
            constants.axis_height
        );
        assert!(constants.fraction_rule_thickness > 0.0);
    }

    /// A parenthesis must have taller pre-drawn variants, or `\left(` around a
    /// fraction cannot grow.
    #[test]
    fn parenthesis_has_vertical_variants() {
        let face = MathFontFace::stix();
        let plain = face.glyph('(').expect("no parenthesis glyph");
        let tall = face
            .vertical_variant('(', 2.0)
            .expect("no tall parenthesis variant");
        assert!(
            tall.height() > plain.height(),
            "variant {} is not taller than base {}",
            tall.height(),
            plain.height()
        );
    }

    /// Very tall fences are built from parts; without an assembly a fence
    /// around a big matrix would top out at the largest variant.
    #[test]
    fn parenthesis_has_an_assembly() {
        let face = MathFontFace::stix();
        let parts = face.vertical_assembly('(').expect("no assembly");
        assert!(parts.iter().any(|p| p.extender), "no extender part");
    }

    /// Math italic letters live at their own code points; if the mapping or
    /// the font coverage is wrong, variables silently render upright.
    #[test]
    fn math_italic_letters_exist() {
        let face = MathFontFace::stix();
        for ch in ['\u{1D465}', '\u{1D44E}', '\u{1D6FC}'] {
            assert!(
                face.glyph(ch).is_some(),
                "missing glyph U+{:04X}",
                ch as u32
            );
        }
    }
}
