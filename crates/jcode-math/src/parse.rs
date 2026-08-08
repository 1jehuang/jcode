//! LaTeX math source to a TeX-style atom tree.
//!
//! This is deliberately a *math-mode only* parser: it never sees text mode, so
//! it can be small. What it must get right is the part TeX cares about, which
//! is not the syntax but the **atom class** of every symbol. Whether `+` is a
//! binary operator or a sign, and whether `=` is a relation, is what decides
//! the spacing in the laid-out formula, so the class travels with the atom
//! from here into layout rather than being guessed at draw time.

use crate::symbols;

const MAX_DEPTH: usize = 64;

/// TeX's atom classes (TeXbook chapter 17). Spacing between two adjacent atoms
/// is a pure function of their classes, which is what makes `a+b` and `f(x)`
/// space differently without any special cases in the layout code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtomClass {
    /// An ordinary symbol: a variable, a digit, `\alpha`.
    Ord,
    /// A large operator: `\sum`, `\int`.
    Op,
    /// A binary operator: `+`, `\times`.
    Bin,
    /// A relation: `=`, `\leq`.
    Rel,
    /// An opening delimiter: `(`, `[`.
    Open,
    /// A closing delimiter: `)`, `]`.
    Close,
    /// Punctuation: `,`, `;`.
    Punct,
    /// A sub-formula treated as ordinary (a braced group, a fraction).
    Inner,
}

/// Which math alphabet a symbol is set in. In TeX, single-letter variables are
/// italic and multi-letter operator names are upright, and that difference is
/// semantic, not decorative: `sin` is a function name, `s*i*n` is a product.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MathFont {
    /// Math italic: the default for variables.
    Italic,
    /// Upright roman: digits, operator names, `\mathrm`.
    Upright,
    /// Bold.
    Bold,
    /// Bold italic.
    BoldItalic,
    /// Blackboard bold, `\mathbb`.
    DoubleStruck,
    /// Script/calligraphic, `\mathcal`.
    Script,
    /// Fraktur, `\mathfrak`.
    Fraktur,
    /// Sans serif, `\mathsf`.
    SansSerif,
    /// Monospace, `\mathtt`.
    Monospace,
}

/// A parsed math node. `Row` is the only container that carries spacing; every
/// other variant is a single atom as far as inter-atom spacing is concerned.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    /// A horizontal list of atoms.
    Row(Vec<Node>),
    /// A single symbol with its class and alphabet.
    Symbol {
        text: String,
        class: AtomClass,
        font: MathFont,
    },
    /// Literal text from `\text{...}`, set upright with normal word spaces.
    Text(String),
    /// `\frac{a}{b}`. `thickness` is `None` for the font default and `Some(0)`
    /// for `\binom`-style stacks with no rule.
    Fraction {
        numerator: Box<Node>,
        denominator: Box<Node>,
        thickness: Option<f64>,
    },
    /// `\sqrt[n]{x}`.
    Radical {
        index: Option<Box<Node>>,
        radicand: Box<Node>,
    },
    /// A base with attached scripts.
    Scripts {
        base: Box<Node>,
        superscript: Option<Box<Node>>,
        subscript: Option<Box<Node>>,
        /// `\limits` behaviour: scripts go above/below rather than beside.
        limits: Limits,
    },
    /// A sub-formula fenced by stretchy delimiters, `\left( .. \right)`.
    Fenced {
        left: Option<String>,
        body: Box<Node>,
        right: Option<String>,
    },
    /// An accent placed over the base, e.g. `\hat{x}`, `\vec{v}`.
    Accent {
        accent: String,
        base: Box<Node>,
        /// A stretchy accent (`\widehat`, `\overline`) grows to the base width.
        stretchy: bool,
    },
    /// A rule over or under the base: `\overline`, `\underline`.
    Bar { base: Box<Node>, over: bool },
    /// A formula surrounded by a rectangular rule: `\boxed{...}`.
    Boxed { body: Box<Node> },
    /// A matrix / aligned environment.
    Matrix {
        rows: Vec<Vec<Node>>,
        delimiters: MatrixDelimiters,
        /// Per-column alignment. `aligned`/`cases` alternate right/left like
        /// LaTeX does, plain matrices centre.
        align: MatrixAlign,
    },
    /// Explicit horizontal space in em units (`\,`, `\quad`).
    Space(f64),
    /// A forced line break inside a display (`\\` outside a matrix).
    Newline,
    /// A style change that applies to the rest of the row.
    Styled { style: StyleShift, body: Box<Node> },
    /// Parser-only control that changes scripts on the preceding atom.
    LimitsControl(Limits),
}

