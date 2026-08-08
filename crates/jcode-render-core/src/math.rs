//! Small, dependency-free LaTeX math renderer for text front-ends.
//!
//! This is intentionally a readable terminal renderer rather than a complete
//! TeX engine. It recognizes the constructs language models use most often and
//! preserves unknown commands instead of dropping content.

use unicode_width::UnicodeWidthStr;

const MAX_PARSE_DEPTH: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Expr {
    Sequence(Vec<Expr>),
    Text(String),
    Fraction(Box<Expr>, Box<Expr>),
    Root(Option<Box<Expr>>, Box<Expr>),
    Scripts {
        base: Box<Expr>,
        superscript: Option<Box<Expr>>,
        subscript: Option<Box<Expr>>,
    },
    Matrix {
        rows: Vec<Vec<Expr>>,
        delimiters: MatrixDelimiters,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MatrixDelimiters {
    None,
    Parentheses,
    Brackets,
    Braces,
    LeftBrace,
    Bars,
    DoubleBars,
}

/// Render inline LaTeX as compact Unicode suitable for an ordinary text line.
pub fn render_inline_latex(source: &str) -> String {
    let expr = Parser::new(source).parse();
    let rendered = expr.compact();
    if rendered.trim().is_empty() && !source.trim().is_empty() {
        source.trim().to_string()
    } else {
        rendered
    }
}

/// Render display LaTeX as a small Unicode layout.
pub fn render_display_latex(source: &str) -> Vec<String> {
    let expr = Parser::new(source.trim()).parse();
    let layout = expr.layout();
    if layout.lines.iter().all(|line| line.trim().is_empty()) && !source.trim().is_empty() {
        vec![source.trim().to_string()]
    } else {
        layout.lines
    }
}

impl Expr {
    fn compact(&self) -> String {
        match self {
            Self::Sequence(items) => items.iter().map(Self::compact).collect(),
            Self::Text(text) => text.clone(),
            Self::Fraction(numerator, denominator) => {
                let numerator = compact_group(numerator);
                let denominator = compact_group(denominator);
                format!("{numerator}⁄{denominator}")
            }
            Self::Root(index, radicand) => match index {
                Some(index) => format!(
                    "{}√{}",
                    superscript_or_group(&index.compact()),
                    compact_group(radicand)
                ),
                None => format!("√{}", compact_group(radicand)),
            },
            Self::Scripts {
                base,
                superscript,
                subscript,
            } => {
                let mut out = base.compact();
                if let Some(subscript) = subscript {
                    let script = subscript.compact();
                    out.push_str(&subscript_or_group(&script));
                }
                if let Some(superscript) = superscript {
                    let script = superscript.compact();
                    out.push_str(&superscript_or_group(&script));
                }
                out
            }
            Self::Matrix { rows, delimiters } => {
                let body = rows
                    .iter()
                    .map(|row| row.iter().map(Self::compact).collect::<Vec<_>>().join(", "))
                    .collect::<Vec<_>>()
                    .join("; ");
                let (left, right) = delimiters.compact_pair();
                format!("{left}{body}{right}")
            }
        }
    }

    fn layout(&self) -> MathLayout {
        match self {
            Self::Sequence(items) => MathLayout::horizontal(items.iter().map(Self::layout)),
            Self::Text(text) => MathLayout::text(text.clone()),
            Self::Fraction(numerator, denominator) => {
                MathLayout::fraction(numerator.layout(), denominator.layout())
            }
            Self::Root(index, radicand) => {
                let radical = MathLayout::horizontal([MathLayout::text("√"), radicand.layout()]);
                match index {
                    Some(index) => {
                        let compact = index.compact();
                        match map_script(&compact, true) {
                            Some(script) => {
                                MathLayout::horizontal([MathLayout::text(script), radical])
                            }
                            None => MathLayout::horizontal([
                                MathLayout::scripts(
                                    MathLayout::text(""),
                                    Some(index.layout()),
                                    None,
                                ),
                                radical,
                            ]),
                        }
                    }
                    None => radical,
                }
            }
            Self::Scripts {
                base,
                superscript,
                subscript,
            } => {
                let compact_superscript = superscript
                    .as_deref()
                    .map(Self::compact)
                    .map(|text| map_script(&text, true));
                let compact_subscript = subscript
                    .as_deref()
                    .map(Self::compact)
                    .map(|text| map_script(&text, false));
                let scripts_fit_inline = compact_superscript.as_ref().is_none_or(Option::is_some)
                    && compact_subscript.as_ref().is_none_or(Option::is_some);
                if scripts_fit_inline {
                    let mut scripts = String::new();
                    if let Some(Some(subscript)) = compact_subscript {
                        scripts.push_str(&subscript);
                    }
                    if let Some(Some(superscript)) = compact_superscript {
                        scripts.push_str(&superscript);
                    }
                    MathLayout::horizontal([base.layout(), MathLayout::text(scripts)])
                } else {
                    MathLayout::scripts(
                        base.layout(),
                        superscript.as_deref().map(Self::layout),
                        subscript.as_deref().map(Self::layout),
                    )
                }
            }
            Self::Matrix { rows, delimiters } => MathLayout::matrix(rows, *delimiters),
        }
    }
}

fn compact_group(expr: &Expr) -> String {
    let rendered = expr.compact();
    if matches!(
        expr,
        Expr::Text(_) | Expr::Scripts { .. } | Expr::Root(_, _)
    ) {
        rendered
    } else {
        format!("({rendered})")
    }
}

fn superscript_or_group(text: &str) -> String {
    map_script(text, true).unwrap_or_else(|| format!("^({text})"))
}

fn subscript_or_group(text: &str) -> String {
    map_script(text, false).unwrap_or_else(|| format!("_({text})"))
}

fn map_script(text: &str, superscript: bool) -> Option<String> {
    text.chars()
        .map(|ch| {
            let mapped = if superscript {
                match ch {
                    '0' => '⁰',
                    '1' => '¹',
                    '2' => '²',
                    '3' => '³',
                    '4' => '⁴',
                    '5' => '⁵',
                    '6' => '⁶',
                    '7' => '⁷',
                    '8' => '⁸',
                    '9' => '⁹',
                    '+' => '⁺',
                    '-' => '⁻',
                    '=' => '⁼',
                    '(' => '⁽',
                    ')' => '⁾',
                    'n' => 'ⁿ',
                    'i' => 'ⁱ',
                    _ => return None,
                }
            } else {
                match ch {
                    '0' => '₀',
                    '1' => '₁',
                    '2' => '₂',
                    '3' => '₃',
                    '4' => '₄',
                    '5' => '₅',
                    '6' => '₆',
                    '7' => '₇',
                    '8' => '₈',
                    '9' => '₉',
                    '+' => '₊',
                    '-' => '₋',
                    '=' => '₌',
                    '(' => '₍',
                    ')' => '₎',
                    'a' => 'ₐ',
                    'e' => 'ₑ',
                    'h' => 'ₕ',
                    'i' => 'ᵢ',
                    'j' => 'ⱼ',
                    'k' => 'ₖ',
                    'l' => 'ₗ',
                    'm' => 'ₘ',
                    'n' => 'ₙ',
                    'o' => 'ₒ',
                    'p' => 'ₚ',
                    'r' => 'ᵣ',
                    's' => 'ₛ',
                    't' => 'ₜ',
                    'u' => 'ᵤ',
                    'v' => 'ᵥ',
                    'x' => 'ₓ',
                    _ => return None,
                }
            };
            Some(mapped)
        })
        .collect()
}

struct Parser<'a> {
    source: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str) -> Self {
        Self { source, pos: 0 }
    }

    fn parse(mut self) -> Expr {
        self.parse_sequence(None, 0)
    }

    fn parse_sequence(&mut self, terminator: Option<char>, depth: usize) -> Expr {
        let mut items = Vec::new();
        while let Some(ch) = self.peek() {
            if Some(ch) == terminator {
                self.bump();
                break;
            }
            let mut atom = self.parse_atom(depth);
            let mut superscript = None;
            let mut subscript = None;
            loop {
                match self.peek() {
                    Some('^') => {
                        self.bump();
                        let next = self.parse_argument(depth + 1);
                        superscript = Some(Box::new(append_expr(superscript.take(), next)));
                    }
                    Some('_') => {
                        self.bump();
                        let next = self.parse_argument(depth + 1);
                        subscript = Some(Box::new(append_expr(subscript.take(), next)));
                    }
                    _ => break,
                }
            }
            if superscript.is_some() || subscript.is_some() {
                atom = Expr::Scripts {
                    base: Box::new(atom),
                    superscript,
                    subscript,
                };
            }
            items.push(atom);
        }
        collapse_sequence(items)
    }

    fn parse_atom(&mut self, depth: usize) -> Expr {
        match self.peek() {
            Some('{') => {
                self.bump();
                if depth >= MAX_PARSE_DEPTH {
                    Expr::Text(self.take_balanced_group())
                } else {
                    self.parse_sequence(Some('}'), depth + 1)
                }
            }
            Some('\\') => self.parse_command(depth),
            Some(ch) if ch.is_whitespace() => {
                while self.peek().is_some_and(char::is_whitespace) {
                    self.bump();
                }
                Expr::Text(" ".to_string())
            }
            Some(ch) => {
                self.bump();
                Expr::Text(ch.to_string())
            }
            None => Expr::Text(String::new()),
        }
    }

    fn parse_argument(&mut self, depth: usize) -> Expr {
        self.skip_spaces();
        if self.peek() == Some('{') {
            self.bump();
            if depth >= MAX_PARSE_DEPTH {
                Expr::Text(self.take_balanced_group())
            } else {
                self.parse_sequence(Some('}'), depth + 1)
            }
        } else {
            self.parse_atom(depth)
        }
    }

    fn parse_command(&mut self, depth: usize) -> Expr {
        self.bump();
        let start = self.pos;
        while self.peek().is_some_and(|ch| ch.is_ascii_alphabetic()) {
            self.bump();
        }
        let command = if self.pos == start {
            self.bump().map(|ch| ch.to_string()).unwrap_or_default()
        } else {
            self.source[start..self.pos].to_string()
        };
        let word_command = command.chars().all(|ch| ch.is_ascii_alphabetic());
        let had_separator = word_command && self.peek().is_some_and(char::is_whitespace);
        if word_command {
            self.skip_spaces();
        }

        if depth >= MAX_PARSE_DEPTH {
            return Expr::Text(format!("\\{command}"));
        }

        match command.as_str() {
            "frac" | "dfrac" | "tfrac" => Expr::Fraction(
                Box::new(self.parse_argument(depth + 1)),
                Box::new(self.parse_argument(depth + 1)),
            ),
            "sqrt" => {
                let index = if self.peek() == Some('[') {
                    self.bump();
                    let raw = self.take_bracketed_raw();
                    Some(Box::new(Parser::new(&raw).parse_sequence(None, depth + 1)))
                } else {
                    None
                };
                Expr::Root(index, Box::new(self.parse_argument(depth + 1)))
            }
            "text" | "textrm" | "textsf" | "texttt" | "operatorname" => {
                Expr::Text(self.parse_argument(depth + 1).compact())
            }
            "begin" => self.parse_environment(depth),
            "left" | "right" | "big" | "Big" | "bigg" | "Bigg" | "bigl" | "bigr" | "Bigl"
            | "Bigr" | "biggl" | "biggr" | "Biggl" | "Biggr" => {
                let delimiter = self.parse_delimiter();
                Expr::Text(delimiter)
            }
            "overline" => {
                let body = self.parse_argument(depth + 1).compact();
                Expr::Text(body.chars().flat_map(|ch| [ch, '\u{0305}']).collect())
            }
            "underline" => {
                let body = self.parse_argument(depth + 1).compact();
                Expr::Text(body.chars().flat_map(|ch| [ch, '\u{0332}']).collect())
            }
            "hat" | "widehat" => self.combining_accent('\u{0302}', depth),
            "bar" => self.combining_accent('\u{0304}', depth),
            "vec" => self.combining_accent('\u{20d7}', depth),
            "dot" => self.combining_accent('\u{0307}', depth),
            "ddot" => self.combining_accent('\u{0308}', depth),
            "tilde" | "widetilde" => self.combining_accent('\u{0303}', depth),
            "mathbf" | "mathrm" | "mathit" | "mathsf" | "mathtt" | "mathcal" | "mathbb"
            | "boldsymbol" | "displaystyle" | "scriptstyle" => self.parse_argument(depth + 1),
            "," | ";" | ":" | " " | "quad" => Expr::Text(" ".to_string()),
            "qquad" => Expr::Text("  ".to_string()),
            "!" | "limits" | "nolimits" => Expr::Text(String::new()),
            "\\" => Expr::Text(" ".to_string()),
            _ => {
                let mut rendered =
                    command_symbol(&command).unwrap_or_else(|| format!("\\{command}"));
                // The whitespace after a word command terminates the command
                // name in TeX, but readers expect `\alpha \leq \pi` to keep
                // its spacing, so preserve the separator in the output. Skip
                // it before scripts so `\alpha ^2` still reads as α².
                if had_separator && !matches!(self.peek(), Some('^' | '_')) {
                    rendered.push(' ');
                }
                Expr::Text(rendered)
            }
        }
    }

    fn parse_environment(&mut self, depth: usize) -> Expr {
        let name = self.parse_braced_raw();
        let end_marker = format!("\\end{{{name}}}");
        let body_start = self.pos;
        let Some((body_end, consumed)) = find_environment_end(self.source, body_start, &name)
        else {
            let body = &self.source[body_start..];
            self.pos = self.source.len();
            return Expr::Text(format!("\\begin{{{name}}}{body}"));
        };
        let body = &self.source[body_start..body_end];
        self.pos += consumed;

        if matches!(name.as_str(), "equation" | "equation*" | "displaymath") {
            return Parser::new(body).parse_sequence(None, depth + 1);
        }

        let delimiters = match name.as_str() {
            "matrix" | "smallmatrix" | "array" | "aligned" | "aligned*" | "align" | "align*"
            | "split" | "gather" | "gather*" | "gathered" | "multline" | "multline*"
            | "eqnarray" | "eqnarray*" | "cases" | "cases*" => {
                if matches!(name.as_str(), "cases" | "cases*") {
                    MatrixDelimiters::LeftBrace
                } else {
                    MatrixDelimiters::None
                }
            }
            "pmatrix" => MatrixDelimiters::Parentheses,
            "bmatrix" => MatrixDelimiters::Brackets,
            "Bmatrix" => MatrixDelimiters::Braces,
            "vmatrix" => MatrixDelimiters::Bars,
            "Vmatrix" => MatrixDelimiters::DoubleBars,
            _ => return Expr::Text(format!("\\begin{{{name}}}{body}{end_marker}")),
        };

        let body = if name == "array" {
            strip_leading_braced_group(body).unwrap_or(body)
        } else {
            body
        };
        let rows = split_matrix(body)
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(|cell| Parser::new(cell.trim()).parse_sequence(None, depth + 1))
                    .collect()
            })
            .collect();
        Expr::Matrix { rows, delimiters }
    }

    fn combining_accent(&mut self, accent: char, depth: usize) -> Expr {
        let body = self.parse_argument(depth + 1).compact();
        Expr::Text(body.chars().flat_map(|ch| [ch, accent]).collect())
    }

    fn parse_delimiter(&mut self) -> String {
        if self.peek() == Some('\\') {
            self.bump();
            let start = self.pos;
            while self.peek().is_some_and(|ch| ch.is_ascii_alphabetic()) {
                self.bump();
            }
            if self.pos == start {
                return self.bump().map(|ch| ch.to_string()).unwrap_or_default();
            }
            let name = &self.source[start..self.pos];
            return command_symbol(name).unwrap_or_else(|| format!("\\{name}"));
        }
        self.bump()
            .map(|ch| match ch {
                '.' => String::new(),
                '{' => "{".to_string(),
                '}' => "}".to_string(),
                other => other.to_string(),
            })
            .unwrap_or_default()
    }

    fn parse_braced_raw(&mut self) -> String {
        self.skip_spaces();
        if self.peek() != Some('{') {
            return String::new();
        }
        self.bump();
        self.take_until('}')
    }

    fn take_until(&mut self, terminator: char) -> String {
        let start = self.pos;
        while self.peek().is_some_and(|ch| ch != terminator) {
            self.bump();
        }
        let out = self.source[start..self.pos].to_string();
        if self.peek() == Some(terminator) {
            self.bump();
        }
        out
    }

    fn take_balanced_group(&mut self) -> String {
        let start = self.pos;
        let mut depth = 1usize;
        while let Some(ch) = self.bump() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return self.source[start..self.pos - 1].to_string();
                    }
                }
                '\\' => {
                    self.bump();
                }
                _ => {}
            }
        }
        self.source[start..self.pos].to_string()
    }

    fn take_bracketed_raw(&mut self) -> String {
        let start = self.pos;
        let mut brackets = 1usize;
        let mut braces = 0usize;
        while let Some(ch) = self.bump() {
            match ch {
                '\\' => {
                    self.bump();
                }
                '{' => braces += 1,
                '}' => braces = braces.saturating_sub(1),
                '[' if braces == 0 => brackets += 1,
                ']' if braces == 0 => {
                    brackets -= 1;
                    if brackets == 0 {
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

fn collapse_sequence(mut items: Vec<Expr>) -> Expr {
    if items.len() == 1 {
        items.pop().unwrap()
    } else {
        Expr::Sequence(items)
    }
}

fn append_expr(existing: Option<Box<Expr>>, next: Expr) -> Expr {
    match existing.map(|expr| *expr) {
        Some(Expr::Sequence(mut items)) => {
            items.push(next);
            Expr::Sequence(items)
        }
        Some(existing) => Expr::Sequence(vec![existing, next]),
        None => next,
    }
}

fn find_environment_end(source: &str, body_start: usize, name: &str) -> Option<(usize, usize)> {
    let begin_marker = format!("\\begin{{{name}}}");
    let end_marker = format!("\\end{{{name}}}");
    let mut search = body_start;
    let mut depth = 1usize;
    while search < source.len() {
        let next_begin = source[search..]
            .find(&begin_marker)
            .map(|offset| search + offset);
        let next_end = source[search..]
            .find(&end_marker)
            .map(|offset| search + offset);
        match (next_begin, next_end) {
            (_, None) => return None,
            (Some(begin), Some(end)) if begin < end => {
                depth += 1;
                search = begin + begin_marker.len();
            }
            (_, Some(end)) => {
                depth -= 1;
                if depth == 0 {
                    return Some((end, end + end_marker.len() - body_start));
                }
                search = end + end_marker.len();
            }
        }
    }
    None
}

fn strip_leading_braced_group(source: &str) -> Option<&str> {
    let trimmed = source.trim_start();
    if !trimmed.starts_with('{') {
        return None;
    }
    let mut depth = 0usize;
    for (index, ch) in trimmed.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(&trimmed[index + ch.len_utf8()..]);
                }
            }
            _ => {}
        }
    }
    None
}

fn split_matrix(source: &str) -> Vec<Vec<&str>> {
    let mut rows = vec![Vec::new()];
    let mut start = 0usize;
    let mut depth = 0usize;
    let mut environment_depth = 0usize;
    let bytes = source.as_bytes();
    let mut pos = 0usize;
    while pos < bytes.len() {
        match bytes[pos] {
            b'\\' if source[pos..].starts_with("\\begin{") => {
                environment_depth += 1;
                if let Some(end) = source[pos..].find('}') {
                    pos += end;
                }
            }
            b'\\' if source[pos..].starts_with("\\end{") => {
                environment_depth = environment_depth.saturating_sub(1);
                if let Some(end) = source[pos..].find('}') {
                    pos += end;
                }
            }
            b'\\' if pos + 1 < bytes.len() && bytes[pos + 1] != b'\\' => {
                pos += 1;
            }
            b'{' => depth += 1,
            b'}' => depth = depth.saturating_sub(1),
            b'&' if depth == 0 && environment_depth == 0 => {
                rows.last_mut().unwrap().push(&source[start..pos]);
                start = pos + 1;
            }
            b'\\'
                if depth == 0
                    && environment_depth == 0
                    && pos + 1 < bytes.len()
                    && bytes[pos + 1] == b'\\' =>
            {
                rows.last_mut().unwrap().push(&source[start..pos]);
                rows.push(Vec::new());
                pos += 1;
                start = pos + 1;
                if source[start..].starts_with('[')
                    && let Some(close) = source[start + 1..].find(']')
                {
                    pos = start + close + 1;
                    start = pos + 1;
                }
            }
            _ => {}
        }
        pos += 1;
    }
    rows.last_mut().unwrap().push(&source[start..]);
    if rows.len() > 1
        && rows
            .last()
            .is_some_and(|row| row.iter().all(|cell| cell.trim().is_empty()))
    {
        rows.pop();
    }
    rows
}

fn command_symbol(command: &str) -> Option<String> {
    let symbol = match command {
        "alpha" => "α",
        "beta" => "β",
        "gamma" => "γ",
        "delta" => "δ",
        "epsilon" | "varepsilon" => "ε",
        "zeta" => "ζ",
        "eta" => "η",
        "theta" => "θ",
        "vartheta" => "ϑ",
        "iota" => "ι",
        "kappa" => "κ",
        "lambda" => "λ",
        "mu" => "μ",
        "nu" => "ν",
        "xi" => "ξ",
        "pi" => "π",
        "varpi" => "ϖ",
        "rho" => "ρ",
        "varrho" => "ϱ",
        "sigma" => "σ",
        "varsigma" => "ς",
        "tau" => "τ",
        "upsilon" => "υ",
        "phi" => "φ",
        "varphi" => "ϕ",
        "chi" => "χ",
        "psi" => "ψ",
        "omega" => "ω",
        "Gamma" => "Γ",
        "Delta" => "Δ",
        "Theta" => "Θ",
        "Lambda" => "Λ",
        "Xi" => "Ξ",
        "Pi" => "Π",
        "Sigma" => "Σ",
        "Upsilon" => "Υ",
        "Phi" => "Φ",
        "Psi" => "Ψ",
        "Omega" => "Ω",
        "sum" => "∑",
        "prod" => "∏",
        "coprod" => "∐",
        "int" => "∫",
        "iint" => "∬",
        "iiint" => "∭",
        "oint" => "∮",
        "partial" => "∂",
        "nabla" => "∇",
        "infty" => "∞",
        "ell" => "ℓ",
        "hbar" => "ℏ",
        "times" => "×",
        "div" => "÷",
        "cdot" => "·",
        "circ" => "∘",
        "pm" => "±",
        "mp" => "∓",
        "ast" => "∗",
        "star" => "⋆",
        "le" | "leq" => "≤",
        "ge" | "geq" => "≥",
        "ne" | "neq" => "≠",
        "approx" => "≈",
        "sim" => "∼",
        "simeq" => "≃",
        "equiv" => "≡",
        "propto" => "∝",
        "ll" => "≪",
        "gg" => "≫",
        "in" => "∈",
        "notin" => "∉",
        "ni" => "∋",
        "subset" => "⊂",
        "supset" => "⊃",
        "subseteq" => "⊆",
        "supseteq" => "⊇",
        "cup" => "∪",
        "cap" => "∩",
        "setminus" => "∖",
        "emptyset" | "varnothing" => "∅",
        "forall" => "∀",
        "exists" => "∃",
        "nexists" => "∄",
        "neg" | "lnot" => "¬",
        "land" | "wedge" => "∧",
        "lor" | "vee" => "∨",
        "oplus" => "⊕",
        "otimes" => "⊗",
        "vdash" => "⊢",
        "models" => "⊨",
        "to" | "rightarrow" => "→",
        "leftarrow" => "←",
        "leftrightarrow" => "↔",
        "Rightarrow" | "implies" => "⇒",
        "Leftarrow" => "⇐",
        "Leftrightarrow" | "iff" => "⇔",
        "mapsto" => "↦",
        "uparrow" => "↑",
        "downarrow" => "↓",
        "ldots" | "dots" => "…",
        "cdots" => "⋯",
        "vdots" => "⋮",
        "ddots" => "⋱",
        "angle" => "∠",
        "degree" => "°",
        "prime" => "′",
        "perp" => "⊥",
        "parallel" => "∥",
        "mid" => "∣",
        "vert" => "|",
        "Vert" => "‖",
        "langle" => "⟨",
        "rangle" => "⟩",
        "lceil" => "⌈",
        "rceil" => "⌉",
        "lfloor" => "⌊",
        "rfloor" => "⌋",
        "%" => "%",
        "$" => "$",
        "#" => "#",
        "&" => "&",
        "_" => "_",
        "{" | "lbrace" => "{",
        "}" | "rbrace" => "}",
        "sin" => "sin",
        "cos" => "cos",
        "tan" => "tan",
        "sec" => "sec",
        "csc" => "csc",
        "cot" => "cot",
        "log" => "log",
        "ln" => "ln",
        "exp" => "exp",
        "lim" => "lim",
        "min" => "min",
        "max" => "max",
        "det" => "det",
        "gcd" => "gcd",
        "mod" => "mod",
        _ => return None,
    };
    Some(symbol.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MathLayout {
    lines: Vec<String>,
    baseline: usize,
}

impl MathLayout {
    fn text(text: impl Into<String>) -> Self {
        let text = text.into();
        let lines = text.split('\n').map(ToString::to_string).collect();
        Self { lines, baseline: 0 }
    }

    fn width(&self) -> usize {
        self.lines
            .iter()
            .map(|line| line.width())
            .max()
            .unwrap_or(0)
    }

    fn horizontal(items: impl IntoIterator<Item = Self>) -> Self {
        let items: Vec<Self> = items.into_iter().collect();
        if items.is_empty() {
            return Self::text("");
        }
        let above = items.iter().map(|item| item.baseline).max().unwrap_or(0);
        let below = items
            .iter()
            .map(|item| item.lines.len().saturating_sub(item.baseline + 1))
            .max()
            .unwrap_or(0);
        let height = above + 1 + below;
        let mut lines = vec![String::new(); height];
        for item in items {
            let width = item.width();
            let offset = above - item.baseline;
            for (row, line) in lines.iter_mut().enumerate() {
                if let Some(content) = row.checked_sub(offset).and_then(|r| item.lines.get(r)) {
                    line.push_str(&pad_right(content, width));
                } else {
                    line.push_str(&" ".repeat(width));
                }
            }
        }
        Self {
            lines,
            baseline: above,
        }
    }

    fn fraction(numerator: Self, denominator: Self) -> Self {
        let width = numerator.width().max(denominator.width()).max(1) + 2;
        let mut lines = numerator
            .lines
            .iter()
            .map(|line| center(line, width))
            .collect::<Vec<_>>();
        lines.push("─".repeat(width));
        let baseline = lines.len() - 1;
        lines.extend(denominator.lines.iter().map(|line| center(line, width)));
        Self { lines, baseline }
    }

    fn scripts(base: Self, superscript: Option<Self>, subscript: Option<Self>) -> Self {
        if superscript.is_none() && subscript.is_none() {
            return base;
        }
        let script_width = superscript
            .as_ref()
            .map(Self::width)
            .unwrap_or(0)
            .max(subscript.as_ref().map(Self::width).unwrap_or(0));
        let superscript_height = superscript.as_ref().map(|s| s.lines.len()).unwrap_or(0);
        let base_width = base.width();
        let mut lines = Vec::new();
        if let Some(superscript) = superscript {
            for line in superscript.lines {
                lines.push(format!("{}{line}", " ".repeat(base_width)));
            }
        }
        let baseline = superscript_height + base.baseline;
        for (index, line) in base.lines.into_iter().enumerate() {
            let mut combined = pad_right(&line, base_width);
            if index == base.baseline {
                combined.push_str(&" ".repeat(script_width));
            }
            lines.push(combined);
        }
        if let Some(subscript) = subscript {
            for line in subscript.lines {
                lines.push(format!("{}{line}", " ".repeat(base_width)));
            }
        }
        Self { lines, baseline }
    }

    fn matrix(rows: &[Vec<Expr>], delimiters: MatrixDelimiters) -> Self {
        if rows.is_empty() {
            return Self::text("");
        }
        let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
        let mut widths = vec![0usize; columns];
        let rendered: Vec<Vec<String>> = rows
            .iter()
            .map(|row| {
                row.iter()
                    .enumerate()
                    .map(|(column, cell)| {
                        let text = cell.compact();
                        widths[column] = widths[column].max(text.width());
                        text
                    })
                    .collect()
            })
            .collect();
        let height = rendered.len();
        let (left, right) = delimiters.glyphs(height);
        let lines = rendered
            .into_iter()
            .enumerate()
            .map(|(row_index, row)| {
                let cells = (0..columns)
                    .map(|column| {
                        center(
                            row.get(column).map(String::as_str).unwrap_or(""),
                            widths[column],
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("  ");
                format!("{} {cells} {}", left[row_index], right[row_index])
            })
            .collect();
        Self {
            lines,
            baseline: height / 2,
        }
    }
}

impl MatrixDelimiters {
    fn compact_pair(self) -> (&'static str, &'static str) {
        match self {
            Self::None => ("", ""),
            Self::Parentheses => ("(", ")"),
            Self::Brackets => ("[", "]"),
            Self::Braces => ("{", "}"),
            Self::LeftBrace => ("{", ""),
            Self::Bars => ("|", "|"),
            Self::DoubleBars => ("‖", "‖"),
        }
    }

    fn glyphs(self, height: usize) -> (Vec<&'static str>, Vec<&'static str>) {
        let repeated = |glyph| vec![glyph; height];
        if height <= 1 {
            let (left, right) = self.compact_pair();
            return (repeated(left), repeated(right));
        }
        match self {
            Self::None => (repeated(""), repeated("")),
            Self::Parentheses => {
                let mut left = repeated("⎜");
                let mut right = repeated("⎟");
                left[0] = "⎛";
                right[0] = "⎞";
                left[height - 1] = "⎝";
                right[height - 1] = "⎠";
                (left, right)
            }
            Self::Brackets => {
                let mut left = repeated("⎢");
                let mut right = repeated("⎥");
                left[0] = "⎡";
                right[0] = "⎤";
                left[height - 1] = "⎣";
                right[height - 1] = "⎦";
                (left, right)
            }
            Self::Braces => {
                let mut left = repeated("⎪");
                let mut right = repeated("⎪");
                left[0] = "⎧";
                right[0] = "⎫";
                left[height / 2] = "⎨";
                right[height / 2] = "⎬";
                left[height - 1] = "⎩";
                right[height - 1] = "⎭";
                (left, right)
            }
            Self::LeftBrace => {
                let mut left = repeated("⎪");
                left[0] = "⎧";
                left[height / 2] = "⎨";
                left[height - 1] = "⎩";
                (left, repeated(""))
            }
            Self::Bars => (repeated("│"), repeated("│")),
            Self::DoubleBars => (repeated("‖"), repeated("‖")),
        }
    }
}

fn center(text: &str, width: usize) -> String {
    let padding = width.saturating_sub(text.width());
    let left = padding / 2;
    let right = padding - left;
    format!("{}{text}{}", " ".repeat(left), " ".repeat(right))
}

fn pad_right(text: &str, width: usize) -> String {
    format!("{text}{}", " ".repeat(width.saturating_sub(text.width())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_common_inline_notation() {
        assert_eq!(render_inline_latex(r"E = mc^2"), "E = mc²");
        assert_eq!(render_inline_latex(r"\alpha + \beta \leq \pi"), "α + β ≤ π");
        assert_eq!(render_inline_latex(r"\alpha ^2 + \beta _1"), "α² + β₁");
        assert_eq!(render_inline_latex(r"x_{10}"), "x₁₀");
        assert_eq!(render_inline_latex(r"x_i^2"), "xᵢ²");
        assert_eq!(render_inline_latex(r"x^2^3"), "x²³");
        assert_eq!(render_inline_latex(r"\frac{a+b}{c}"), "(a+b)⁄c");
        assert_eq!(render_inline_latex(r"\sqrt[3]{x}"), "³√x");
        assert_eq!(render_inline_latex(r"\sin x + \log n"), "sin x + log n");
        assert_eq!(render_inline_latex(r"\sqrt[\sqrt[2]{y}]{x}"), "^(²√y)√x");
    }

    #[test]
    fn renders_fraction_as_display_layout() {
        assert_eq!(
            render_display_latex(r"\frac{x+1}{y}"),
            vec![" x+1 ", "─────", "  y  "]
        );
    }

    #[test]
    fn renders_matrix_with_tall_brackets() {
        assert_eq!(
            render_display_latex(r"\begin{bmatrix}a & b \\ c & d\end{bmatrix}"),
            vec!["⎡ a  b ⎤", "⎣ c  d ⎦"]
        );
    }

    #[test]
    fn preserves_unknown_commands() {
        assert_eq!(render_inline_latex(r"\custom{x}"), r"\customx");
    }

    #[test]
    fn malformed_input_stays_visible_without_panicking() {
        for source in [
            r"\frac{x",
            r"\sqrt[3{x}",
            r"\begin{bmatrix}a & b",
            r"x^{y_{z}",
            r"\left\{x\right",
            "α_{😀",
        ] {
            let inline = render_inline_latex(source);
            let display = render_display_latex(source);
            assert!(
                !inline.is_empty() || !display.is_empty(),
                "malformed source disappeared: {source:?}"
            );
        }
    }

    #[test]
    fn deeply_nested_input_is_bounded_and_does_not_overflow_the_stack() {
        let source = format!("{}x{}", "{".repeat(20_000), "}".repeat(20_000));
        let repeated_scripts = format!("x{}", "^1".repeat(20_000));
        std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(move || {
                assert!(render_inline_latex(&source).contains('x'));
                assert!(
                    render_display_latex(&source)
                        .iter()
                        .any(|line| line.contains('x'))
                );
                assert_eq!(
                    render_inline_latex(&repeated_scripts).chars().next(),
                    Some('x')
                );
            })
            .expect("spawn bounded-stack renderer")
            .join()
            .expect("deeply nested rendering must not panic");
    }

    #[test]
    fn renders_braced_matrices_and_cases_with_their_distinct_delimiters() {
        let matrix = render_display_latex(r"\begin{Bmatrix}a & b \\ c & d\end{Bmatrix}");
        assert!(
            matrix
                .first()
                .is_some_and(|line| line.starts_with('⎧') && line.ends_with('⎫'))
        );
        assert!(
            matrix
                .last()
                .is_some_and(|line| line.starts_with('⎩') && line.ends_with('⎭'))
        );

        let cases = render_display_latex(r"\begin{cases}x & x>0 \\ -x & x<0\end{cases}");
        assert!(cases.iter().all(|line| !line.ends_with(['⎫', '⎬', '⎭'])));
    }

    #[test]
    fn matrix_scanner_respects_nested_environments_escapes_and_spacing() {
        assert_eq!(
            render_inline_latex(
                r"\begin{bmatrix}\begin{bmatrix}a & b \\ c & d\end{bmatrix}\end{bmatrix}"
            ),
            "[[a, b; c, d]]"
        );
        assert_eq!(
            render_inline_latex(r"\begin{bmatrix}a \& b & c\end{bmatrix}"),
            "[a & b, c]"
        );
        assert_eq!(
            render_inline_latex(r"\begin{array}{cc}a & b\end{array}"),
            "a, b"
        );
        assert_eq!(
            render_inline_latex(r"\begin{bmatrix}a \\[4pt] b \\\end{bmatrix}"),
            "[a; b]"
        );
    }

    #[test]
    fn unmatched_environment_is_preserved_without_inventing_an_end_marker() {
        let source = r"\begin{bmatrix}a & b";
        assert_eq!(render_inline_latex(source), source);
    }

    #[test]
    fn latexish_fuzz_corpus_never_panics_or_loses_all_visible_content() {
        const ALPHABET: &[char] = &[
            '\\', '{', '}', '[', ']', '^', '_', '&', '$', ' ', '\n', 'x', '7', 'α', '界',
        ];
        let mut state = 0x9e37_79b9_u32;
        for case in 0..256 {
            let mut source = String::new();
            for _ in 0..(32 + case % 96) {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                source.push(ALPHABET[state as usize % ALPHABET.len()]);
            }
            let inline = render_inline_latex(&source);
            let display = render_display_latex(&source);
            if !source.trim().is_empty() {
                assert!(!inline.trim().is_empty(), "case {case}: {source:?}");
                assert!(
                    display.iter().any(|line| !line.trim().is_empty()),
                    "case {case}: {source:?}"
                );
            }
        }
    }
}
