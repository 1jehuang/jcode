//! TeX-style box layout: an atom tree plus a math font become positioned
//! glyphs and rules.
//!
//! The output is deliberately renderer-neutral. Layout decides *where every
//! shape goes*, in logical pixels relative to a single baseline origin, and
//! nothing else; a front-end only has to draw glyph outlines and filled
//! rectangles. That split is what lets the desktop app and an SVG exporter
//! show the same formula without either of them knowing any TeX.
//!
//! Sizes follow the TeXbook: a formula is laid out in a *style* (display,
//! text, script, scriptscript) and the style picks both the scale factor and
//! the shift constants used for fractions, scripts, and radicals.

use crate::font::{Glyph, MathConstants, MathFontFace};
use crate::parse::{AtomClass, Limits, MathFont, MatrixAlign, MatrixDelimiters, Node, StyleShift};
use crate::symbols;

/// A glyph placed at an absolute position, with the size it must be drawn at.
/// `y` is the baseline of the glyph, positive downward from the formula's own
/// baseline, matching every 2D renderer we target.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlacedGlyph {
    pub id: u16,
    pub x: f64,
    pub y: f64,
    /// Font size in logical pixels for this glyph.
    pub size: f64,
}

/// A filled rectangle: fraction bars, radical rules, overlines. Rules are
/// their own item rather than a glyph because they must be *exactly* as wide
/// as the thing they span, which no glyph can promise.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlacedRule {
    pub x: f64,
    /// Top edge, positive downward from the baseline.
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// One drawable produced by layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Item {
    Glyph(PlacedGlyph),
    Rule(PlacedRule),
}

impl Item {
    fn translated(self, dx: f64, dy: f64) -> Self {
        match self {
            Self::Glyph(g) => Self::Glyph(PlacedGlyph {
                x: g.x + dx,
                y: g.y + dy,
                ..g
            }),
            Self::Rule(r) => Self::Rule(PlacedRule {
                x: r.x + dx,
                y: r.y + dy,
                ..r
            }),
        }
    }
}

/// A laid-out box: its metrics plus the shapes inside it, positioned relative
/// to the box's own origin (left edge, baseline).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LayoutBox {
    pub width: f64,
    /// Height above the baseline.
    pub ascent: f64,
    /// Depth below the baseline (positive).
    pub descent: f64,
    pub items: Vec<Item>,
}

impl LayoutBox {
    pub fn height(&self) -> f64 {
        self.ascent + self.descent
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty() && self.width == 0.0
    }

    fn empty_with_width(width: f64) -> Self {
        Self {
            width,
            ..Self::default()
        }
    }

    /// Absorb `other`'s shapes at offset (`dx`, `dy`) without changing our own
    /// metrics. Callers that want the metrics to grow do that explicitly, so
    /// an overlay (an accent, a radical sign) cannot silently inflate a box.
    fn absorb(&mut self, other: &Self, dx: f64, dy: f64) {
        self.items
            .extend(other.items.iter().map(|item| item.translated(dx, dy)));
    }

    /// Place `other` to the right of everything so far and extend the metrics.
    fn append(&mut self, other: &Self, gap: f64) {
        let dx = self.width + gap;
        self.absorb(other, dx, 0.0);
        self.width = dx + other.width;
        self.ascent = self.ascent.max(other.ascent);
        self.descent = self.descent.max(other.descent);
    }
}

/// A laid-out box plus the atom class it presents to its neighbours. Spacing
/// in TeX is a function of adjacent classes, so the class has to survive
/// layout rather than being recomputed from shapes.
#[derive(Debug, Clone)]
struct Atom {
    class: AtomClass,
    boxed: LayoutBox,
}

/// TeX's four math styles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Display,
    Text,
    Script,
    ScriptScript,
}

impl Style {
    fn is_display(self) -> bool {
        matches!(self, Self::Display)
    }

    /// The style a script attached to something in this style is set in.
    fn script(self) -> Self {
        match self {
            Self::Display | Self::Text => Self::Script,
            Self::Script | Self::ScriptScript => Self::ScriptScript,
        }
    }

    /// The style for a fraction's numerator and denominator.
    fn fraction_part(self) -> Self {
        match self {
            Self::Display => Self::Text,
            Self::Text => Self::Script,
            Self::Script | Self::ScriptScript => Self::ScriptScript,
        }
    }

    /// Script-size styles suppress medium and thick inter-atom spacing, which
    /// is what keeps a subscript like `a_{i+1}` from looking gappy.
    fn suppresses_wide_spacing(self) -> bool {
        matches!(self, Self::Script | Self::ScriptScript)
    }

    fn scale(self, constants: &MathConstants) -> f64 {
        match self {
            Self::Display | Self::Text => 1.0,
            Self::Script => constants.script_percent,
            Self::ScriptScript => constants.script_script_percent,
        }
    }
}

/// Layout parameters carried down the tree: the current style, the font size
/// it resolves to, and TeX's "cramped" flag (a denominator or a radicand is
/// cramped, and cramped superscripts sit lower).
#[derive(Debug, Clone, Copy)]
struct Context {
    style: Style,
    cramped: bool,
    /// Base font size of the whole formula, in logical pixels.
    base_size: f64,
}

impl Context {
    /// Font size at the current style.
    fn size(self, constants: &MathConstants) -> f64 {
        self.base_size * self.style.scale(constants)
    }

    fn with_style(self, style: Style) -> Self {
        Self { style, ..self }
    }

    fn cramp(self) -> Self {
        Self {
            cramped: true,
            ..self
        }
    }
}

/// The engine: a font plus its constants, reused across a formula.
pub struct MathLayoutEngine<'a> {
    face: MathFontFace<'a>,
    constants: MathConstants,
}

/// A display formula: one or more baseline-separated lines.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MathLines {
    pub lines: Vec<LayoutBox>,
    pub width: f64,
    pub height: f64,
}

impl<'a> MathLayoutEngine<'a> {
    pub fn new(face: MathFontFace<'a>) -> Self {
        let constants = face.constants();
        Self { face, constants }
    }

