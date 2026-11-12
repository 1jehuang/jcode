//! Minimal shell tokenizer, tuned for risk analysis rather than execution.
//!
//! This deliberately does not implement POSIX shell. It implements just enough
//! to answer "which words are targets of a destructive command", and it is
//! written to **fail loud rather than quiet**: when it cannot understand
//! something it marks the segment as opaque so the caller escalates.

/// One shell word plus the classification bits the assessor needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub text: String,
    /// True when this segment's stdin comes from a pipe, so its operands are
    /// partly supplied at runtime by the previous command.
    pub receives_pipe: bool,
    /// True for `>` / `>|` redirect destinations, which are truncated on open.
    pub is_truncating_redirect_target: bool,
    /// True for control operators like `&&`, which are never path targets.
    pub is_operator: bool,
}

impl Token {
    fn word(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            receives_pipe: false,
            is_truncating_redirect_target: false,
            is_operator: false,
        }
    }

    /// The command name without its directory, so `/bin/rm` matches `rm`.
    pub fn basename(&self) -> String {
        self.text
            .rsplit('/')
            .next()
            .unwrap_or(&self.text)
            .to_string()
    }

    pub fn is_flag(&self) -> bool {
        self.text.starts_with('-') && self.text.len() > 1
    }

    /// Whether this flag requests recursion, including bundles like `-rf`.
    pub fn is_recursive_flag(&self) -> bool {
        if !self.is_flag() {
            return false;
        }
        if self.text.starts_with("--") {
            return self.text == "--recursive";
        }
        self.text.contains('r') || self.text.contains('R')
    }
}

/// Characters that separate one command from the next.
const SEGMENT_SEPARATORS: &[&str] = &["&&", "||", ";", "|", "\n", "(", ")"];

/// Split a command line into individual command segments, each tokenized.
///
/// `rm -rf a && rm -rf b` yields two segments so both are assessed. Without
/// this, chaining would be a trivial bypass.
pub fn split_segments(command: &str) -> Vec<Vec<Token>> {
    let tokens = tokenize(command);
    let mut segments = Vec::new();
    let mut current = Vec::new();

    let mut next_receives_pipe = false;
    for token in tokens {
        if token.is_operator && SEGMENT_SEPARATORS.contains(&token.text.as_str()) {
            if !current.is_empty() {
                segments.push(std::mem::take(&mut current));
            }
            // The command after `|` consumes the previous one's output as
            // operands, which this parser cannot see (#604 review).
            next_receives_pipe = token.text == "|";
            continue;
        }
        let mut token = token;
        token.receives_pipe = next_receives_pipe;
        current.push(token);
    }
    if !current.is_empty() {
        segments.push(current);
    }
    segments
}

/// Tokenize a command line, resolving quotes so `rm "$HOME"` and `rm $HOME`
/// produce the same target text.
///
/// Note the asymmetry with a real shell: we intentionally keep `$VAR` intact
/// rather than expanding it, and the path layer treats an unexpanded variable
/// as unknown-and-therefore-risky.
pub fn tokenize(command: &str) -> Vec<Token> {
    let command = without_heredoc_bodies(command);
    let mut tokens: Vec<Token> = Vec::new();
    let mut current = String::new();
    let mut has_content = false;
    let mut chars = command.chars().peekable();
    let mut pending_redirect = false;

    macro_rules! flush {
        () => {
            if has_content {
                let mut token = Token::word(std::mem::take(&mut current));
                #[allow(unused_assignments)]
                {
                    if pending_redirect {
                        token.is_truncating_redirect_target = true;
                        pending_redirect = false;
                    }
                    has_content = false;
                }
                tokens.push(token);
            }
        };
    }

    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                has_content = true;
                for q in chars.by_ref() {
                    if q == '\'' {
                        break;
                    }
                    current.push(q);
                }
            }
            '"' => {
                has_content = true;
                while let Some(q) = chars.next() {
                    if q == '"' {
                        break;
                    }
                    if q == '\\'
                        && let Some(&next) = chars.peek()
                        && matches!(next, '"' | '\\' | '$' | '`')
                    {
                        current.push(next);
                        chars.next();
                        continue;
                    }
                    current.push(q);
                }
            }
            '\\' => {
                if let Some(next) = chars.next() {
                    has_content = true;
                    current.push(next);
                }
            }
            ' ' | '\t' => flush!(),
            '\n' | ';' => {
                flush!();
                let mut op = Token::word(c.to_string());
                op.is_operator = true;
                tokens.push(op);
            }
            '&' | '|' => {
                flush!();
                let mut text = c.to_string();
                if chars.peek() == Some(&c) {
                    text.push(c);
                    chars.next();
                }
                let mut op = Token::word(text);
                op.is_operator = true;
                tokens.push(op);
            }
            '(' | ')' => {
                flush!();
                let mut op = Token::word(c.to_string());
                op.is_operator = true;
                tokens.push(op);
            }
            '>' => {
                flush!();
                // `>>` appends and does not truncate, so it is far less
                // destructive; only a single `>` clobbers.
                if chars.peek() == Some(&'>') {
                    chars.next();
                } else {
                    if chars.peek() == Some(&'|') {
                        chars.next();
                    }
                    pending_redirect = true;
                }
            }
            '<' => flush!(),
            _ => {
                has_content = true;
                current.push(c);
            }
        }
    }
    flush!();

    tokens
}

