use super::*;

const MAX_EXPRESSION_BYTES: usize = 512;
const MAX_PARSER_DEPTH: usize = 64;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum CalculatorBase {
    Binary,
    Octal,
    #[default]
    Decimal,
    Hexadecimal,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum CalculatorTheme {
    Dark,
    #[default]
    Light,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum CalculatorMode {
    #[default]
    Scientific,
    Programmer,
}

#[derive(Clone, Debug)]
pub(crate) struct CalculatorHistoryEntry {
    pub(crate) expression: String,
    pub(crate) result: String,
}

fn in_hex_literal(expression: &str) -> bool {
    let tail = expression.rsplit_once("0x").map(|(_, tail)| tail);
    tail.is_some_and(|tail| tail.chars().all(|character| character.is_ascii_hexdigit()))
}

#[derive(Clone, Debug, Default)]
pub struct CalculatorOverlay {
    pub(crate) expression: String,
    pub(crate) result: Option<f64>,
    pub(crate) error: Option<String>,
    pub(crate) base: CalculatorBase,
    pub(crate) theme: CalculatorTheme,
    pub(crate) mode: CalculatorMode,
    pub(crate) degrees: bool,
    pub(crate) history: Vec<CalculatorHistoryEntry>,
    pub(crate) history_visible: bool,
    pub(crate) history_query: String,
    pub(crate) history_cursor: usize,
    just_evaluated: bool,
}

impl CalculatorOverlay {
    pub fn formatted_result(&self) -> Option<String> {
        let value = self.result?;
        if !is_integer_result(value) {
            return Some(format_float(value));
        }
        let integer = value as i64;
        Some(format_integer(integer, self.base))
    }

    pub fn conversion_summary(&self) -> Option<String> {
        let value = self.result?;
        if !is_integer_result(value) {
            return None;
        }
        let integer = value as i64;
        Some(format!(
            "BIN {}   OCT {}   DEC {}   HEX {}",
            format_radix(integer, 2, "0b"),
            format_radix(integer, 8, "0o"),
            integer,
            format_radix(integer, 16, "0x")
        ))
    }

    pub(crate) fn filtered_history_indices(&self) -> Vec<usize> {
        let query = self.history_query.to_lowercase();
        self.history
            .iter()
            .enumerate()
            .rev()
            .filter(|(_, entry)| {
                query.is_empty()
                    || entry.expression.to_lowercase().contains(&query)
                    || entry.result.to_lowercase().contains(&query)
            })
            .map(|(index, _)| index)
            .collect()
    }
}

fn is_integer_result(value: f64) -> bool {
    value.fract() == 0.0 && value.abs() <= ((1u64 << 53) - 1) as f64
}

fn format_integer(value: i64, base: CalculatorBase) -> String {
    match base {
        CalculatorBase::Binary => format_radix(value, 2, "0b"),
        CalculatorBase::Octal => format_radix(value, 8, "0o"),
        CalculatorBase::Decimal => value.to_string(),
        CalculatorBase::Hexadecimal => format_radix(value, 16, "0x"),
    }
}

fn format_radix(value: i64, radix: u32, prefix: &str) -> String {
    let digits = match (radix, value.is_negative()) {
        (2, true) => format!("{:b}", value.unsigned_abs()),
        (8, true) => format!("{:o}", value.unsigned_abs()),
        (16, true) => format!("{:X}", value.unsigned_abs()),
        (2, false) => format!("{value:b}"),
        (8, false) => format!("{value:o}"),
        (16, false) => format!("{value:X}"),
        _ => value.to_string(),
    };
    if value.is_negative() {
        format!("-{prefix}{digits}")
    } else {
        format!("{prefix}{digits}")
    }
}

fn format_float(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else if value.abs() < 1e-9 {
        format!("{value:.12e}")
    } else {
        format!("{value:.12}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    }
}

impl App {
    pub(super) fn handle_calculator_command(&mut self, input: &str) -> bool {
        let Some(rest) = input
            .strip_prefix("/calc")
            .filter(|rest| rest.is_empty() || rest.chars().next().is_some_and(char::is_whitespace))
        else {
            return false;
        };
        let argument = rest.trim();
        if argument.eq_ignore_ascii_case("close") {
            self.calculator_overlay = None;
        } else if argument.eq_ignore_ascii_case("clear") {
            let overlay = self
                .calculator_overlay
                .get_or_insert_with(CalculatorOverlay::default);
            clear_calculator(overlay);
        } else {
            let mut overlay = self.calculator_overlay.take().unwrap_or_default();
            if !argument.is_empty() {
                overlay.expression = argument.to_string();
                evaluate_overlay(&mut overlay);
            }
            self.calculator_overlay = Some(overlay);
        }
        true
    }

    pub(super) fn handle_calculator_key(&mut self, code: KeyCode) -> Result<()> {
        let Some(mut overlay) = self.calculator_overlay.take() else {
            return Ok(());
        };
        if overlay.history_visible {
            match code {
                KeyCode::Esc | KeyCode::F(9) => {
                    overlay.history_visible = false;
                    overlay.history_query.clear();
                    overlay.history_cursor = 0;
                }
                KeyCode::Backspace => {
                    overlay.history_query.pop();
                    overlay.history_cursor = 0;
                }
                KeyCode::Up => {
                    overlay.history_cursor = overlay.history_cursor.saturating_sub(1);
                }
                KeyCode::Down => {
                    let count = overlay.filtered_history_indices().len();
                    if count > 0 {
                        overlay.history_cursor = (overlay.history_cursor + 1).min(count - 1);
                    }
                }
                KeyCode::Enter => {
                    if let Some(index) = overlay
                        .filtered_history_indices()
                        .get(overlay.history_cursor)
                    {
                        overlay.expression = overlay.history[*index].expression.clone();
                        overlay.history_visible = false;
                        overlay.history_query.clear();
                        overlay.history_cursor = 0;
                        update_calculator_preview(&mut overlay);
                        overlay.just_evaluated = true;
                    }
                }
                KeyCode::Char(character) if !character.is_control() => {
                    overlay.history_query.push(character);
                    overlay.history_cursor = 0;
                }
                _ => {}
            }
            self.calculator_overlay = Some(overlay);
            return Ok(());
        }
        match code {
            KeyCode::Esc | KeyCode::F(10) => return Ok(()),
            KeyCode::Enter | KeyCode::Char('=') => evaluate_overlay(&mut overlay),
            KeyCode::Backspace => {
                overlay.expression.pop();
                overlay.result = None;
                overlay.error = None;
                overlay.just_evaluated = false;
                update_calculator_preview(&mut overlay);
            }
            KeyCode::Delete => clear_calculator(&mut overlay),
            KeyCode::Char('C') if in_hex_literal(&overlay.expression) => {
                append_calculator_text(&mut overlay, "c");
            }
            KeyCode::Char('C') => clear_calculator(&mut overlay),
            KeyCode::F(1) => overlay.base = CalculatorBase::Binary,
            KeyCode::F(2) => overlay.base = CalculatorBase::Octal,
            KeyCode::F(3) => overlay.base = CalculatorBase::Decimal,
            KeyCode::F(4) => overlay.base = CalculatorBase::Hexadecimal,
            KeyCode::F(5) => append_calculator_text(&mut overlay, "sin("),
            KeyCode::F(6) => append_calculator_text(&mut overlay, "cos("),
            KeyCode::F(7) => append_calculator_text(&mut overlay, "tan("),
            KeyCode::F(8) => append_calculator_text(&mut overlay, "sqrt("),
            KeyCode::F(9) => {
                overlay.history_visible = true;
                overlay.history_query.clear();
                overlay.history_cursor = 0;
            }
            KeyCode::Tab => {
                overlay.mode = match overlay.mode {
                    CalculatorMode::Scientific => CalculatorMode::Programmer,
                    CalculatorMode::Programmer => CalculatorMode::Scientific,
                };
            }
            KeyCode::Char('m') => {
                overlay.theme = match overlay.theme {
                    CalculatorTheme::Dark => CalculatorTheme::Light,
                    CalculatorTheme::Light => CalculatorTheme::Dark,
                };
            }
            KeyCode::Char('s') => append_calculator_text(&mut overlay, "sin("),
            KeyCode::Char('c') if in_hex_literal(&overlay.expression) => {
                append_calculator_text(&mut overlay, "c");
            }
            KeyCode::Char('c') => append_calculator_text(&mut overlay, "cos("),
            KeyCode::Char('t') => append_calculator_text(&mut overlay, "tan("),
            KeyCode::Char('l') => append_calculator_text(&mut overlay, "ln("),
            KeyCode::Char('g') => append_calculator_text(&mut overlay, "log("),
            KeyCode::Char('r') => append_calculator_text(&mut overlay, "sqrt("),
            KeyCode::Char('p') => append_calculator_text(&mut overlay, "pi"),
            KeyCode::Char('e') if in_hex_literal(&overlay.expression) => {
                append_calculator_text(&mut overlay, "e");
            }
            KeyCode::Char('e') => append_calculator_text(&mut overlay, "e"),
            KeyCode::Char('b') if in_hex_literal(&overlay.expression) => {
                append_calculator_text(&mut overlay, "b");
            }
            KeyCode::Char('b') => append_calculator_text(&mut overlay, "0b"),
            KeyCode::Char('o') => append_calculator_text(&mut overlay, "0o"),
            KeyCode::Char('h') => append_calculator_text(&mut overlay, "0x"),
            KeyCode::Char('y') => append_calculator_text(&mut overlay, " xor "),
            KeyCode::Char('x') => append_calculator_text(&mut overlay, "*"),
            KeyCode::Char('d') if in_hex_literal(&overlay.expression) => {
                append_calculator_text(&mut overlay, "d");
            }
            KeyCode::Char('d') => {
                overlay.degrees = !overlay.degrees;
                update_calculator_preview(&mut overlay);
            }
            KeyCode::Char('a') if in_hex_literal(&overlay.expression) => {
                append_calculator_text(&mut overlay, "a");
            }
            KeyCode::Char('f') if in_hex_literal(&overlay.expression) => {
                append_calculator_text(&mut overlay, "f");
            }
            KeyCode::Char('<') => append_calculator_text(&mut overlay, "<<"),
            KeyCode::Char('>') => append_calculator_text(&mut overlay, ">>"),
            KeyCode::Char(character)
                if character.is_ascii_digit()
                    || matches!(
                        character,
                        '.' | '+' | '-' | '*' | '/' | '%' | '^' | '&' | '|' | '~' | '(' | ')'
                    ) =>
            {
                append_calculator_text(&mut overlay, &character.to_string());
            }
            KeyCode::Char(character) if ('A'..='F').contains(&character) => {
                append_calculator_text(&mut overlay, &character.to_ascii_lowercase().to_string());
            }
            _ => {}
        }
        self.calculator_overlay = Some(overlay);
        Ok(())
    }
}

fn clear_calculator(overlay: &mut CalculatorOverlay) {
    overlay.expression.clear();
    overlay.result = None;
    overlay.error = None;
    overlay.just_evaluated = false;
}

fn append_calculator_text(overlay: &mut CalculatorOverlay, text: &str) {
    if overlay.just_evaluated
        && (text
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_digit())
            || matches!(
                text,
                "sin("
                    | "cos("
                    | "tan("
                    | "ln("
                    | "log("
                    | "sqrt("
                    | "pi"
                    | "e"
                    | "0b"
                    | "0o"
                    | "0x"
            ))
    {
        overlay.expression.clear();
    }
    overlay.expression.push_str(text);
    overlay.result = None;
    overlay.error = None;
    overlay.just_evaluated = false;
    update_calculator_preview(overlay);
}

fn update_calculator_preview(overlay: &mut CalculatorOverlay) {
    if overlay.expression.trim().is_empty() {
        overlay.result = None;
        return;
    }
    overlay.result = evaluate_expression(&overlay.expression, overlay.degrees).ok();
    overlay.error = None;
}

fn evaluate_overlay(overlay: &mut CalculatorOverlay) {
    match evaluate_expression(&overlay.expression, overlay.degrees) {
        Ok(value) => {
            overlay.result = Some(value);
            overlay.error = None;
            overlay.just_evaluated = true;
            if !overlay.expression.trim().is_empty() {
                let result = overlay
                    .formatted_result()
                    .unwrap_or_else(|| format_float(value));
                overlay.history.push(CalculatorHistoryEntry {
                    expression: overlay.expression.clone(),
                    result,
                });
                if overlay.history.len() > 100 {
                    overlay.history.remove(0);
                }
            }
        }
        Err(error) => {
            overlay.result = None;
            overlay.error = Some(error);
            overlay.just_evaluated = false;
        }
    }
}

pub(crate) fn evaluate_expression(expression: &str, degrees: bool) -> Result<f64, String> {
    if expression.trim().is_empty() {
        return Err("Enter an expression".to_string());
    }
    if expression.len() > MAX_EXPRESSION_BYTES {
        return Err(format!(
            "Expression is too long (maximum {MAX_EXPRESSION_BYTES} bytes)"
        ));
    }
    let normalized = expression
        .replace('π', "pi")
        .replace('×', "*")
        .replace('÷', "/")
        .replace('−', "-");
    let mut parser = Parser {
        input: normalized.as_bytes(),
        position: 0,
        degrees,
        depth: 0,
    };
    let value = parser.parse_bit_or()?;
    parser.skip_whitespace();
    if parser.position != parser.input.len() {
        return Err(format!("Unexpected token near '{}'", parser.remaining()));
    }
    if !value.is_finite() {
        return Err("Result is outside the finite number range".to_string());
    }
    Ok(value)
}

struct Parser<'a> {
    input: &'a [u8],
    position: usize,
    degrees: bool,
    depth: usize,
}

impl Parser<'_> {
    fn parse_nested(
        &mut self,
        parse: impl FnOnce(&mut Self) -> Result<f64, String>,
    ) -> Result<f64, String> {
        if self.depth >= MAX_PARSER_DEPTH {
            return Err(format!(
                "Expression nesting exceeds the limit of {MAX_PARSER_DEPTH}"
            ));
        }
        self.depth += 1;
        let result = parse(self);
        self.depth -= 1;
        result
    }

    fn skip_whitespace(&mut self) {
        while self
            .input
            .get(self.position)
            .is_some_and(|byte| byte.is_ascii_whitespace())
        {
            self.position += 1;
        }
    }

    fn remaining(&self) -> String {
        String::from_utf8_lossy(&self.input[self.position..])
            .chars()
            .take(12)
            .collect()
    }

    fn consume(&mut self, token: &[u8]) -> bool {
        self.skip_whitespace();
        if self.input[self.position..].starts_with(token) {
            self.position += token.len();
            true
        } else {
            false
        }
    }

    fn consume_word(&mut self, word: &[u8]) -> bool {
        self.skip_whitespace();
        if !self.input[self.position..].starts_with(word) {
            return false;
        }
        let end = self.position + word.len();
        if self
            .input
            .get(end)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        {
            return false;
        }
        self.position = end;
        true
    }

    fn parse_bit_or(&mut self) -> Result<f64, String> {
        let mut value = self.parse_bit_xor()?;
        while self.consume(b"|") {
            value = (as_integer(value)? | as_integer(self.parse_bit_xor()?)?) as f64;
        }
        Ok(value)
    }

    fn parse_bit_xor(&mut self) -> Result<f64, String> {
        let mut value = self.parse_bit_and()?;
        while self.consume_word(b"xor") {
            value = (as_integer(value)? ^ as_integer(self.parse_bit_and()?)?) as f64;
        }
        Ok(value)
    }

    fn parse_bit_and(&mut self) -> Result<f64, String> {
        let mut value = self.parse_shift()?;
        while self.consume(b"&") {
            value = (as_integer(value)? & as_integer(self.parse_shift()?)?) as f64;
        }
        Ok(value)
    }

    fn parse_shift(&mut self) -> Result<f64, String> {
        let mut value = self.parse_add_sub()?;
        loop {
            if self.consume(b"<<") {
                let shift = as_integer(self.parse_add_sub()?)?;
                if !(0..64).contains(&shift) {
                    return Err("Shift count must be between 0 and 63".to_string());
                }
                let shifted = as_integer(value)?.wrapping_shl(shift as u32);
                if shifted.unsigned_abs() > (1u64 << 53) - 1 {
                    return Err("Bitwise results are limited to exact 53-bit integers".to_string());
                }
                value = shifted as f64;
            } else if self.consume(b">>") {
                let shift = as_integer(self.parse_add_sub()?)?;
                if !(0..64).contains(&shift) {
                    return Err("Shift count must be between 0 and 63".to_string());
                }
                value = (as_integer(value)? >> shift) as f64;
            } else {
                return Ok(value);
            }
        }
    }

    fn parse_add_sub(&mut self) -> Result<f64, String> {
        let mut value = self.parse_mul_div()?;
        loop {
            if self.consume(b"+") {
                value += self.parse_mul_div()?;
            } else if self.consume(b"-") {
                value -= self.parse_mul_div()?;
            } else {
                return Ok(value);
            }
        }
    }

    fn parse_mul_div(&mut self) -> Result<f64, String> {
        let mut value = self.parse_unary()?;
        loop {
            if self.consume(b"*") {
                value *= self.parse_unary()?;
            } else if self.consume(b"/") {
                value /= self.parse_unary()?;
            } else if self.consume(b"%") {
                value %= self.parse_unary()?;
            } else {
                return Ok(value);
            }
        }
    }

    fn parse_unary(&mut self) -> Result<f64, String> {
        if self.consume(b"+") {
            self.parse_nested(|parser| parser.parse_unary())
        } else if self.consume(b"-") {
            Ok(-self.parse_nested(|parser| parser.parse_unary())?)
        } else if self.consume(b"~") {
            Ok((!as_integer(self.parse_nested(|parser| parser.parse_unary())?)?) as f64)
        } else {
            self.parse_power()
        }
    }

    fn parse_power(&mut self) -> Result<f64, String> {
        let value = self.parse_primary()?;
        if self.consume(b"^") {
            Ok(value.powf(self.parse_nested(|parser| parser.parse_unary())?))
        } else {
            Ok(value)
        }
    }

    fn parse_primary(&mut self) -> Result<f64, String> {
        self.skip_whitespace();
        if self.consume(b"(") {
            let value = self.parse_nested(|parser| parser.parse_bit_or())?;
            if !self.consume(b")") {
                return Err("Missing closing parenthesis".to_string());
            }
            return Ok(value);
        }

        let Some(&next) = self.input.get(self.position) else {
            return Err("Expected a number or parenthesized expression".to_string());
        };
        if next.is_ascii_digit() || next == b'.' {
            self.parse_number()
        } else if next.is_ascii_alphabetic() {
            self.parse_identifier()
        } else {
            Err(format!("Unexpected token near '{}'", self.remaining()))
        }
    }

    fn parse_identifier(&mut self) -> Result<f64, String> {
        let start = self.position;
        while self
            .input
            .get(self.position)
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        {
            self.position += 1;
        }
        let name = String::from_utf8_lossy(&self.input[start..self.position]).to_ascii_lowercase();
        if name == "pi" {
            return Ok(std::f64::consts::PI);
        }
        if name == "e" {
            return Ok(std::f64::consts::E);
        }
        if name == "xor" {
            return Err("Use xor between two integers".to_string());
        }
        if !self.consume(b"(") {
            return Err(format!("Unknown constant or function '{name}'"));
        }
        let argument = self.parse_nested(|parser| parser.parse_bit_or())?;
        if !self.consume(b")") {
            return Err("Missing closing parenthesis".to_string());
        }
        let to_radians = |value: f64| {
            if self.degrees {
                value.to_radians()
            } else {
                value
            }
        };
        let from_radians = |value: f64| {
            if self.degrees {
                value.to_degrees()
            } else {
                value
            }
        };
        match name.as_str() {
            "sin" => Ok(to_radians(argument).sin()),
            "cos" => Ok(to_radians(argument).cos()),
            "tan" => Ok(to_radians(argument).tan()),
            "asin" => Ok(from_radians(argument.asin())),
            "acos" => Ok(from_radians(argument.acos())),
            "atan" => Ok(from_radians(argument.atan())),
            "sqrt" if argument >= 0.0 => Ok(argument.sqrt()),
            "sqrt" => Err("Square root requires a non-negative value".to_string()),
            "ln" if argument > 0.0 => Ok(argument.ln()),
            "log" | "log10" if argument > 0.0 => Ok(argument.log10()),
            "log2" if argument > 0.0 => Ok(argument.log2()),
            "abs" => Ok(argument.abs()),
            "exp" => Ok(argument.exp()),
            "ln" | "log" | "log10" | "log2" => {
                Err("Logarithms require a positive value".to_string())
            }
            _ => Err(format!("Unknown function '{name}'")),
        }
    }

    fn parse_number(&mut self) -> Result<f64, String> {
        let start = self.position;
        let remaining = &self.input[self.position..];
        let radix = if remaining.starts_with(b"0x") || remaining.starts_with(b"0X") {
            Some(16)
        } else if remaining.starts_with(b"0b") || remaining.starts_with(b"0B") {
            Some(2)
        } else if remaining.starts_with(b"0o") || remaining.starts_with(b"0O") {
            Some(8)
        } else {
            None
        };
        if let Some(radix) = radix {
            self.position += 2;
            let digits_start = self.position;
            while self
                .input
                .get(self.position)
                .is_some_and(|byte| byte.is_ascii_hexdigit())
            {
                self.position += 1;
            }
            if digits_start == self.position {
                return Err("Base prefix must be followed by digits".to_string());
            }
            let digits = std::str::from_utf8(&self.input[digits_start..self.position])
                .map_err(|_| "Invalid integer literal".to_string())?;
            let integer = u64::from_str_radix(digits, radix)
                .map_err(|_| format!("Invalid base-{radix} integer"))?;
            if integer > (1u64 << 53) - 1 {
                return Err("Integer literals are limited to 53 bits".to_string());
            }
            return Ok(integer as f64);
        }

        while self
            .input
            .get(self.position)
            .is_some_and(|byte| byte.is_ascii_digit())
        {
            self.position += 1;
        }
        if self.input.get(self.position) == Some(&b'.') {
            self.position += 1;
            while self
                .input
                .get(self.position)
                .is_some_and(|byte| byte.is_ascii_digit())
            {
                self.position += 1;
            }
        }
        if self
            .input
            .get(self.position)
            .is_some_and(|byte| matches!(*byte, b'e' | b'E'))
        {
            self.position += 1;
            if self
                .input
                .get(self.position)
                .is_some_and(|byte| matches!(*byte, b'+' | b'-'))
            {
                self.position += 1;
            }
            let exponent_start = self.position;
            while self
                .input
                .get(self.position)
                .is_some_and(|byte| byte.is_ascii_digit())
            {
                self.position += 1;
            }
            if exponent_start == self.position {
                return Err("Exponent must contain digits".to_string());
            }
        }
        let number = std::str::from_utf8(&self.input[start..self.position])
            .map_err(|_| "Invalid number".to_string())?;
        number
            .parse::<f64>()
            .map_err(|_| "Invalid number".to_string())
    }
}

fn as_integer(value: f64) -> Result<i64, String> {
    if !value.is_finite() || value.fract() != 0.0 || value.abs() > ((1i64 << 53) - 1) as f64 {
        return Err(
            "Bitwise operations require an exact integer no larger than 53 bits".to_string(),
        );
    }
    Ok(value as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluates_scientific_expressions_and_precedence() {
        assert_eq!(evaluate_expression("2 + 3 * 4", false).unwrap(), 14.0);
        assert_eq!(evaluate_expression("2^3^2", false).unwrap(), 512.0);
        assert_eq!(evaluate_expression("-2^2", false).unwrap(), -4.0);
        assert!((evaluate_expression("sin(30)", true).unwrap() - 0.5).abs() < 1e-12);
        assert_eq!(
            evaluate_expression("sqrt(81) + log(100) + ln(e)", false).unwrap(),
            12.0
        );
        assert_eq!(evaluate_expression("log2(8)", false).unwrap(), 3.0);
        assert!((evaluate_expression("pi", false).unwrap() - std::f64::consts::PI).abs() < 1e-12);
    }

    #[test]
    fn evaluates_programmer_literals_and_operators() {
        assert_eq!(
            evaluate_expression("0b1010 + 0o10 + 0x10", false).unwrap(),
            34.0
        );
        assert_eq!(evaluate_expression("5 & 3", false).unwrap(), 1.0);
        assert_eq!(evaluate_expression("5 | 2", false).unwrap(), 7.0);
        assert_eq!(evaluate_expression("5 xor 3", false).unwrap(), 6.0);
        assert_eq!(evaluate_expression("1 << 4", false).unwrap(), 16.0);
        assert_eq!(evaluate_expression("~0", false).unwrap(), -1.0);
    }

    #[test]
    fn rejects_invalid_and_unsafe_inputs_with_errors() {
        assert!(evaluate_expression("1 + )", false).is_err());
        assert!(evaluate_expression("sqrt(-1)", false).is_err());
        assert!(evaluate_expression("1 / 0", false).is_err());
        assert!(evaluate_expression("1 << 64", false).is_err());
        assert!(evaluate_expression("system(1)", false).is_err());
        assert!(evaluate_expression("0x20000000000000", false).is_err());
        assert!(evaluate_expression(&"(".repeat(MAX_EXPRESSION_BYTES), false).is_err());
    }

    #[test]
    fn formats_integral_results_in_selected_base() {
        let overlay = CalculatorOverlay {
            result: Some(42.0),
            base: CalculatorBase::Hexadecimal,
            ..Default::default()
        };
        assert_eq!(overlay.formatted_result().as_deref(), Some("0x2A"));
        assert_eq!(
            overlay.conversion_summary().as_deref(),
            Some("BIN 0b101010   OCT 0o52   DEC 42   HEX 0x2A")
        );
    }
}
