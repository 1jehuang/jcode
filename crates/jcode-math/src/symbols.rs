//! LaTeX command to Unicode symbol, with the atom class the symbol carries.
//!
//! The class matters as much as the character: TeX spaces `a\times b` like a
//! binary operator and `a\leq b` like a relation, and the only place that
//! distinction exists is this table. Getting `\to` classed as a relation is
//! what makes `f: A \to B` breathe the way it does in a paper.

use crate::parse::{AtomClass, MathFont};

/// A symbol's rendering: the character to set, its atom class, and the
/// alphabet it belongs to.
pub struct Symbol {
    pub text: &'static str,
    pub class: AtomClass,
    pub font: MathFont,
}

const fn sym(text: &'static str, class: AtomClass) -> Symbol {
    Symbol {
        text,
        class,
        font: MathFont::Upright,
    }
}

/// A Greek letter or other variable-like symbol: italic, like a variable.
const fn var(text: &'static str) -> Symbol {
    Symbol {
        text,
        class: AtomClass::Ord,
        font: MathFont::Italic,
    }
}

/// An accent's character and whether it stretches over a wide base.
pub struct Accent {
    pub symbol: &'static str,
    pub stretchy: bool,
}

/// Atom class of a literal character in math mode.
pub fn char_class(ch: char) -> AtomClass {
    match ch {
        '+' | '-' | '*' | '/' | '\u{00b1}' | '\u{2213}' | '\u{00d7}' | '\u{00f7}' => AtomClass::Bin,
        '=' | '<' | '>' | '\u{2264}' | '\u{2265}' | '\u{2260}' | '\u{2248}' | '\u{2261}' | ':' => {
            AtomClass::Rel
        }
        '(' | '[' => AtomClass::Open,
        ')' | ']' => AtomClass::Close,
        ',' | ';' => AtomClass::Punct,
        _ => AtomClass::Ord,
    }
}