/// Remove heredoc payloads before tokenizing shell source.
///
/// A heredoc body is input data for the command on the declaration line, not
/// shell source in the surrounding command. Treating it as source makes prose
/// containing `time`, or a script containing `rm`, trip the risk gate (#922).
/// Newlines are retained so a command after the terminator remains a separate
/// segment and cannot disappear from assessment.
fn without_heredoc_bodies(command: &str) -> String {
    let lines: Vec<&str> = command.split_inclusive('\n').collect();
    let mut output = String::with_capacity(command.len());
    let mut index = 0;

    while index < lines.len() {
        let line = lines[index];
        output.push_str(line);
        let delimiters = heredoc_delimiters(line.trim_end_matches('\n'));
        index += 1;

        for (delimiter, strip_tabs) in delimiters {
            while index < lines.len() {
                let candidate = lines[index].trim_end_matches(['\r', '\n']);
                let candidate = if strip_tabs {
                    candidate.trim_start_matches('\t')
                } else {
                    candidate
                };
                index += 1;
                if candidate == delimiter {
                    output.push('\n');
                    break;
                }
            }
        }
    }

    output
}

fn heredoc_delimiters(line: &str) -> Vec<(String, bool)> {
    let bytes = line.as_bytes();
    let mut found = Vec::new();
    let mut index = 0;
    let mut quote = None;

    while index + 1 < bytes.len() {
        let byte = bytes[index];
        if let Some(end) = quote {
            if byte == b'\\' && end == b'"' {
                index += 2;
                continue;
            }
            if byte == end {
                quote = None;
            }
            index += 1;
            continue;
        }
        if matches!(byte, b'\'' | b'"') {
            quote = Some(byte);
            index += 1;
            continue;
        }
        if byte == b'\\' {
            index += 2;
            continue;
        }
        if byte != b'<' || bytes[index + 1] != b'<' {
            index += 1;
            continue;
        }

        index += 2;
        // `<<<` is a here-string, not a multiline heredoc.
        if bytes.get(index) == Some(&b'<') {
            index += 1;
            continue;
        }
        let strip_tabs = bytes.get(index) == Some(&b'-');
        if strip_tabs {
            index += 1;
        }
        while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
            index += 1;
        }

        let mut delimiter = String::new();
        let mut delimiter_quote = None;
        while index < bytes.len() {
            let byte = bytes[index];
            if let Some(end) = delimiter_quote {
                if byte == end {
                    delimiter_quote = None;
                } else if byte == b'\\' && end == b'"' && index + 1 < bytes.len() {
                    index += 1;
                    delimiter.push(bytes[index] as char);
                } else {
                    delimiter.push(byte as char);
                }
            } else if matches!(byte, b'\'' | b'"') {
                delimiter_quote = Some(byte);
            } else if byte == b'\\' && index + 1 < bytes.len() {
                index += 1;
                delimiter.push(bytes[index] as char);
            } else if byte.is_ascii_whitespace() || matches!(byte, b';' | b'&' | b'|') {
                break;
            } else {
                delimiter.push(byte as char);
            }
            index += 1;
        }
        if !delimiter.is_empty() {
            found.push((delimiter, strip_tabs));
        }
    }

    found
}

#[cfg(test)]
#[path = "tokenize_tests.rs"]
mod tokenize_tests;
