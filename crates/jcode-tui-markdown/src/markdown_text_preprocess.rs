use super::{is_closing_fence, parse_opening_fence};

pub(crate) fn escape_currency_dollars(text: &str) -> String {
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

pub(crate) fn looks_like_line_oriented_transcript_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.is_empty() {
        return false;
    }

    if trimmed.starts_with("tool:")
        || trimmed.starts_with("tools:")
        || trimmed.starts_with("broadcast from ")
    {
        return true;
    }

    matches!(trimmed.chars().next(), Some('✓' | '✗' | '┌' | '│' | '└'))
}

pub(crate) fn preserve_line_oriented_softbreaks(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let lines: Vec<&str> = text.split('\n').collect();
    let mut in_code_fence = false;
    let mut fence_char = '\0';
    let mut fence_len = 0usize;

    for (idx, line) in lines.iter().enumerate() {
        let prev_line = idx.checked_sub(1).map(|prev| lines[prev]);
        let prev_log_like = prev_line.is_some_and(looks_like_line_oriented_transcript_line);
        let next_log_like =
            idx + 1 < lines.len() && looks_like_line_oriented_transcript_line(lines[idx + 1]);
        let line_log_like = looks_like_line_oriented_transcript_line(line);
        let entering_log_block = !in_code_fence
            && line_log_like
            && !prev_log_like
            && prev_line.is_some_and(|prev| !prev.trim().is_empty());
        let leaving_log_block = !in_code_fence
            && line_log_like
            && !next_log_like
            && idx + 1 < lines.len()
            && !lines[idx + 1].trim().is_empty();
        let preserve_softbreak = !in_code_fence && line_log_like && next_log_like;

        if entering_log_block && !out.ends_with("\n\n") {
            out.push('\n');
        }

        out.push_str(line);
        if idx + 1 < lines.len() {
            if preserve_softbreak && !line.ends_with("  ") {
                out.push_str("  ");
            }
            out.push('\n');
            if leaving_log_block {
                out.push('\n');
            }
        }

        if in_code_fence {
            if is_closing_fence(line, fence_char, fence_len) {
                in_code_fence = false;
                fence_char = '\0';
                fence_len = 0;
            }
        } else if let Some((marker, min_len)) = parse_opening_fence(line) {
            in_code_fence = true;
            fence_char = marker;
            fence_len = min_len;
        }
    }

    out
}