/// Where scripts attach.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Limits {
    /// Follow the class: operators get limits in display style, others do not.
    #[default]
    Default,
    /// `\limits`: above and below.
    Above,
    /// `\nolimits`: beside.
    Beside,
}

/// Explicit `\displaystyle` / `\scriptstyle` requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StyleShift {
    Display,
    Text,
    Script,
    ScriptScript,
}

/// Fences around a matrix environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatrixDelimiters {
    None,
    Parentheses,
    Brackets,
    Braces,
    LeftBrace,
    Bars,
    DoubleBars,
}

impl MatrixDelimiters {
    pub fn pair(self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::None => None,
            Self::Parentheses => Some(("(", ")")),
            Self::Brackets => Some(("[", "]")),
            Self::Braces => Some(("{", "}")),
            Self::LeftBrace => Some(("{", "")),
            Self::Bars => Some(("|", "|")),
            Self::DoubleBars => Some(("\u{2016}", "\u{2016}")),
        }
    }
}

/// How matrix columns line up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatrixAlign {
    Center,
    /// `aligned`: odd columns flush right, even columns flush left, so the
    /// relation symbols in a derivation stack into a single vertical line.
    Alternating,
    Left,
}

/// Parse LaTeX math source into an atom tree. Unknown commands survive as
/// literal text rather than being dropped, because a formula with a visible
/// `\foo` in it is debuggable and a formula with a hole in it is not.
pub fn parse(source: &str) -> Node {
    Parser::new(source).parse_row(None, 0)
}