    /// The bundled STIX Two Math engine.
    pub fn stix() -> MathLayoutEngine<'static> {
        MathLayoutEngine::new(MathFontFace::stix())
    }

    /// Lay out inline math (`$..$`) at `font_size` logical pixels.
    pub fn layout_inline(&self, source: &str, font_size: f64) -> LayoutBox {
        let node = crate::parse::parse(source);
        self.layout_atom(
            &node,
            Context {
                style: Style::Text,
                cramped: false,
                base_size: font_size,
            },
        )
        .boxed
    }

    /// Lay out display math (`$$..$$`). Explicit `\\` breaks become separate
    /// lines, stacked with normal display leading.
    pub fn layout_display(&self, source: &str, font_size: f64) -> MathLines {
        let node = crate::parse::parse(source);
        let context = Context {
            style: Style::Display,
            cramped: false,
            base_size: font_size,
        };
        let lines: Vec<LayoutBox> = split_lines(&node)
            .iter()
            .map(|line| self.layout_atom(line, context).boxed)
            .collect();
        let width = lines.iter().fold(0.0_f64, |acc, l| acc.max(l.width));
        let height = lines
            .iter()
            .map(|l| l.height().max(font_size * 0.9))
            .sum::<f64>();
        MathLines {
            lines,
            width,
            height,
        }
    }

    // -- core ------------------------------------------------------------

    fn layout_atom(&self, node: &Node, context: Context) -> Atom {
        match node {
            Node::Row(items) => self.layout_row(items, context),
            Node::Symbol { text, class, font } => self.layout_symbol(text, *class, *font, context),
            Node::Text(text) => Atom {
                class: AtomClass::Ord,
                boxed: self.layout_literal(text, MathFont::Upright, context),
            },
            Node::Space(em) => Atom {
                class: AtomClass::Ord,
                boxed: LayoutBox::empty_with_width(em * context.size(&self.constants)),
            },
            Node::Newline => Atom {
                class: AtomClass::Ord,
                boxed: LayoutBox::default(),
            },
            Node::LimitsControl(_) => Atom {
                class: AtomClass::Ord,
                boxed: LayoutBox::default(),
            },
            Node::Styled { style, body } => {
                let style = match style {
                    StyleShift::Display => Style::Display,
                    StyleShift::Text => Style::Text,
                    StyleShift::Script => Style::Script,
                    StyleShift::ScriptScript => Style::ScriptScript,
                };
                let inner = self.layout_atom(body, context.with_style(style));
                Atom {
                    class: AtomClass::Inner,
                    boxed: inner.boxed,
                }
            }
            Node::Fraction {
                numerator,
                denominator,
                thickness,
            } => Atom {
                class: AtomClass::Inner,
                boxed: self.layout_fraction(numerator, denominator, *thickness, context),
            },
            Node::Radical { index, radicand } => Atom {
                class: AtomClass::Ord,
                boxed: self.layout_radical(index.as_deref(), radicand, context),
            },
            Node::Scripts {
                base,
                superscript,
                subscript,
                limits,
            } => self.layout_scripts(
                base,
                superscript.as_deref(),
                subscript.as_deref(),
                *limits,
                context,
            ),
            Node::Fenced { left, body, right } => Atom {
                class: AtomClass::Inner,
                boxed: self.layout_fenced(left.as_deref(), body, right.as_deref(), context),
            },
            Node::Accent {
                accent,
                base,
                stretchy,
            } => Atom {
                class: AtomClass::Ord,
                boxed: self.layout_accent(accent, base, *stretchy, context),
            },
            Node::Bar { base, over } => Atom {
                class: AtomClass::Ord,
                boxed: self.layout_bar(base, *over, context),
            },
            Node::Boxed { body } => Atom {
                class: AtomClass::Inner,
                boxed: self.layout_boxed(body, context),
            },
            Node::Matrix {
                rows,
                delimiters,
                align,
            } => Atom {
                class: AtomClass::Inner,
                boxed: self.layout_matrix(rows, *delimiters, *align, context),
            },
        }
    }

    /// Lay out `\boxed{...}` with TeX-like padding and a rule on all sides.
    fn layout_boxed(&self, body: &Node, context: Context) -> LayoutBox {
        let inner = self.layout_atom(body, context).boxed;
        let size = context.size(&self.constants);
        let padding = size * 0.2;
        let thickness = (size * 0.055).max(0.75);
        let width = inner.width + padding * 2.0 + thickness * 2.0;
        let ascent = inner.ascent + padding + thickness;
        let descent = inner.descent + padding + thickness;
        let height = ascent + descent;
        let mut boxed = LayoutBox {
            width,
            ascent,
            descent,
            items: Vec::with_capacity(inner.items.len() + 4),
        };
        boxed.absorb(&inner, padding + thickness, 0.0);
        boxed.items.extend([
            Item::Rule(PlacedRule {
                x: 0.0,
                y: -ascent,
                width,
                height: thickness,
            }),
            Item::Rule(PlacedRule {
                x: 0.0,
                y: descent - thickness,
                width,
                height: thickness,
            }),
            Item::Rule(PlacedRule {
                x: 0.0,
                y: -ascent,
                width: thickness,
                height,
            }),
            Item::Rule(PlacedRule {
                x: width - thickness,
                y: -ascent,
                width: thickness,
                height,
            }),
        ]);
        boxed
    }

    /// A horizontal list, with TeX's class-driven inter-atom spacing.
    fn layout_row(&self, items: &[Node], context: Context) -> Atom {
        let laid: Vec<Atom> = items
            .iter()
            .map(|item| self.layout_atom(item, context))
            .collect();
        let mut out = LayoutBox::default();
        let mut previous: Option<AtomClass> = None;
        for atom in &laid {
            // Zero-width empties (a stray `\limits`) must not create spacing.
            if atom.boxed.is_empty() {
                continue;
            }
            let gap = previous.map_or(0.0, |left| {
                inter_atom_space(left, atom.class, context.style) * context.size(&self.constants)
            });
            out.append(&atom.boxed, gap);
            previous = Some(atom.class);
        }
        let class = match laid.len() {
            1 => laid[0].class,
            _ => AtomClass::Inner,
        };
        Atom { class, boxed: out }
    }

    fn layout_symbol(
        &self,
        text: &str,
        class: AtomClass,
        font: MathFont,
        context: Context,
    ) -> Atom {
        // A big operator is set larger in display style, and centred on the
        // math axis rather than sitting on the baseline: that vertical centring
        // is what makes a displayed sum look right next to its limits.
        if class == AtomClass::Op && context.style.is_display() {
            if let Some(ch) = text.chars().next() {
                if let Some(glyph) = self.face.display_variant(ch) {
                    let size = context.size(&self.constants);
                    let axis = self.constants.axis_height * size;
                    let half = (glyph.ascent - glyph.descent) / 2.0 * size;
                    let shift = half - axis;
                    let boxed = LayoutBox {
                        width: glyph.advance * size,
                        ascent: glyph.ascent * size - shift,
                        descent: glyph.descent * size + shift,
                        items: vec![Item::Glyph(PlacedGlyph {
                            id: glyph.id,
                            x: 0.0,
                            y: shift,
                            size,
                        })],
                    };
                    return Atom { class, boxed };
                }
            }
        }
        Atom {
            class,
            boxed: self.layout_literal(text, font, context),
        }
    }

    /// Set a run of characters in one alphabet, with no inter-atom spacing.
    fn layout_literal(&self, text: &str, font: MathFont, context: Context) -> LayoutBox {
        let size = context.size(&self.constants);
        let mut out = LayoutBox::default();
        for ch in text.chars() {
            if ch == ' ' {
                out.width += size * 0.28;
                continue;
            }
            let mapped = symbols::map_alphabet(ch, font);
            let Some(glyph) = self.face.glyph(mapped).or_else(|| self.face.glyph(ch)) else {
                // No glyph: reserve the space rather than silently closing the
                // gap, so a missing character is visible as a gap and not as a
                // wrong formula.
                out.width += size * 0.5;
                continue;
            };
            self.push_glyph(&mut out, glyph, size);
        }
        out
    }

    fn push_glyph(&self, out: &mut LayoutBox, glyph: Glyph, size: f64) {
        out.items.push(Item::Glyph(PlacedGlyph {
            id: glyph.id,
            x: out.width,
            y: 0.0,
            size,
        }));
        out.width += glyph.advance * size;
        out.ascent = out.ascent.max(glyph.ascent * size);
        out.descent = out.descent.max(glyph.descent * size);
    }

    fn layout_fraction(
        &self,
        numerator: &Node,
        denominator: &Node,
        thickness: Option<f64>,
        context: Context,
    ) -> LayoutBox {
        let part = context.with_style(context.style.fraction_part());
        let numerator = self.layout_atom(numerator, part).boxed;
        // The denominator is cramped: nothing in it may rise into the bar.
        let denominator = self.layout_atom(denominator, part.cramp()).boxed;
        let size = context.size(&self.constants);
        let c = &self.constants;
        let display = context.style.is_display();
        let rule = thickness.unwrap_or(c.fraction_rule_thickness) * size;
        let axis = c.axis_height * size;

        let (mut shift_up, mut shift_down) = if rule > 0.0 {
            if display {
                (
                    c.fraction_numerator_display_shift_up * size,
                    c.fraction_denominator_display_shift_down * size,
                )
            } else {
                (
                    c.fraction_numerator_shift_up * size,
                    c.fraction_denominator_shift_down * size,
                )
            }
        } else if display {
            (
                c.stack_top_display_shift_up * size,
                c.stack_bottom_display_shift_down * size,
            )
        } else {
            (
                c.stack_top_shift_up * size,
                c.stack_bottom_shift_down * size,
            )
        };

        if rule > 0.0 {
            let gap_num = if display {
                c.fraction_numerator_display_gap_min
            } else {
                c.fraction_numerator_gap_min
            } * size;
            let gap_den = if display {
                c.fraction_denominator_display_gap_min
            } else {
                c.fraction_denominator_gap_min
            } * size;
            // The bar sits on the axis; push the parts out until both clear it.
            let numerator_bottom = shift_up - numerator.descent;
            let bar_top = axis + rule / 2.0;
            if numerator_bottom < bar_top + gap_num {
                shift_up += bar_top + gap_num - numerator_bottom;
            }
            let denominator_top = denominator.ascent - shift_down;
            let bar_bottom = axis - rule / 2.0;
            if denominator_top > bar_bottom - gap_den {
                shift_down += denominator_top - (bar_bottom - gap_den);
            }
        } else {
            let gap_min = if display {
                c.stack_display_gap_min
            } else {
                c.stack_gap_min
            } * size;
            let gap = (shift_up - numerator.descent) - (denominator.ascent - shift_down);
            if gap < gap_min {
                let extra = (gap_min - gap) / 2.0;
                shift_up += extra;
                shift_down += extra;
            }
        }

        let width = numerator.width.max(denominator.width);
        // A little air either side, so `\frac{a}{b}` never touches a neighbour.
        let padding = 0.12 * size;
        let mut out = LayoutBox {
            width: width + padding * 2.0,
            ascent: 0.0,
            descent: 0.0,
            items: Vec::new(),
        };
        let numerator_dx = padding + (width - numerator.width) / 2.0;
        let denominator_dx = padding + (width - denominator.width) / 2.0;
        out.absorb(&numerator, numerator_dx, -shift_up);
        out.absorb(&denominator, denominator_dx, shift_down);
        out.ascent = numerator.ascent + shift_up;
        out.descent = denominator.descent + shift_down;
        if rule > 0.0 {
            out.items.push(Item::Rule(PlacedRule {
                x: padding,
                y: -axis - rule / 2.0,
                width,
                height: rule,
            }));
            out.ascent = out.ascent.max(axis + rule / 2.0);
            out.descent = out.descent.max(rule / 2.0 - axis);
        }
        out
    }

    fn layout_radical(&self, index: Option<&Node>, radicand: &Node, context: Context) -> LayoutBox {
        // A radicand is cramped: the rule above it is a ceiling.
        let body = self.layout_atom(radicand, context.cramp()).boxed;
        let size = context.size(&self.constants);
        let c = &self.constants;
        let rule = c.radical_rule_thickness * size;
        let gap = if context.style.is_display() {
            c.radical_display_vertical_gap
        } else {
            c.radical_vertical_gap
        } * size;

        // The sign must span the radicand plus the gap and the rule.
        let target = body.height() + gap + rule;
        let sign = self
            .face
            .vertical_variant('\u{221A}', target / size)
            .or_else(|| self.face.glyph('\u{221A}'));
        let Some(sign) = sign else {
            return body;
        };
        let sign_height = sign.height() * size;
        // Hang the sign so its top edge is the rule and its foot clears the
        // radicand; TeX aligns the two by their vertical extents.
        let inner_ascent = body.ascent + gap;
        let mut shift = sign.ascent * size - (inner_ascent + rule);
        if sign_height < inner_ascent + body.descent + rule {
            shift = sign.ascent * size - (inner_ascent + rule);
        }

        let mut out = LayoutBox::default();
        let index_box = index.map(|index| {
            let mut style = context;
            style.style = Style::ScriptScript;
            self.layout_atom(index, style).boxed
        });

        let mut x = 0.0;
        if let Some(index_box) = &index_box {
            let kern_before = c.radical_kern_before_degree * size;
            let raise = c.radical_degree_bottom_raise_percent * sign_height;
            let dy = -(sign.ascent * size - shift - raise) + index_box.descent;
            x += kern_before;
            out.absorb(index_box, x, dy);
            out.ascent = out.ascent.max(index_box.ascent - dy);
            x += index_box.width + c.radical_kern_after_degree * size;
            x = x.max(0.0);
        }

        out.items.push(Item::Glyph(PlacedGlyph {
            id: sign.id,
            x,
            y: shift,
            size,
        }));
        let sign_width = sign.advance * size;
        let body_x = x + sign_width;
        out.absorb(&body, body_x, 0.0);
        out.width = body_x + body.width;
        out.ascent = out
            .ascent
            .max(inner_ascent + rule + c.radical_extra_ascender * size);
        out.descent = out
            .descent
            .max(body.descent)
            .max(sign.descent * size - shift);
        out.items.push(Item::Rule(PlacedRule {
            x: body_x,
            y: -(inner_ascent + rule),
            width: body.width,
            height: rule,
        }));
        out
    }

    fn layout_scripts(
        &self,
        base: &Node,
        superscript: Option<&Node>,
        subscript: Option<&Node>,
        limits: Limits,
        context: Context,
    ) -> Atom {
        let base_atom = self.layout_atom(base, context);
        let use_limits = match limits {
            Limits::Above => true,
            Limits::Beside => false,
            // A big operator in display style takes limits above and below;
            // the same operator inline takes them beside, which is exactly the
            // difference between a displayed and an inline sum.
            Limits::Default => base_atom.class == AtomClass::Op && context.style.is_display(),
        };
        let script = context.with_style(context.style.script());
        let superscript = superscript.map(|node| self.layout_atom(node, script).boxed);
        let subscript = subscript.map(|node| self.layout_atom(node, script.cramp()).boxed);

        let boxed = if use_limits {
            self.stack_limits(&base_atom.boxed, superscript, subscript, context)
        } else {
            self.attach_scripts(base, &base_atom.boxed, superscript, subscript, context)
        };
        Atom {
            class: base_atom.class,
            boxed,
        }
    }

    /// Limits set above and below a big operator, centred on it.
    fn stack_limits(
        &self,
        base: &LayoutBox,
        superscript: Option<LayoutBox>,
        subscript: Option<LayoutBox>,
        context: Context,
    ) -> LayoutBox {
        let size = context.size(&self.constants);
        let c = &self.constants;
        let width = base
            .width
            .max(superscript.as_ref().map_or(0.0, |b| b.width))
            .max(subscript.as_ref().map_or(0.0, |b| b.width));
        let mut out = LayoutBox {
            width,
            ascent: base.ascent,
            descent: base.descent,
            items: Vec::new(),
        };
        out.absorb(base, (width - base.width) / 2.0, 0.0);
        if let Some(upper) = &superscript {
            let gap = c.upper_limit_gap_min * size;
            let rise = c.upper_limit_baseline_rise_min * size;
            let dy = -(base.ascent + gap + upper.descent).max(base.ascent + rise);
            out.absorb(upper, (width - upper.width) / 2.0, dy);
            out.ascent = out.ascent.max(upper.ascent - dy);
        }
        if let Some(lower) = &subscript {
            let gap = c.lower_limit_gap_min * size;
            let drop = c.lower_limit_baseline_drop_min * size;
            let dy = (base.descent + gap + lower.ascent).max(base.descent + drop);
            out.absorb(lower, (width - lower.width) / 2.0, dy);
            out.descent = out.descent.max(lower.descent + dy);
        }
        out
    }

    /// Scripts beside the base, TeX's shift-and-clearance rules.
    fn attach_scripts(
        &self,
        base_node: &Node,
        base: &LayoutBox,
        superscript: Option<LayoutBox>,
        subscript: Option<LayoutBox>,
        context: Context,
    ) -> LayoutBox {
        let size = context.size(&self.constants);
        let c = &self.constants;
        let mut out = base.clone();
        let x = base.width;
        // A slanted base leans into its superscript; the font's italic
        // correction is the amount to step right so they do not collide.
        let italic = self.italic_correction(base_node, context);

        let mut up = if context.cramped {
            c.superscript_shift_up_cramped
        } else {
            c.superscript_shift_up
        } * size;
        let mut down = c.subscript_shift_down * size;

        if let Some(upper) = &superscript {
            up = up
                .max(base.ascent - c.superscript_baseline_drop_max * size)
                .max(c.superscript_bottom_min * size + upper.descent);
        }
        if let Some(lower) = &subscript {
            down = down
                .max(base.descent + c.subscript_baseline_drop_min * size)
                .max(lower.ascent - c.subscript_top_max * size);
        }
        if let (Some(upper), Some(lower)) = (&superscript, &subscript) {
            // Both present: the gap between them has a floor, and the
            // superscript is not allowed to ride arbitrarily high.
            let gap = (up - upper.descent) - (lower.ascent - down);
            let min = c.sub_superscript_gap_min * size;
            if gap < min {
                down += min - gap;
                let top = c.superscript_bottom_max_with_subscript * size;
                let overshoot = (up - upper.descent) - top;
                if overshoot > 0.0 {
                    up -= overshoot;
                    down += overshoot;
                }
            }
        }

        let mut advance: f64 = 0.0;
        if let Some(upper) = &superscript {
            out.absorb(upper, x + italic, -up);
            out.ascent = out.ascent.max(upper.ascent + up);
            advance = advance.max(italic + upper.width);
        }
        if let Some(lower) = &subscript {
            out.absorb(lower, x, down);
            out.descent = out.descent.max(lower.descent + down);
            advance = advance.max(lower.width);
        }
        out.width = x + advance + c.space_after_script * size;
        out
    }

    fn italic_correction(&self, node: &Node, context: Context) -> f64 {
        let Node::Symbol { text, font, .. } = node else {
            return 0.0;
        };
        let Some(ch) = text.chars().next() else {
            return 0.0;
        };
        let mapped = symbols::map_alphabet(ch, *font);
        self.face
            .glyph(mapped)
            .or_else(|| self.face.glyph(ch))
            .map_or(0.0, |glyph| {
                glyph.italic_correction * context.size(&self.constants)
            })
    }

    fn layout_fenced(
        &self,
        left: Option<&str>,
        body: &Node,
        right: Option<&str>,
        context: Context,
    ) -> LayoutBox {
        let body = self.layout_atom(body, context).boxed;
        let size = context.size(&self.constants);
        let axis = self.constants.axis_height * size;
        // Fences are sized symmetrically about the axis, so `\left(\frac..`
        // brackets stay centred on the formula rather than on the baseline.
        let reach = (body.ascent - axis).max(body.descent + axis);
        let target = (2.0 * reach).max(self.constants.delimited_sub_formula_min_height * size);

        let mut out = LayoutBox::default();
        if let Some(left) = left.filter(|d| !d.is_empty()) {
            let fence = self.stretch_delimiter(left, target, size, axis);
            out.append(&fence, 0.0);
        }
        out.append(&body, 0.0);
        if let Some(right) = right.filter(|d| !d.is_empty()) {
            let fence = self.stretch_delimiter(right, target, size, axis);
            out.append(&fence, 0.0);
        }
        out
    }

    /// A delimiter grown to `target` height and centred on the math axis,
    /// using the font's pre-drawn variants first and its part assembly only
    /// when even the largest variant is too short.
    fn stretch_delimiter(&self, delimiter: &str, target: f64, size: f64, axis: f64) -> LayoutBox {
        let Some(ch) = delimiter.chars().next() else {
            return LayoutBox::default();
        };
        let variant = self
            .face
            .vertical_variant(ch, target / size)
            .or_else(|| self.face.glyph(ch));
        let Some(glyph) = variant else {
            return LayoutBox::default();
        };
        if glyph.height() * size >= target - 0.01 {
            let half = (glyph.ascent - glyph.descent) / 2.0 * size;
            let shift = half - axis;
            return LayoutBox {
                width: glyph.advance * size,
                ascent: glyph.ascent * size - shift,
                descent: glyph.descent * size + shift,
                items: vec![Item::Glyph(PlacedGlyph {
                    id: glyph.id,
                    x: 0.0,
                    y: shift,
                    size,
                })],
            };
        }
        match self.face.vertical_assembly(ch) {
            Some(parts) => self.assemble_delimiter(&parts, target, size, axis),
            None => LayoutBox {
                width: glyph.advance * size,
                ascent: target / 2.0 + axis,
                descent: target / 2.0 - axis,
                items: vec![Item::Glyph(PlacedGlyph {
                    id: glyph.id,
                    x: 0.0,
                    y: 0.0,
                    size,
                })],
            },
        }
    }

    /// Build a delimiter taller than any single glyph by repeating the font's
    /// extender parts between its fixed ends.
    fn assemble_delimiter(
        &self,
        parts: &[crate::font::AssemblyPart],
        target: f64,
        size: f64,
        axis: f64,
    ) -> LayoutBox {
        let overlap = self.constants.min_connector_overlap * size;
        let fixed: f64 = parts
            .iter()
            .filter(|p| !p.extender)
            .map(|p| p.full_advance * size)
            .sum();
        let extenders: Vec<_> = parts.iter().filter(|p| p.extender).collect();
        let joins = parts.len().saturating_sub(1) as f64;
        let extender_len: f64 = extenders
            .iter()
            .map(|p| p.full_advance * size - overlap)
            .sum();
        let repeats = if extender_len > 0.0 {
            let needed = target - (fixed - joins * overlap);
            (needed / extender_len).ceil().max(1.0) as usize
        } else {
            1
        };

        let mut out = LayoutBox::default();
        let width = parts
            .iter()
            .map(|p| self.face.glyph_advance(p.id) * size)
            .fold(0.0_f64, f64::max);
        // Parts are stacked from the top down, then the whole column is
        // centred on the axis in one move.
        let mut y = 0.0_f64;
        for part in parts {
            let count = if part.extender { repeats } else { 1 };
            for _ in 0..count {
                let (ascent, _descent) = self.face.glyph_extents(part.id);
                let advance = self.face.glyph_advance(part.id) * size;
                out.items.push(Item::Glyph(PlacedGlyph {
                    id: part.id,
                    x: (width - advance) / 2.0,
                    y: y + ascent * size,
                    size,
                }));
                y += part.full_advance * size - overlap;
            }
        }
        let total = y + overlap;
        let shift = total / 2.0 + axis;
        out.width = width;
        out.ascent = shift;
        out.descent = total - shift;
        for item in &mut out.items {
            *item = item.translated(0.0, -shift);
        }
        out
    }

    fn layout_accent(
        &self,
        accent: &str,
        base: &Node,
        stretchy: bool,
        context: Context,
    ) -> LayoutBox {
        // An accented base is cramped: the accent is the ceiling.
        let body = self.layout_atom(base, context.cramp()).boxed;
        let size = context.size(&self.constants);
        let Some(ch) = accent.chars().next() else {
            return body;
        };
        // Combining marks have no advance of their own; the spacing form is
        // what can be positioned, so prefer it and fall back to the mark.
        let spacing = spacing_form(ch);
        let glyph = if stretchy {
            self.face
                .vertical_variant(spacing, body.width / size)
                .or_else(|| self.face.glyph(spacing))
        } else {
            self.face.glyph(spacing)
        }
        .or_else(|| self.face.glyph(ch));
        let Some(glyph) = glyph else { return body };

        let mut out = body.clone();
        // The accent rides just above the base, but never lower than the
        // font's accent-base height: `\hat{x}` and `\hat{X}` line up.
        let clearance = body.ascent.min(self.constants.accent_base_height * size);
        let dy = -clearance - glyph.descent * size;
        let dx = (body.width - glyph.advance * size) / 2.0;
        out.items.push(Item::Glyph(PlacedGlyph {
            id: glyph.id,
            x: dx.max(0.0),
            y: dy,
            size,
        }));
        out.ascent = out.ascent.max(glyph.ascent * size - dy);
        out
    }

    fn layout_bar(&self, base: &Node, over: bool, context: Context) -> LayoutBox {
        let body = self
            .layout_atom(base, if over { context.cramp() } else { context })
            .boxed;
        let size = context.size(&self.constants);
        let c = &self.constants;
        let mut out = body.clone();
        if over {
            let gap = c.overbar_vertical_gap * size;
            let rule = c.overbar_rule_thickness * size;
            let y = -(body.ascent + gap + rule);
            out.items.push(Item::Rule(PlacedRule {
                x: 0.0,
                y,
                width: body.width,
                height: rule,
            }));
            out.ascent = body.ascent + gap + rule + c.overbar_extra_ascender * size;
        } else {
            let gap = c.underbar_vertical_gap * size;
            let rule = c.underbar_rule_thickness * size;
            let y = body.descent + gap;
            out.items.push(Item::Rule(PlacedRule {
                x: 0.0,
                y,
                width: body.width,
                height: rule,
            }));
            out.descent = body.descent + gap + rule + c.underbar_extra_descender * size;
        }
        out
    }

    fn layout_matrix(
        &self,
        rows: &[Vec<Node>],
        delimiters: MatrixDelimiters,
        align: MatrixAlign,
        context: Context,
    ) -> LayoutBox {
        let size = context.size(&self.constants);
        // Cells are set in text style: a display matrix of fractions would
        // otherwise grow without bound.
        let cell_context = context.with_style(match context.style {
            Style::Display => Style::Text,
            other => other,
        });
        let laid: Vec<Vec<LayoutBox>> = rows
            .iter()
            .map(|row| {
                row.iter()
                    .map(|cell| self.layout_atom(cell, cell_context).boxed)
                    .collect()
            })
            .collect();
        let columns = laid.iter().map(Vec::len).max().unwrap_or(0);
        let mut column_widths = vec![0.0_f64; columns];
        for row in &laid {
            for (index, cell) in row.iter().enumerate() {
                column_widths[index] = column_widths[index].max(cell.width);
            }
        }
        let column_gap = 0.6 * size;
        let row_gap = 0.35 * size;
        let body_width: f64 =
            column_widths.iter().sum::<f64>() + column_gap * (columns.saturating_sub(1)) as f64;

        // Stack rows, then centre the block on the math axis so fences and
        // neighbouring symbols line up with its middle, not its baseline.
        let mut body = LayoutBox {
            width: body_width,
            ..LayoutBox::default()
        };
        let mut y = 0.0_f64;
        let mut total_height = 0.0_f64;
        for (index, row) in laid.iter().enumerate() {
            let ascent = row.iter().fold(0.0_f64, |a, c| a.max(c.ascent));
            let descent = row.iter().fold(0.0_f64, |a, c| a.max(c.descent));
            if index > 0 {
                y += row_gap;
            }
            y += ascent;
            let mut x = 0.0_f64;
            for (column, cell) in row.iter().enumerate() {
                let width = column_widths[column];
                let dx = match align {
                    MatrixAlign::Center => x + (width - cell.width) / 2.0,
                    MatrixAlign::Left => x,
                    MatrixAlign::Alternating => {
                        if column % 2 == 0 {
                            x + (width - cell.width)
                        } else {
                            x
                        }
                    }
                };
                body.absorb(cell, dx, y);
                x += width + column_gap;
            }
            y += descent;
            total_height = y;
        }
        let axis = self.constants.axis_height * size;
        let shift = total_height / 2.0 + axis;
        for item in &mut body.items {
            *item = item.translated(0.0, -shift);
        }
        body.ascent = shift;
        body.descent = total_height - shift;

        match delimiters.pair() {
            None => body,
            Some((left, right)) => {
                let target = body.height();
                let mut out = LayoutBox::default();
                if !left.is_empty() {
                    out.append(&self.stretch_delimiter(left, target, size, axis), 0.0);
                }
                out.append(&body, 0.15 * size);
                if !right.is_empty() {
                    out.append(
                        &self.stretch_delimiter(right, target, size, axis),
                        0.15 * size,
                    );
                }
                out
            }
        }
    }
}