/// Look up a LaTeX command name.
pub fn command(name: &str) -> Option<Symbol> {
    use AtomClass::{Bin, Close, Op, Open, Ord, Punct, Rel};
    let symbol = match name {
        // Greek lowercase. Italic, because in math they are variables.
        "alpha" => var("\u{03b1}"),
        "beta" => var("\u{03b2}"),
        "gamma" => var("\u{03b3}"),
        "delta" => var("\u{03b4}"),
        "epsilon" => var("\u{03f5}"),
        "varepsilon" => var("\u{03b5}"),
        "zeta" => var("\u{03b6}"),
        "eta" => var("\u{03b7}"),
        "theta" => var("\u{03b8}"),
        "vartheta" => var("\u{03d1}"),
        "iota" => var("\u{03b9}"),
        "kappa" => var("\u{03ba}"),
        "lambda" => var("\u{03bb}"),
        "mu" => var("\u{03bc}"),
        "nu" => var("\u{03bd}"),
        "xi" => var("\u{03be}"),
        "omicron" => var("\u{03bf}"),
        "pi" => var("\u{03c0}"),
        "varpi" => var("\u{03d6}"),
        "rho" => var("\u{03c1}"),
        "varrho" => var("\u{03f1}"),
        "sigma" => var("\u{03c3}"),
        "varsigma" => var("\u{03c2}"),
        "tau" => var("\u{03c4}"),
        "upsilon" => var("\u{03c5}"),
        "phi" => var("\u{03d5}"),
        "varphi" => var("\u{03c6}"),
        "chi" => var("\u{03c7}"),
        "psi" => var("\u{03c8}"),
        "omega" => var("\u{03c9}"),
        // Greek uppercase is upright in TeX's default (non-ISO) convention.
        "Gamma" => sym("\u{0393}", Ord),
        "Delta" => sym("\u{0394}", Ord),
        "Theta" => sym("\u{0398}", Ord),
        "Lambda" => sym("\u{039b}", Ord),
        "Xi" => sym("\u{039e}", Ord),
        "Pi" => sym("\u{03a0}", Ord),
        "Sigma" => sym("\u{03a3}", Ord),
        "Upsilon" => sym("\u{03a5}", Ord),
        "Phi" => sym("\u{03a6}", Ord),
        "Psi" => sym("\u{03a8}", Ord),
        "Omega" => sym("\u{03a9}", Ord),

        // Large operators. These take limits and grow in display style.
        "sum" => sym("\u{2211}", Op),
        "prod" => sym("\u{220f}", Op),
        "coprod" => sym("\u{2210}", Op),
        "int" => sym("\u{222b}", Op),
        "iint" => sym("\u{222c}", Op),
        "iiint" => sym("\u{222d}", Op),
        "oint" => sym("\u{222e}", Op),
        "bigcup" => sym("\u{22c3}", Op),
        "bigcap" => sym("\u{22c2}", Op),
        "bigoplus" => sym("\u{2a01}", Op),
        "bigotimes" => sym("\u{2a02}", Op),
        "bigvee" => sym("\u{22c1}", Op),
        "bigwedge" => sym("\u{22c0}", Op),

        // Named functions: upright, and operators so `\lim_{x\to0}` sets its
        // subscript underneath in display style.
        "lim" => sym("lim", Op),
        "limsup" => sym("lim sup", Op),
        "liminf" => sym("lim inf", Op),
        "max" => sym("max", Op),
        "min" => sym("min", Op),
        "sup" => sym("sup", Op),
        "inf" => sym("inf", Op),
        "det" => sym("det", Op),
        "gcd" => sym("gcd", Op),
        "Pr" => sym("Pr", Op),
        // These are operators too, so they get an operator's spacing, but
        // they never take limits above and below: `\sin^2 x` is a power on
        // the function, not a limit over it. Layout reads that off the name
        // being a word rather than a single operator glyph.
        "sin" => sym("sin", Op),
        "cos" => sym("cos", Op),
        "tan" => sym("tan", Op),
        "cot" => sym("cot", Op),
        "sec" => sym("sec", Op),
        "csc" => sym("csc", Op),
        "arcsin" => sym("arcsin", Op),
        "arccos" => sym("arccos", Op),
        "arctan" => sym("arctan", Op),
        "sinh" => sym("sinh", Op),
        "cosh" => sym("cosh", Op),
        "tanh" => sym("tanh", Op),
        "log" => sym("log", Op),
        "ln" => sym("ln", Op),
        "lg" => sym("lg", Op),
        "exp" => sym("exp", Op),
        "deg" => sym("deg", Op),
        "dim" => sym("dim", Op),
        "ker" => sym("ker", Op),
        "arg" => sym("arg", Op),
        "hom" => sym("hom", Op),
        "bmod" | "mod" => sym("mod", Bin),

        // Binary operators.
        "pm" => sym("\u{00b1}", Bin),
        "mp" => sym("\u{2213}", Bin),
        "times" => sym("\u{00d7}", Bin),
        "div" => sym("\u{00f7}", Bin),
        "cdot" => sym("\u{22c5}", Bin),
        "ast" => sym("\u{2217}", Bin),
        "star" => sym("\u{22c6}", Bin),
        "circ" => sym("\u{2218}", Bin),
        "bullet" => sym("\u{2219}", Bin),
        "oplus" => sym("\u{2295}", Bin),
        "ominus" => sym("\u{2296}", Bin),
        "otimes" => sym("\u{2297}", Bin),
        "oslash" => sym("\u{2298}", Bin),
        "odot" => sym("\u{2299}", Bin),
        "cup" => sym("\u{222a}", Bin),
        "cap" => sym("\u{2229}", Bin),
        "uplus" => sym("\u{228e}", Bin),
        "sqcup" => sym("\u{2294}", Bin),
        "sqcap" => sym("\u{2293}", Bin),
        "setminus" => sym("\u{2216}", Bin),
        "wedge" | "land" => sym("\u{2227}", Bin),
        "vee" | "lor" => sym("\u{2228}", Bin),
        "triangleleft" => sym("\u{25c1}", Bin),
        "triangleright" => sym("\u{25b7}", Bin),
        "amalg" => sym("\u{2a3f}", Bin),
        "dagger" => sym("\u{2020}", Bin),
        "ddagger" => sym("\u{2021}", Bin),
        "wr" => sym("\u{2240}", Bin),

        // Relations.
        "leq" | "le" => sym("\u{2264}", Rel),
        "geq" | "ge" => sym("\u{2265}", Rel),
        "neq" | "ne" => sym("\u{2260}", Rel),
        "leqslant" => sym("\u{2a7d}", Rel),
        "geqslant" => sym("\u{2a7e}", Rel),
        "ll" => sym("\u{226a}", Rel),
        "gg" => sym("\u{226b}", Rel),
        "prec" => sym("\u{227a}", Rel),
        "succ" => sym("\u{227b}", Rel),
        "preceq" => sym("\u{2aaf}", Rel),
        "succeq" => sym("\u{2ab0}", Rel),
        "sim" => sym("\u{223c}", Rel),
        "simeq" => sym("\u{2243}", Rel),
        "cong" => sym("\u{2245}", Rel),
        "approx" => sym("\u{2248}", Rel),
        "asymp" => sym("\u{224d}", Rel),
        "equiv" => sym("\u{2261}", Rel),
        "propto" => sym("\u{221d}", Rel),
        "doteq" => sym("\u{2250}", Rel),
        "subset" => sym("\u{2282}", Rel),
        "supset" => sym("\u{2283}", Rel),
        "subseteq" => sym("\u{2286}", Rel),
        "supseteq" => sym("\u{2287}", Rel),
        "subsetneq" => sym("\u{228a}", Rel),
        "supsetneq" => sym("\u{228b}", Rel),
        "sqsubseteq" => sym("\u{2291}", Rel),
        "sqsupseteq" => sym("\u{2292}", Rel),
        "in" => sym("\u{2208}", Rel),
        "notin" => sym("\u{2209}", Rel),
        "ni" | "owns" => sym("\u{220b}", Rel),
        "vdash" => sym("\u{22a2}", Rel),
        "dashv" => sym("\u{22a3}", Rel),
        "models" => sym("\u{22a8}", Rel),
        "perp" => sym("\u{22a5}", Rel),
        "mid" => sym("\u{2223}", Rel),
        "nmid" => sym("\u{2224}", Rel),
        "parallel" => sym("\u{2225}", Rel),
        "bowtie" => sym("\u{22c8}", Rel),
        "smile" => sym("\u{2323}", Rel),
        "frown" => sym("\u{2322}", Rel),
        "ncong" => sym("\u{2247}", Rel),
        "nsim" => sym("\u{2241}", Rel),
        "nless" => sym("\u{226e}", Rel),
        "ngtr" => sym("\u{226f}", Rel),
        "nleq" => sym("\u{2270}", Rel),
        "ngeq" => sym("\u{2271}", Rel),
        "nsubseteq" => sym("\u{2288}", Rel),
        "nsupseteq" => sym("\u{2289}", Rel),
        "coloneqq" => sym("\u{2254}", Rel),
        "eqqcolon" => sym("\u{2255}", Rel),

        // Arrows. Arrows are relations in TeX, which is why `A \to B` gets
        // relation-sized gaps rather than crowding the endpoints.
        "to" | "rightarrow" => sym("\u{2192}", Rel),
        "leftarrow" | "gets" => sym("\u{2190}", Rel),
        "leftrightarrow" => sym("\u{2194}", Rel),
        "Rightarrow" | "implies" => sym("\u{21d2}", Rel),
        "Leftarrow" | "impliedby" => sym("\u{21d0}", Rel),
        "Leftrightarrow" | "iff" => sym("\u{21d4}", Rel),
        "longrightarrow" => sym("\u{27f6}", Rel),
        "longleftarrow" => sym("\u{27f5}", Rel),
        "longleftrightarrow" => sym("\u{27f7}", Rel),
        "Longrightarrow" => sym("\u{27f9}", Rel),
        "Longleftarrow" => sym("\u{27f8}", Rel),
        "Longleftrightarrow" => sym("\u{27fa}", Rel),
        "mapsto" => sym("\u{21a6}", Rel),
        "longmapsto" => sym("\u{27fc}", Rel),
        "hookrightarrow" => sym("\u{21aa}", Rel),
        "hookleftarrow" => sym("\u{21a9}", Rel),
        "uparrow" => sym("\u{2191}", Rel),
        "downarrow" => sym("\u{2193}", Rel),
        "updownarrow" => sym("\u{2195}", Rel),
        "Uparrow" => sym("\u{21d1}", Rel),
        "Downarrow" => sym("\u{21d3}", Rel),
        "nearrow" => sym("\u{2197}", Rel),
        "searrow" => sym("\u{2198}", Rel),
        "swarrow" => sym("\u{2199}", Rel),
        "nwarrow" => sym("\u{2196}", Rel),
        "rightleftharpoons" => sym("\u{21cc}", Rel),

        // Ordinary symbols.
        "infty" => sym("\u{221e}", Ord),
        "partial" => sym("\u{2202}", Ord),
        "nabla" => sym("\u{2207}", Ord),
        "forall" => sym("\u{2200}", Ord),
        "exists" => sym("\u{2203}", Ord),
        "nexists" => sym("\u{2204}", Ord),
        "neg" | "lnot" => sym("\u{00ac}", Ord),
        "emptyset" => sym("\u{2205}", Ord),
        "varnothing" => sym("\u{2205}", Ord),
        "top" => sym("\u{22a4}", Ord),
        "bot" => sym("\u{22a5}", Ord),
        "angle" => sym("\u{2220}", Ord),
        "triangle" => sym("\u{25b3}", Ord),
        "square" => sym("\u{25a1}", Ord),
        "aleph" => sym("\u{2135}", Ord),
        "hbar" => sym("\u{210f}", Ord),
        "ell" => var("\u{2113}"),
        "Re" => sym("\u{211c}", Ord),
        "Im" => sym("\u{2111}", Ord),
        "wp" => sym("\u{2118}", Ord),
        "prime" => sym("\u{2032}", Ord),
        "surd" => sym("\u{221a}", Ord),
        "flat" => sym("\u{266d}", Ord),
        "natural" => sym("\u{266e}", Ord),
        "sharp" => sym("\u{266f}", Ord),
        "clubsuit" => sym("\u{2663}", Ord),
        "diamondsuit" => sym("\u{2662}", Ord),
        "heartsuit" => sym("\u{2661}", Ord),
        "spadesuit" => sym("\u{2660}", Ord),
        "checkmark" => sym("\u{2713}", Ord),
        "degree" => sym("\u{00b0}", Ord),
        "ldots" | "dots" => sym("\u{2026}", Ord),
        "cdots" => sym("\u{22ef}", Ord),
        "vdots" => sym("\u{22ee}", Ord),
        "ddots" => sym("\u{22f1}", Ord),
        "dotsb" => sym("\u{22ef}", Ord),

        // Delimiters.
        "langle" => sym("\u{27e8}", Open),
        "rangle" => sym("\u{27e9}", Close),
        "lceil" => sym("\u{2308}", Open),
        "rceil" => sym("\u{2309}", Close),
        "lfloor" => sym("\u{230a}", Open),
        "rfloor" => sym("\u{230b}", Close),
        "lbrace" | "{" => sym("{", Open),
        "rbrace" | "}" => sym("}", Close),
        "lbrack" => sym("[", Open),
        "rbrack" => sym("]", Close),
        "vert" | "|" => sym("|", Ord),
        "Vert" => sym("\u{2016}", Ord),
        "lvert" => sym("|", Open),
        "rvert" => sym("|", Close),
        "lVert" => sym("\u{2016}", Open),
        "rVert" => sym("\u{2016}", Close),
        "backslash" => sym("\u{2216}", Ord),

        // Escaped literals.
        "%" => sym("%", Ord),
        "$" => sym("$", Ord),
        "#" => sym("#", Ord),
        "&" => sym("&", Ord),
        "_" => sym("_", Ord),
        "colon" => sym(":", Punct),

        _ => return None,
    };
    Some(symbol)
}

