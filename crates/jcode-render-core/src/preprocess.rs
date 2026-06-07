//! Input preprocessing applied before markdown parsing.
//!
//! pulldown-cmark's math extension is aggressive: any `$`-delimited run can be
//! treated as inline math, so plain currency like `$5` or `$5x$` gets parsed as
//! math. The TUI renderer guards against this by escaping `$`-then-digit into
//! `\$` before parsing. We mirror that behavior here so the shared core matches
//! the authoritative renderer.

/// Escape dollar signs that look like currency amounts (`$` immediately
/// followed by an ASCII digit) into `\$`, so the math extension does not treat
/// them as inline math. Dollars inside inline code spans and fenced code blocks
/// are left untouched, and already-escaped `\$` is preserved. Display-math
/// `$$` runs are passed through unchanged.
pub fn escape_currency_dollars(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut in_code_fence = false;
    let mut inline_code_len: usize = 0;
    let mut at_line_start = true;
    let mut leading_spaces = 0;

    let count_backticks = |chars: &[char], start: usize| {
        let mut j = start;
        while j < chars.len() && chars[j] == '`' {
            j += 1;
        }
        j - start
    };

    let is_escaped = |chars: &[char], pos: usize| {
        let mut backslashes = 0usize;
        let mut j = pos;
        while j > 0 {
            if chars[j - 1] != '\\' {
                break;
            }
            backslashes += 1;
            j -= 1;
        }
        backslashes % 2 == 1
    };

    while i < len {
        let c = chars[i];

        if c == '\n' {
            at_line_start = true;
            leading_spaces = 0;
            out.push('\n');
            i += 1;
            continue;
        }

        if at_line_start && (c == ' ' || c == '\t') {
            leading_spaces += 1;
            out.push(c);
            i += 1;
            continue;
        }

        let maybe_fence = inline_code_len == 0 && c == '`' && count_backticks(&chars, i) >= 3;
        if maybe_fence && at_line_start && leading_spaces <= 3 {
            let run = count_backticks(&chars, i);
            for _ in 0..run {
                out.push('`');
            }
            i += run;
            in_code_fence = !in_code_fence;
            at_line_start = false;
            leading_spaces = 0;
            continue;
        }

        if c == '`' {
            let run = count_backticks(&chars, i);
            if inline_code_len > 0 {
                if run == inline_code_len {
                    inline_code_len = 0;
                }
                for _ in 0..run {
                    out.push('`');
                }
                i += run;
                at_line_start = false;
                leading_spaces = 0;
                continue;
            }

            inline_code_len = run;
            for _ in 0..run {
                out.push('`');
            }
            i += run;
            at_line_start = false;
            leading_spaces = 0;
            continue;
        }

        if at_line_start {
            at_line_start = false;
        }

        if c == ' ' || c == '\t' {
            out.push(c);
            i += 1;
            continue;
        }

        if in_code_fence || inline_code_len > 0 {
            out.push(c);
            i += 1;
            continue;
        }

        if c == '$' && i + 1 < len && chars[i + 1] == '$' {
            out.push_str("$$");
            i += 2;
            continue;
        }

        if c == '$' && i + 1 < len && chars[i + 1].is_ascii_digit() {
            if is_escaped(&chars, i) {
                out.push('$');
            } else {
                out.push_str("\\$");
            }
            i += 1;
            continue;
        }

        out.push(c);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::escape_currency_dollars;

    #[test]
    fn escapes_currency() {
        assert_eq!(escape_currency_dollars("$5"), "\\$5");
        assert_eq!(escape_currency_dollars("$5x$"), "\\$5x$");
    }

    #[test]
    fn leaves_real_math() {
        assert_eq!(escape_currency_dollars("$a+b$"), "$a+b$");
    }

    #[test]
    fn passes_through_display_math() {
        assert_eq!(escape_currency_dollars("$$x=5$$"), "$$x=5$$");
    }

    #[test]
    fn skips_inline_code() {
        assert_eq!(escape_currency_dollars("`$5`"), "`$5`");
    }

    #[test]
    fn skips_fenced_code() {
        let input = "```\n$5\n```";
        assert_eq!(escape_currency_dollars(input), input);
    }

    #[test]
    fn preserves_already_escaped() {
        assert_eq!(escape_currency_dollars("\\$5"), "\\$5");
    }
}