/// The spacing (standalone) form of a combining accent mark, which is what can
/// actually be positioned as a glyph.
fn spacing_form(ch: char) -> char {
    match ch {
        '\u{0300}' => '\u{02CB}',
        '\u{0301}' => '\u{02CA}',
        '\u{0302}' => '\u{02C6}',
        '\u{0303}' => '\u{02DC}',
        '\u{0304}' => '\u{00AF}',
        '\u{0306}' => '\u{02D8}',
        '\u{0307}' => '\u{02D9}',
        '\u{0308}' => '\u{00A8}',
        '\u{030A}' => '\u{02DA}',
        '\u{030C}' => '\u{02C7}',
        '\u{20D7}' => '\u{2192}',
        other => other,
    }
}

/// Split a formula on explicit `\\` breaks.
fn split_lines(node: &Node) -> Vec<Node> {
    let Node::Row(items) = node else {
        return vec![node.clone()];
    };
    if !items.iter().any(|item| matches!(item, Node::Newline)) {
        return vec![node.clone()];
    }
    let mut lines: Vec<Node> = Vec::new();
    let mut current: Vec<Node> = Vec::new();
    for item in items {
        if matches!(item, Node::Newline) {
            lines.push(Node::Row(std::mem::take(&mut current)));
        } else {
            current.push(item.clone());
        }
    }
    lines.push(Node::Row(current));
    lines.retain(|line| !matches!(line, Node::Row(items) if items.is_empty()));
    if lines.is_empty() {
        lines.push(Node::Row(Vec::new()));
    }
    lines
}