struct Parser<'a> {
    source: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str) -> Self {
        Self { source, pos: 0 }
    }

    fn parse_row(&mut self, terminator: Option<char>, depth: usize) -> Node {
        let mut items: Vec<Node> = Vec::new();
        while let Some(ch) = self.peek() {
            if Some(ch) == terminator {
                self.bump();
                break;
            }
            match ch {
                '^' | '_' => {
                    self.bump();
                    let script = self.parse_argument(depth + 1);
                    let base = items.pop().unwrap_or(Node::Row(Vec::new()));
                    items.push(attach_script(base, script, ch == '^'));
                }
                '{' => {
                    self.bump();
                    let group = self.parse_row(Some('}'), depth + 1);
                    items.push(group_atom(group));
                }
                '}' => {
                    // An unmatched close brace: consume it rather than spin.
                    self.bump();
                }
                '\\' => {
                    if let Some(node) = self.parse_command(depth) {
                        if let Node::LimitsControl(limits) = node {
                            if let Some(base) = items.pop() {
                                items.push(with_limits(base, limits));
                            }
                        } else {
                            items.push(node);
                        }
                    }
                }
                '&' => {
                    // Alignment tabs only mean something inside an environment,
                    // which handles them before we get here.
                    self.bump();
                }
                ch if ch.is_whitespace() => {
                    self.bump();
                }
                _ => {
                    self.bump();
                    items.push(self.symbol_from_char(ch));
                }
            }
            if depth > MAX_DEPTH {
                break;
            }
        }
        collapse(items)
    }

    /// One argument: a braced group, a single command, or a single character.
    fn parse_argument(&mut self, depth: usize) -> Node {
        self.skip_spaces();
        match self.peek() {
            Some('{') => {
                self.bump();
                let inner = self.parse_row(Some('}'), depth + 1);
                group_atom(inner)
            }
            Some('\\') => self
                .parse_command(depth + 1)
                .unwrap_or(Node::Row(Vec::new())),
            Some(ch) => {
                self.bump();
                self.symbol_from_char(ch)
            }
            None => Node::Row(Vec::new()),
        }
    }

    fn symbol_from_char(&self, ch: char) -> Node {
        let class = symbols::char_class(ch);
        // Single letters are variables and set in math italic; digits and
        // everything else keep their upright shapes.
        let font = if ch.is_ascii_alphabetic() {
            MathFont::Italic
        } else {
            MathFont::Upright
        };
        Node::Symbol {
            text: ch.to_string(),
            class,
            font,
        }
    }

    fn parse_command(&mut self, depth: usize) -> Option<Node> {
        self.bump(); // the backslash
        let start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
            self.bump();
        }
        let name: String = if self.pos == start {
            let ch = self.bump()?;
            ch.to_string()
        } else {
            self.source[start..self.pos].to_string()
        };
        if name.chars().all(|c| c.is_ascii_alphabetic()) {
            self.skip_spaces();
        }
        if depth > MAX_DEPTH {
            return Some(Node::Text(format!("\\{name}")));
        }
        Some(self.build_command(&name, depth))
    }

    fn build_command(&mut self, name: &str, depth: usize) -> Node {
        match name {
            "frac" | "dfrac" | "tfrac" | "cfrac" => {
                let numerator = Box::new(self.parse_argument(depth + 1));
                let denominator = Box::new(self.parse_argument(depth + 1));
                let node = Node::Fraction {
                    numerator,
                    denominator,
                    thickness: None,
                };
                match name {
                    "dfrac" => Node::Styled {
                        style: StyleShift::Display,
                        body: Box::new(node),
                    },
                    "tfrac" => Node::Styled {
                        style: StyleShift::Text,
                        body: Box::new(node),
                    },
                    _ => node,
                }
            }
            "binom" | "dbinom" | "tbinom" => {
                let numerator = Box::new(self.parse_argument(depth + 1));
                let denominator = Box::new(self.parse_argument(depth + 1));
                Node::Fenced {
                    left: Some("(".into()),
                    body: Box::new(Node::Fraction {
                        numerator,
                        denominator,
                        thickness: Some(0.0),
                    }),
                    right: Some(")".into()),
                }
            }
            "sqrt" => {
                let index = if self.peek() == Some('[') {
                    self.bump();
                    let raw = self.take_bracketed();
                    Some(Box::new(parse(&raw)))
                } else {
                    None
                };
                Node::Radical {
                    index,
                    radicand: Box::new(self.parse_argument(depth + 1)),
                }
            }
            "text" | "textrm" | "textnormal" | "mbox" | "textsf" | "texttt" => {
                Node::Text(self.take_group_raw())
            }
            "operatorname" => Node::Symbol {
                text: self.take_group_raw(),
                class: AtomClass::Op,
                font: MathFont::Upright,
            },
            "mathop" => {
                let body = self.parse_argument(depth + 1);
                match body {
                    Node::Symbol { text, font, .. } => Node::Symbol {
                        text,
                        class: AtomClass::Op,
                        font,
                    },
                    other => other,
                }
            }
            "mathrm" => self.styled_alphabet(MathFont::Upright, depth),
            "mathbf" | "boldsymbol" | "bm" => self.styled_alphabet(MathFont::Bold, depth),
            "mathit" => self.styled_alphabet(MathFont::Italic, depth),
            "mathbb" => self.styled_alphabet(MathFont::DoubleStruck, depth),
            "mathcal" => self.styled_alphabet(MathFont::Script, depth),
            "mathscr" => self.styled_alphabet(MathFont::Script, depth),
            "mathfrak" => self.styled_alphabet(MathFont::Fraktur, depth),
            "mathsf" => self.styled_alphabet(MathFont::SansSerif, depth),
            "mathtt" => self.styled_alphabet(MathFont::Monospace, depth),
            "displaystyle" => Node::Styled {
                style: StyleShift::Display,
                body: Box::new(self.parse_rest(depth)),
            },
            "textstyle" => Node::Styled {
                style: StyleShift::Text,
                body: Box::new(self.parse_rest(depth)),
            },
            "scriptstyle" => Node::Styled {
                style: StyleShift::Script,
                body: Box::new(self.parse_rest(depth)),
            },
            "scriptscriptstyle" => Node::Styled {
                style: StyleShift::ScriptScript,
                body: Box::new(self.parse_rest(depth)),
            },
            "left" => self.parse_fenced(depth),
            "right" => Node::Row(Vec::new()),
            "begin" => self.parse_environment(depth),
            "end" => {
                let _ = self.take_group_raw();
                Node::Row(Vec::new())
            }
            "overline" => Node::Bar {
                base: Box::new(self.parse_argument(depth + 1)),
                over: true,
            },
            "underline" => Node::Bar {
                base: Box::new(self.parse_argument(depth + 1)),
                over: false,
            },
            "boxed" => Node::Boxed {
                body: Box::new(self.parse_argument(depth + 1)),
            },
            "overset" | "stackrel" => {
                let superscript = Box::new(self.parse_argument(depth + 1));
                let base = Box::new(self.parse_argument(depth + 1));
                Node::Scripts {
                    base,
                    superscript: Some(superscript),
                    subscript: None,
                    limits: Limits::Above,
                }
            }
            "underset" => {
                let subscript = Box::new(self.parse_argument(depth + 1));
                let base = Box::new(self.parse_argument(depth + 1));
                Node::Scripts {
                    base,
                    superscript: None,
                    subscript: Some(subscript),
                    limits: Limits::Above,
                }
            }
            "substack" => {
                let body = self.take_group_raw();
                let rows = split_rows(&body)
                    .into_iter()
                    .flatten()
                    .map(|row| vec![parse(row.trim())])
                    .collect();
                Node::Matrix {
                    rows,
                    delimiters: MatrixDelimiters::None,
                    align: MatrixAlign::Center,
                }
            }
            "pmod" => Node::Fenced {
                left: Some("(".into()),
                body: Box::new(Node::Row(vec![
                    Node::Symbol {
                        text: "mod".into(),
                        class: AtomClass::Op,
                        font: MathFont::Upright,
                    },
                    self.parse_argument(depth + 1),
                ])),
                right: Some(")".into()),
            },
            // Colour is a presentation concern owned by the surrounding text
            // style. Consume its argument so colour names never leak into the
            // formula, while preserving the mathematical body.
            "color" => {
                let _ = self.take_group_raw();
                Node::Row(Vec::new())
            }
            "textcolor" | "colorbox" => {
                let _ = self.take_group_raw();
                self.parse_argument(depth + 1)
            }
            "fcolorbox" => {
                let _ = self.take_group_raw();
                let _ = self.take_group_raw();
                self.parse_argument(depth + 1)
            }
            "overbrace" | "underbrace" => Node::Scripts {
                base: Box::new(Node::Bar {
                    base: Box::new(self.parse_argument(depth + 1)),
                    over: name == "overbrace",
                }),
                superscript: None,
                subscript: None,
                limits: Limits::Above,
            },
            "limits" => Node::LimitsControl(Limits::Above),
            "nolimits" => Node::LimitsControl(Limits::Beside),
            "tag" => Node::Fenced {
                left: Some("(".into()),
                body: Box::new(self.parse_argument(depth + 1)),
                right: Some(")".into()),
            },
            "cancel" | "bcancel" | "xcancel" => self.parse_argument(depth + 1),
            "," => Node::Space(3.0 / 18.0),
            ":" | ">" => Node::Space(4.0 / 18.0),
            ";" => Node::Space(5.0 / 18.0),
            "!" => Node::Space(-3.0 / 18.0),
            " " | "enspace" => Node::Space(0.5),
            "quad" => Node::Space(1.0),
            "qquad" => Node::Space(2.0),
            "\\" | "newline" | "cr" => Node::Newline,
            "big" | "bigl" | "bigr" | "bigm" | "Big" | "Bigl" | "Bigr" | "Bigm" | "bigg"
            | "biggl" | "biggr" | "biggm" | "Bigg" | "Biggl" | "Biggr" | "Biggm" => {
                let delimiter = self.parse_delimiter();
                let class = match name.chars().last() {
                    Some('l') => AtomClass::Open,
                    Some('r') => AtomClass::Close,
                    _ => AtomClass::Ord,
                };
                Node::Symbol {
                    text: delimiter,
                    class,
                    font: MathFont::Upright,
                }
            }
            _ => {
                if let Some(accent) = symbols::accent(name) {
                    return Node::Accent {
                        accent: accent.symbol.to_string(),
                        base: Box::new(self.parse_argument(depth + 1)),
                        stretchy: accent.stretchy,
                    };
                }
                match symbols::command(name) {
                    Some(symbol) => Node::Symbol {
                        text: symbol.text.to_string(),
                        class: symbol.class,
                        font: symbol.font,
                    },
                    None => Node::Text(format!("\\{name}")),
                }
            }
        }
    }

    /// `\mathbf{...}`: re-set every symbol in the argument in one alphabet.
    fn styled_alphabet(&mut self, font: MathFont, depth: usize) -> Node {
        let body = self.parse_argument(depth + 1);
        apply_font(body, font)
    }

    /// The remainder of the current group, for style commands that apply until
    /// the enclosing group ends rather than taking an argument.
    fn parse_rest(&mut self, depth: usize) -> Node {
        self.parse_row(Some('}'), depth + 1)
    }

    fn parse_fenced(&mut self, depth: usize) -> Node {
        let left = self.parse_delimiter();
        let (body, right) = self.parse_until_right(depth);
        Node::Fenced {
            left: (!left.is_empty()).then_some(left),
            body: Box::new(body),
            right: (!right.is_empty()).then_some(right),
        }
    }

    /// Collect atoms up to the matching `\right`, returning its delimiter.
    fn parse_until_right(&mut self, depth: usize) -> (Node, String) {
        let mut items: Vec<Node> = Vec::new();
        let mut right = String::new();
        while let Some(ch) = self.peek() {
            if ch == '\\' {
                let save = self.pos;
                self.bump();
                let start = self.pos;
                while self.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
                    self.bump();
                }
                let name = &self.source[start..self.pos];
                if name == "right" {
                    self.skip_spaces();
                    right = self.parse_delimiter();
                    break;
                }
                self.pos = save;
                if let Some(node) = self.parse_command(depth + 1) {
                    items.push(node);
                }
                continue;
            }
            match ch {
                '^' | '_' => {
                    self.bump();
                    let script = self.parse_argument(depth + 1);
                    let base = items.pop().unwrap_or(Node::Row(Vec::new()));
                    items.push(attach_script(base, script, ch == '^'));
                }
                '{' => {
                    self.bump();
                    let group = self.parse_row(Some('}'), depth + 1);
                    items.push(group_atom(group));
                }
                '}' => break,
                ch if ch.is_whitespace() => {
                    self.bump();
                }
                _ => {
                    self.bump();
                    items.push(self.symbol_from_char(ch));
                }
            }
        }
        (collapse(items), right)
    }

    fn parse_delimiter(&mut self) -> String {
        self.skip_spaces();
        match self.peek() {
            Some('\\') => {
                self.bump();
                let start = self.pos;
                while self.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
                    self.bump();
                }
                if self.pos == start {
                    return self.bump().map(|c| c.to_string()).unwrap_or_default();
                }
                let name = &self.source[start..self.pos];
                symbols::command(name)
                    .map(|s| s.text.to_string())
                    .unwrap_or_default()
            }
            // `\left.` is an *invisible* fence: the null delimiter.
            Some('.') => {
                self.bump();
                String::new()
            }
            Some(ch) => {
                self.bump();
                ch.to_string()
            }
            None => String::new(),
        }
    }

    fn parse_environment(&mut self, depth: usize) -> Node {
        let name = self.take_group_raw();
        let Some((body, consumed)) = find_environment_body(self.source, self.pos, &name) else {
            let rest = self.source[self.pos..].to_string();
            self.pos = self.source.len();
            return Node::Text(rest);
        };
        let body = body.to_string();
        self.pos += consumed;

        let base = name.trim_end_matches('*');
        if matches!(base, "equation" | "displaymath" | "math") {
            return parse(&body);
        }

        let (delimiters, align) = match base {
            "matrix" | "smallmatrix" => (MatrixDelimiters::None, MatrixAlign::Center),
            "pmatrix" => (MatrixDelimiters::Parentheses, MatrixAlign::Center),
            "bmatrix" => (MatrixDelimiters::Brackets, MatrixAlign::Center),
            "Bmatrix" => (MatrixDelimiters::Braces, MatrixAlign::Center),
            "vmatrix" => (MatrixDelimiters::Bars, MatrixAlign::Center),
            "Vmatrix" => (MatrixDelimiters::DoubleBars, MatrixAlign::Center),
            "cases" => (MatrixDelimiters::LeftBrace, MatrixAlign::Left),
            "array" => (MatrixDelimiters::None, MatrixAlign::Center),
            "aligned" | "align" | "split" | "eqnarray" | "alignat" => {
                (MatrixDelimiters::None, MatrixAlign::Alternating)
            }
            "gathered" | "gather" | "multline" => (MatrixDelimiters::None, MatrixAlign::Center),
            _ => return parse(&body),
        };

        let body = if base == "array" || base == "alignat" {
            strip_leading_group(&body).unwrap_or(&body).to_string()
        } else {
            body
        };

        let rows = split_rows(&body)
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(|cell| Parser::new(cell.trim()).parse_row(None, depth + 1))
                    .collect()
            })
            .collect();
        Node::Matrix {
            rows,
            delimiters,
            align,
        }
    }

    fn take_group_raw(&mut self) -> String {
        self.skip_spaces();
        if self.peek() != Some('{') {
            // A bare argument: `\text x`.
            return self.bump().map(|c| c.to_string()).unwrap_or_default();
        }
        self.bump();
        let start = self.pos;
        let mut depth = 1usize;
        while let Some(ch) = self.bump() {
            match ch {
                '\\' => {
                    self.bump();
                }
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return self.source[start..self.pos - 1].to_string();
                    }
                }
                _ => {}
            }
        }
        self.source[start..self.pos].to_string()
    }

    fn take_bracketed(&mut self) -> String {
        let start = self.pos;
        let mut depth = 1usize;
        while let Some(ch) = self.bump() {
            match ch {
                '\\' => {
                    self.bump();
                }
                '[' => depth += 1,
                ']' => {
                    depth -= 1;
                    if depth == 0 {
                        return self.source[start..self.pos - 1].to_string();
                    }
                }
                _ => {}
            }
        }
        self.source[start..self.pos].to_string()
    }

    fn skip_spaces(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.bump();
        }
    }

    fn peek(&self) -> Option<char> {
        self.source[self.pos..].chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.pos += ch.len_utf8();
        Some(ch)
    }
}