/// Accent commands. `stretchy` accents grow to cover a wide base, which is the
/// whole difference between `\hat{x}` and `\widehat{x+y}`.
pub fn accent(name: &str) -> Option<Accent> {
    let (symbol, stretchy) = match name {
        "hat" => ("\u{0302}", false),
        "widehat" => ("\u{0302}", true),
        "tilde" => ("\u{0303}", false),
        "widetilde" => ("\u{0303}", true),
        "bar" => ("\u{0304}", false),
        "overbar" => ("\u{0304}", true),
        "vec" => ("\u{20d7}", false),
        "overrightarrow" => ("\u{20d7}", true),
        "dot" => ("\u{0307}", false),
        "ddot" => ("\u{0308}", false),
        "dddot" => ("\u{20db}", false),
        "acute" => ("\u{0301}", false),
        "grave" => ("\u{0300}", false),
        "check" => ("\u{030c}", false),
        "breve" => ("\u{0306}", false),
        "mathring" => ("\u{030a}", false),
        _ => return None,
    };
    Some(Accent { symbol, stretchy })
}

/// Map a character into a Unicode math alphabet. STIX Two Math encodes math
/// italic, blackboard bold, script, and the rest at their own code points, so
/// selecting an alphabet is a character remap rather than a font switch.
pub fn map_alphabet(ch: char, font: MathFont) -> char {
    let offset =
        |base: u32, start: u32| -> Option<char> { char::from_u32(base + (ch as u32 - start)) };
    let mapped = match font {
        MathFont::Upright => None,
        MathFont::Italic => match ch {
            // Math italic small h is unassigned in the plane-1 block; the
            // Planck constant character is what TeX shows there.
            'h' => Some('\u{210e}'),
            'a'..='z' => offset(0x1D44E, 'a' as u32),
            'A'..='Z' => offset(0x1D434, 'A' as u32),
            // Italic Greek, so `\theta` in a formula matches its neighbours.
            // Variant phi shares the ordinary Greek range in Unicode, so it
            // must be selected before the contiguous fallback.
            '\u{03c6}' => Some('\u{1D711}'),
            '\u{03b1}'..='\u{03c9}' => offset(0x1D6FC, 0x03b1),
            '\u{03d1}' => Some('\u{1D717}'),
            '\u{03d5}' => Some('\u{1D719}'),
            '\u{03f1}' => Some('\u{1D71A}'),
            '\u{03d6}' => Some('\u{1D71B}'),
            '\u{03f5}' => Some('\u{1D716}'),
            _ => None,
        },
        MathFont::Bold => match ch {
            'a'..='z' => offset(0x1D41A, 'a' as u32),
            'A'..='Z' => offset(0x1D400, 'A' as u32),
            '0'..='9' => offset(0x1D7CE, '0' as u32),
            '\u{03b1}'..='\u{03c9}' => offset(0x1D6C2, 0x03b1),
            _ => None,
        },
        MathFont::BoldItalic => match ch {
            'a'..='z' => offset(0x1D482, 'a' as u32),
            'A'..='Z' => offset(0x1D468, 'A' as u32),
            _ => None,
        },
        MathFont::DoubleStruck => match ch {
            // C, H, N, P, Q, R, Z are letterlike-symbols holes in the plane-1
            // block, so they must be picked up individually.
            'C' => Some('\u{2102}'),
            'H' => Some('\u{210D}'),
            'N' => Some('\u{2115}'),
            'P' => Some('\u{2119}'),
            'Q' => Some('\u{211A}'),
            'R' => Some('\u{211D}'),
            'Z' => Some('\u{2124}'),
            'a'..='z' => offset(0x1D552, 'a' as u32),
            'A'..='Z' => offset(0x1D538, 'A' as u32),
            '0'..='9' => offset(0x1D7D8, '0' as u32),
            _ => None,
        },
        MathFont::Script => match ch {
            'B' => Some('\u{212C}'),
            'E' => Some('\u{2130}'),
            'F' => Some('\u{2131}'),
            'H' => Some('\u{210B}'),
            'I' => Some('\u{2110}'),
            'L' => Some('\u{2112}'),
            'M' => Some('\u{2133}'),
            'R' => Some('\u{211B}'),
            'e' => Some('\u{212F}'),
            'g' => Some('\u{210A}'),
            'o' => Some('\u{2134}'),
            'a'..='z' => offset(0x1D4B6, 'a' as u32),
            'A'..='Z' => offset(0x1D49C, 'A' as u32),
            _ => None,
        },
        MathFont::Fraktur => match ch {
            'C' => Some('\u{212D}'),
            'H' => Some('\u{210C}'),
            'I' => Some('\u{2111}'),
            'R' => Some('\u{211C}'),
            'Z' => Some('\u{2128}'),
            'a'..='z' => offset(0x1D51E, 'a' as u32),
            'A'..='Z' => offset(0x1D504, 'A' as u32),
            _ => None,
        },
        MathFont::SansSerif => match ch {
            'a'..='z' => offset(0x1D5BA, 'a' as u32),
            'A'..='Z' => offset(0x1D5A0, 'A' as u32),
            '0'..='9' => offset(0x1D7E2, '0' as u32),
            _ => None,
        },
        MathFont::Monospace => match ch {
            'a'..='z' => offset(0x1D68A, 'a' as u32),
            'A'..='Z' => offset(0x1D670, 'A' as u32),
            '0'..='9' => offset(0x1D7F6, '0' as u32),
            _ => None,
        },
    };
    mapped.unwrap_or(ch)
}

/// Whether a multi-letter operator name takes its scripts as limits above and
/// below in display style.
///
/// TeX splits the operator names in two: `\lim`, `\max`, and their relatives
/// are declared `\limits` and set their subscript underneath, while `\sin` and
/// `\log` are `\nolimits` and take an ordinary superscript, which is what makes
/// `\sin^2 x` a squared sine rather than a sine with a limit over it. Both are
/// `Op` for spacing, so the difference has to live here.
pub fn takes_limits(name: &str) -> bool {
    matches!(
        name,
        "lim"
            | "limsup"
            | "liminf"
            | "max"
            | "min"
            | "sup"
            | "inf"
            | "det"
            | "gcd"
            | "Pr"
            | "argmax"
            | "argmin"
    )
}