/// TeX's inter-atom spacing table (TeXbook chapter 18), in em.
fn inter_atom_space(left: AtomClass, right: AtomClass, style: Style) -> f64 {
    use AtomClass::{Bin, Close, Inner, Op, Open, Ord, Punct, Rel};
    const THIN: f64 = 3.0 / 18.0;
    const MEDIUM: f64 = 4.0 / 18.0;
    const THICK: f64 = 5.0 / 18.0;
    let (space, wide) = match (left, right) {
        (Ord, Op)
        | (Op, Ord)
        | (Op, Op)
        | (Close, Op)
        | (Punct, _)
        | (Inner, Op)
        | (Ord, Inner)
        | (Op, Inner)
        | (Close, Inner)
        | (Inner, Ord)
        | (Inner, Inner)
        | (Inner, Open)
        | (Inner, Punct)
        | (Close, Ord) => (THIN, false),
        (Ord, Bin)
        | (Bin, Ord)
        | (Bin, Op)
        | (Bin, Open)
        | (Bin, Inner)
        | (Inner, Bin)
        | (Close, Bin) => (MEDIUM, true),
        (Ord, Rel)
        | (Rel, Ord)
        | (Op, Rel)
        | (Rel, Op)
        | (Rel, Open)
        | (Rel, Inner)
        | (Close, Rel)
        | (Inner, Rel) => (THICK, true),
        _ => (0.0, false),
    };
    if wide && style.suppresses_wide_spacing() {
        0.0
    } else {
        space
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> MathLayoutEngine<'static> {
        MathLayoutEngine::stix()
    }

    /// The most basic promise of the engine: a formula produces shapes, with
    /// real metrics, rather than an empty box.
    #[test]
    fn a_simple_formula_produces_glyphs() {
        let laid = engine().layout_inline("a + b", 16.0);
        assert!(laid.width > 0.0, "zero width");
        assert!(laid.ascent > 0.0, "zero ascent");
        assert_eq!(
            laid.items
                .iter()
                .filter(|item| matches!(item, Item::Glyph(_)))
                .count(),
            3,
            "expected three glyphs: {:?}",
            laid.items
        );
    }

    /// A fraction is a *stack*: numerator above the bar, denominator below,
    /// with a rule between. If any of the three collapses onto the baseline
    /// the formula reads as `a+b/c`, which is a different formula.
    #[test]
    fn a_fraction_stacks_around_a_rule() {
        let laid = engine().layout_display("\\frac{a+b}{c}", 20.0);
        let line = &laid.lines[0];
        let rule = line
            .items
            .iter()
            .find_map(|item| match item {
                Item::Rule(rule) => Some(*rule),
                _ => None,
            })
            .expect("no fraction rule");
        let above = line
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Glyph(g) => Some(g.y),
                _ => None,
            })
            .filter(|y| *y < rule.y)
            .count();
        let below = line
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Glyph(g) => Some(g.y),
                _ => None,
            })
            .filter(|y| *y > rule.y)
            .count();
        assert_eq!(above, 3, "numerator glyphs not above the bar");
        assert_eq!(below, 1, "denominator glyph not below the bar");
        assert!(rule.width > 0.0);
    }

    #[test]
    fn boxed_formula_draws_four_sides_instead_of_command_text() {
        let layout = engine().layout_display(r"\boxed{e^{i\pi}+1=0}", 18.0);
        let line = layout.lines.first().expect("display line");
        let rules = line
            .items
            .iter()
            .filter(|item| matches!(item, Item::Rule(_)))
            .count();
        assert_eq!(rules, 4, "a simple boxed formula needs exactly four rules");
        assert!(line.width > 0.0 && line.height() > 0.0);
    }

    /// Scripts must be smaller than the base and offset vertically, which is
    /// the entire visual difference between `x^2` and `x2`.
    #[test]
    fn scripts_are_smaller_and_raised() {
        let laid = engine().layout_inline("x^2", 16.0);
        let glyphs: Vec<PlacedGlyph> = laid
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Glyph(g) => Some(*g),
                _ => None,
            })
            .collect();
        assert_eq!(glyphs.len(), 2);
        assert!(
            glyphs[1].size < glyphs[0].size,
            "superscript not scaled down"
        );
        assert!(glyphs[1].y < glyphs[0].y, "superscript not raised");
    }

    /// A displayed sum takes its limits above and below; the same sum inline
    /// takes them beside. Losing that distinction is the classic giveaway of
    /// a fake math renderer.
    #[test]
    fn display_sum_takes_limits_above_and_below() {
        let display = engine().layout_display("\\sum_{i=1}^{n} i", 20.0);
        let line = &display.lines[0];
        let glyphs: Vec<PlacedGlyph> = line
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Glyph(g) => Some(*g),
                _ => None,
            })
            .collect();
        let sigma = glyphs
            .iter()
            .max_by(|a, b| a.size.total_cmp(&b.size))
            .copied()
            .expect("no glyphs");
        let small: Vec<&PlacedGlyph> = glyphs.iter().filter(|g| g.size < sigma.size).collect();
        assert!(small.iter().any(|g| g.y < sigma.y), "no upper limit");
        assert!(small.iter().any(|g| g.y > sigma.y), "no lower limit");

        let inline = engine().layout_inline("\\sum_{i=1}^{n} i", 20.0);
        let inline_glyphs: Vec<PlacedGlyph> = inline
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Glyph(g) => Some(*g),
                _ => None,
            })
            .collect();
        let inline_sigma = inline_glyphs.first().copied().expect("no glyphs");
        assert!(
            inline_glyphs
                .iter()
                .skip(1)
                .all(|g| g.x >= inline_sigma.x + 0.001),
            "inline limits were stacked instead of set beside"
        );
    }

    #[test]
    fn explicit_limits_and_nolimits_change_script_placement() {
        let engine = engine();
        let above = engine.layout_display(r"\int\limits_0^1", 20.0);
        let beside = engine.layout_display(r"\int\nolimits_0^1", 20.0);
        let above = &above.lines[0];
        let beside = &beside.lines[0];
        assert!(
            above.height() > beside.height(),
            "limits should stack more vertically: {} vs {}",
            above.height(),
            beside.height()
        );
    }

    /// `\left(` around a tall body must actually grow. A fixed-size fence
    /// beside a fraction is the most visible failure mode of naive renderers.
    #[test]
    fn fences_grow_around_a_tall_body() {
        let short = engine().layout_inline("\\left( x \\right)", 16.0);
        let tall = engine().layout_inline("\\left( \\frac{a}{b} \\right)", 16.0);
        assert!(
            tall.height() > short.height() * 1.2,
            "fence did not grow: {} vs {}",
            tall.height(),
            short.height()
        );
    }

    /// Binary operators and relations get more air than ordinary atoms. This
    /// is the spacing table doing its job.
    #[test]
    fn relations_get_more_space_than_ordinals() {
        let tight = engine().layout_inline("ab", 16.0).width;
        let binary = engine().layout_inline("a+b", 16.0).width;
        let relation = engine().layout_inline("a=b", 16.0).width;
        let plus = engine().layout_inline("+", 16.0).width;
        let equals = engine().layout_inline("=", 16.0).width;
        assert!(
            binary - tight - plus > 0.0,
            "no spacing around a binary operator"
        );
        assert!(
            relation - tight - equals > binary - tight - plus,
            "relations are not spaced wider than binary operators"
        );
    }

    /// Every construct must survive layout without panicking and produce ink;
    /// this is the crate's state-space smoke test.
    #[test]
    fn every_construct_lays_out() {
        let cases = [
            "\\sqrt{x^2 + y^2}",
            "\\sqrt[3]{x}",
            "\\int_0^\\infty e^{-x^2} dx = \\frac{\\sqrt{\\pi}}{2}",
            "\\begin{pmatrix} \\cos\\theta & -\\sin\\theta \\\\ \\sin\\theta & \\cos\\theta \\end{pmatrix}",
            "\\begin{cases} 1 & x > 0 \\\\ 0 & x \\leq 0 \\end{cases}",
            "\\hat{x} + \\vec{v} + \\overline{z}",
            "\\mathbb{R}^n \\to \\mathcal{L}(\\mathbb{C})",
            "\\lim_{n \\to \\infty} \\left( 1 + \\frac{1}{n} \\right)^n = e",
            "\\alpha\\beta\\gamma \\quad \\text{for all } n",
            "\\binom{n}{k}",
        ];
        for case in cases {
            let laid = engine().layout_display(case, 18.0);
            assert!(laid.width > 0.0, "empty layout for {case}");
            assert!(
                laid.lines
                    .iter()
                    .any(|line| line.items.iter().any(|i| matches!(i, Item::Glyph(_)))),
                "no glyphs for {case}"
            );
        }
    }

    /// Representative model-generated LaTeX must make a finite, visible box.
    /// This corpus deliberately spans every parser/layout family rather than
    /// checking isolated symbols only.
    #[test]
    fn common_latex_compatibility_corpus_lays_out() {
        let formulas = [
            r"e^{i\pi}+1=0",
            r"\frac{-b\pm\sqrt{b^2-4ac}}{2a}",
            r"\sum_{k=1}^{n} k = \frac{n(n+1)}{2}",
            r"\int_{-\infty}^{\infty} e^{-x^2}\,dx=\sqrt{\pi}",
            r"\lim_{x\to0}\frac{\sin x}{x}=1",
            r"\left\langle x,y\right\rangle \leq \lVert x\rVert\lVert y\rVert",
            r"\begin{pmatrix}a&b\\c&d\end{pmatrix}",
            r"f(x)=\begin{cases}x^2,&x\geq0\\-x,&x<0\end{cases}",
            r"\begin{aligned}a&=b+c\\&=d\end{aligned}",
            r"\widehat{ABC}+\overline{z}+\vec v",
            r"\mathbb{R}^n\subseteq\mathcal{P}(\mathbb{R})",
            r"\binom{n}{k}=\frac{n!}{k!(n-k)!}",
            r"\boxed{\frac{a}{b}}",
            r"A\overset{\mathrm{def}}{=}B\underset{x\to0}{\longrightarrow}C",
            r"\sum_{\substack{1\le i\le n\\i\text{ odd}}}i",
            r"a\equiv b\pmod n",
            r"\textcolor{red}{x^2}+\colorbox{blue}{y}",
            r"\sum\limits_{i=1}^{n}i+\int\nolimits_0^1x\,dx",
            r"\overbrace{a+b}^{n}+\underbrace{c+d}_{m}\tag{1}",
            r"\cancel{x}+\bcancel{y}+\xcancel{z}",
        ];
        let engine = engine();
        for source in formulas {
            let display = engine.layout_display(source, 18.0);
            assert!(!display.lines.is_empty(), "no lines for {source:?}");
            assert!(
                display.width.is_finite() && display.width > 0.0,
                "bad width for {source:?}"
            );
            assert!(
                display.height.is_finite() && display.height > 0.0,
                "bad height for {source:?}"
            );
            assert!(
                display
                    .lines
                    .iter()
                    .flat_map(|line| &line.items)
                    .next()
                    .is_some(),
                "no drawable items for {source:?}"
            );
        }
    }

    /// An unknown command must render visibly rather than vanish: a formula
    /// with a stray `\foo` is debuggable, one with a hole is not.
    #[test]
    fn unknown_commands_survive() {
        let laid = engine().layout_inline("\\foo{x}", 16.0);
        assert!(laid.width > 0.0);
        assert!(laid.items.iter().any(|i| matches!(i, Item::Glyph(_))));
    }

    /// `\\` inside a display splits it into separate lines.
    #[test]
    fn explicit_breaks_make_lines() {
        let laid = engine().layout_display("a = b \\\\ c = d", 18.0);
        assert_eq!(laid.lines.len(), 2, "did not split on \\\\");
    }

    /// Pathological input must terminate and stay bounded rather than hang.
    #[test]
    fn deep_nesting_terminates() {
        let source = "\\frac{".repeat(200) + &"a}{b}".repeat(200);
        let laid = engine().layout_display(&source, 16.0);
        assert!(laid.height.is_finite());
    }
}