/// A braced group is an `Inner` atom: `{a+b}` spaces as one unit, which is why
/// `2^{a+b}` does not get binary-operator gaps around the exponent's plus.
fn group_atom(node: Node) -> Node {
    node
}

fn collapse(mut items: Vec<Node>) -> Node {
    if items.len() == 1 {
        items.pop().unwrap()
    } else {
        Node::Row(items)
    }
}

/// Attach a script to a base, merging with any scripts it already has so
/// `x_i^2` and `x^2_i` produce the same atom.
fn attach_script(base: Node, script: Node, superscript: bool) -> Node {
    match base {
        Node::Scripts {
            base,
            superscript: sup,
            subscript: sub,
            limits,
        } => {
            let (sup, sub) = if superscript {
                (Some(Box::new(script)), sub)
            } else {
                (sup, Some(Box::new(script)))
            };
            Node::Scripts {
                base,
                superscript: sup,
                subscript: sub,
                limits,
            }
        }
        base => {
            let (sup, sub) = if superscript {
                (Some(Box::new(script)), None)
            } else {
                (None, Some(Box::new(script)))
            };
            Node::Scripts {
                base: Box::new(base),
                superscript: sup,
                subscript: sub,
                limits: Limits::Default,
            }
        }
    }
}

fn with_limits(base: Node, limits: Limits) -> Node {
    match base {
        Node::Scripts {
            base,
            superscript,
            subscript,
            ..
        } => Node::Scripts {
            base,
            superscript,
            subscript,
            limits,
        },
        base => Node::Scripts {
            base: Box::new(base),
            superscript: None,
            subscript: None,
            limits,
        },
    }
}

/// Re-set every symbol in a subtree in one math alphabet.
fn apply_font(node: Node, font: MathFont) -> Node {
    match node {
        Node::Symbol { text, class, .. } => Node::Symbol { text, class, font },
        Node::Row(items) => Node::Row(items.into_iter().map(|n| apply_font(n, font)).collect()),
        Node::Scripts {
            base,
            superscript,
            subscript,
            limits,
        } => Node::Scripts {
            base: Box::new(apply_font(*base, font)),
            superscript,
            subscript,
            limits,
        },
        other => other,
    }
}

/// Find the body of `\begin{name}...\end{name}`, honouring nesting.
/// Returns the body and how many bytes to advance past `\end{name}`.
fn find_environment_body<'a>(
    source: &'a str,
    body_start: usize,
    name: &str,
) -> Option<(&'a str, usize)> {
    let begin = format!("\\begin{{{name}}}");
    let end = format!("\\end{{{name}}}");
    let mut search = body_start;
    let mut depth = 1usize;
    while search < source.len() {
        let next_begin = source[search..].find(&begin).map(|o| search + o);
        let next_end = source[search..].find(&end).map(|o| search + o)?;
        match next_begin {
            Some(b) if b < next_end => {
                depth += 1;
                search = b + begin.len();
            }
            _ => {
                depth -= 1;
                if depth == 0 {
                    return Some((
                        &source[body_start..next_end],
                        next_end + end.len() - body_start,
                    ));
                }
                search = next_end + end.len();
            }
        }
    }
    None
}

fn strip_leading_group(source: &str) -> Option<&str> {
    let trimmed = source.trim_start();
    if !trimmed.starts_with('{') {
        return None;
    }
    let mut depth = 0usize;
    for (index, ch) in trimmed.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&trimmed[index + 1..]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Split an environment body on `&` and `\\`, ignoring separators nested in
/// braces or inner environments.
fn split_rows(source: &str) -> Vec<Vec<&str>> {
    let mut rows: Vec<Vec<&str>> = vec![Vec::new()];
    let mut start = 0usize;
    let mut braces = 0usize;
    let mut environments = 0usize;
    let bytes = source.as_bytes();
    let mut pos = 0usize;
    while pos < bytes.len() {
        let rest = &source[pos..];
        if rest.starts_with("\\begin{") {
            environments += 1;
            pos += rest.find('}').map_or(rest.len(), |o| o + 1);
            continue;
        }
        if rest.starts_with("\\end{") {
            environments = environments.saturating_sub(1);
            pos += rest.find('}').map_or(rest.len(), |o| o + 1);
            continue;
        }
        if rest.starts_with("\\\\") && braces == 0 && environments == 0 {
            rows.last_mut().unwrap().push(&source[start..pos]);
            rows.push(Vec::new());
            pos += 2;
            start = pos;
            continue;
        }
        match bytes[pos] {
            b'\\' => {
                // Skip the escaped token so `\{` never counts as a group.
                pos += 1;
                if pos < bytes.len() {
                    pos += 1;
                }
                continue;
            }
            b'{' => braces += 1,
            b'}' => braces = braces.saturating_sub(1),
            b'&' if braces == 0 && environments == 0 => {
                rows.last_mut().unwrap().push(&source[start..pos]);
                start = pos + 1;
            }
            _ => {}
        }
        pos += 1;
    }
    rows.last_mut().unwrap().push(&source[start..]);
    // A trailing `\\` produces an empty final row; drop it so the matrix does
    // not gain a blank line.
    rows.retain(|row| !row.iter().all(|cell| cell.trim().is_empty()));
    if rows.is_empty() {
        rows.push(vec![""]);
    }
    rows
}
