//! Input preprocessing applied before markdown parsing.
//!
//! pulldown-cmark's math extension is aggressive: any `$`-delimited run can be
//! treated as inline math, so plain currency like `$5` or `$5x$` gets parsed as
//! math. The TUI renderer guards against this by escaping `$`-then-digit into
//! `\$` before parsing. We mirror that behavior here so the shared core matches
//! the authoritative renderer.

const DISPLAY_MATH_ENVIRONMENTS: &[&str] = &[
    "equation",
    "equation*",
    "displaymath",
    "align",
    "align*",
    "aligned",
    "aligned*",
    "gather",
    "gather*",
    "gathered",
    "multline",
    "multline*",
    "split",
    "eqnarray",
    "eqnarray*",
    "matrix",
    "smallmatrix",
    "array",
    "pmatrix",
    "bmatrix",
    "Bmatrix",
    "vmatrix",
    "Vmatrix",
    "cases",
    "cases*",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum MathDelimiter {
    DollarInline,
    DollarDisplay,
    LatexInline,
    LatexDisplay,
}

/// Normalize common LaTeX math containers into pulldown-cmark's `$` / `$$`
/// syntax before markdown parsing.
///
/// This recognizes `\(...\)`, `\[...\]`, standalone display-math
/// environments, and fenced `math`/`latex`/`tex`/`katex` blocks. Generic code
/// spans and code fences are intentionally left byte-for-byte unchanged.
pub fn normalize_latex_math(text: &str) -> String {
    let fenced = normalize_math_fences(text);
    let normalized = normalize_latex_delimiters_and_environments(&fenced);
    let promoted = promote_standalone_inline_math(&normalized);
    stabilize_display_math(&promoted)
}

/// A single-dollar or `\(`/`\)` pair placed on lines by itself is visibly a
/// block construct even though its delimiter spelling is inline. pulldown-cmark
/// does not tokenize inline math across physical newlines, so promote this
/// unambiguous line-oriented form to display math. Embedded multiline inline
/// delimiters keep Markdown's ordinary literal fallback behavior.
fn promote_standalone_inline_math(text: &str) -> String {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut out = String::with_capacity(text.len());
    let mut index = 0usize;
    let mut fence: Option<(char, usize)> = None;

    while index < lines.len() {
        let line = lines[index];
        let logical = line
            .strip_suffix('\n')
            .unwrap_or(line)
            .trim_end_matches('\r');
        if let Some((marker, len)) = fence {
            out.push_str(line);
            if closing_fence(logical, marker, len) {
                fence = None;
            }
            index += 1;
            continue;
        }
        let Some((prefix, marker_start)) = math_line_prefix(logical, "$") else {
            if let Some((_, marker, len, _)) = opening_fence(logical) {
                fence = Some((marker, len));
            }
            out.push_str(line);
            index += 1;
            continue;
        };

        let Some(close_offset) = lines[index + 1..].iter().position(|candidate| {
            let candidate = candidate
                .strip_suffix('\n')
                .unwrap_or(candidate)
                .trim_end_matches('\r');
            prefix
                .strip_continuation(candidate)
                .and_then(|rest| math_line_prefix(rest, "$"))
                .is_some()
        }) else {
            out.push_str(line);
            index += 1;
            continue;
        };
        let close_index = index + 1 + close_offset;
        out.push_str(&line[..marker_start]);
        out.push_str("$$");
        out.push_str(&line[marker_start + 1..]);
        for body_line in &lines[index + 1..close_index] {
            out.push_str(body_line);
        }
        let close = lines[close_index];
        let close_logical = close
            .strip_suffix('\n')
            .unwrap_or(close)
            .trim_end_matches('\r');
        let close_start = close_logical.rfind('$').expect("matched closing marker");
        out.push_str(&close[..close_start]);
        out.push_str("$$");
        out.push_str(&close[close_start + 1..]);
        index = close_index + 1;
    }

    out
}

/// pulldown-cmark recognizes block constructs before it finishes collecting a
/// multiline `$$` span. A perfectly valid TeX line such as `=` can therefore
/// become a Setext heading underline and split the math block. Prefix those
/// interrupting body lines with an empty TeX group (`{}`), which is invisible to
/// both the terminal renderer and a real LaTeX toolchain while making the line
/// unambiguously part of the math span. Unlike flattening the display to one
/// line, this preserves TeX comments and other newline-sensitive source.
fn stabilize_display_math(text: &str) -> String {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut out = String::with_capacity(text.len());
    let mut index = 0usize;
    let mut fence: Option<(char, usize)> = None;

    while index < lines.len() {
        let line = lines[index];
        let logical = line
            .strip_suffix('\n')
            .unwrap_or(line)
            .trim_end_matches('\r');

        if let Some((marker, len)) = fence {
            out.push_str(line);
            if closing_fence(logical, marker, len) {
                fence = None;
            }
            index += 1;
            continue;
        }
        let Some((prefix, _)) = math_line_prefix(logical, "$$") else {
            if let Some((_, marker, len, _)) = opening_fence(logical) {
                fence = Some((marker, len));
            }
            out.push_str(line);
            index += 1;
            continue;
        };

        let Some(close_offset) = lines[index + 1..].iter().position(|candidate| {
            let candidate = candidate
                .strip_suffix('\n')
                .unwrap_or(candidate)
                .trim_end_matches('\r');
            prefix
                .strip_continuation(candidate)
                .is_some_and(|rest| rest.trim() == "$$")
        }) else {
            out.push_str(line);
            index += 1;
            continue;
        };
        let close_index = index + 1 + close_offset;
        out.push_str(line);
        for body_line in &lines[index + 1..close_index] {
            let without_newline = body_line.strip_suffix('\n').unwrap_or(body_line);
            let logical_body = without_newline.trim_end_matches('\r');
            let ending = &body_line[logical_body.len()..];
            let (container, math) = prefix.split_body(logical_body);
            out.push_str(container);
            if markdown_interrupts_math(math) {
                out.push_str("{}");
            }
            out.push_str(math);
            out.push_str(ending);
        }
        out.push_str(lines[close_index]);
        index = close_index + 1;
    }

    out
}

#[derive(Debug)]
struct DisplayMathLinePrefix {
    continuation: String,
    containerized: bool,
}

impl DisplayMathLinePrefix {
    fn strip_continuation<'a>(&self, line: &'a str) -> Option<&'a str> {
        if self.containerized {
            line.strip_prefix(&self.continuation)
        } else {
            Some(line.strip_prefix(&self.continuation).unwrap_or(line))
        }
    }

    fn split_body<'a>(&self, line: &'a str) -> (&'a str, &'a str) {
        match line.strip_prefix(&self.continuation) {
            Some(math) => line.split_at(line.len() - math.len()),
            None => ("", line),
        }
    }
}

fn math_line_prefix(line: &str, marker: &str) -> Option<(DisplayMathLinePrefix, usize)> {
    let marker_start = line.rfind(marker)?;
    if line[marker_start..].trim() != marker
        || (marker == "$" && line[..marker_start].ends_with('$'))
    {
        return None;
    }
    let before = &line[..marker_start];
    let leading_len = before.len() - before.trim_start_matches([' ', '\t']).len();
    let leading = &before[..leading_len];
    let mut position = leading_len;
    let mut has_quote = false;

    while line[position..marker_start].starts_with('>') {
        has_quote = true;
        position += 1;
        if line[position..marker_start].starts_with([' ', '\t']) {
            position += 1;
        }
    }

    let list_marker = &line[position..marker_start];
    if list_marker.is_empty() {
        let leading_columns = leading
            .chars()
            .map(|ch| if ch == '\t' { 4 } else { 1 })
            .sum::<usize>();
        if !has_quote && leading_columns > 3 {
            return None;
        }
        return Some((
            DisplayMathLinePrefix {
                continuation: before.to_string(),
                containerized: has_quote,
            },
            marker_start,
        ));
    }

    let marker_width = markdown_list_marker_width(list_marker)?;
    Some((
        DisplayMathLinePrefix {
            continuation: format!("{}{}", &line[..position], " ".repeat(marker_width)),
            containerized: true,
        },
        marker_start,
    ))
}

fn markdown_list_marker_width(marker: &str) -> Option<usize> {
    if matches!(marker, "- " | "* " | "+ ") {
        return Some(marker.len());
    }
    let (number, suffix) = marker.split_once(['.', ')'])?;
    (!number.is_empty()
        && number.chars().all(|ch| ch.is_ascii_digit())
        && matches!(suffix, " " | "\t"))
    .then_some(marker.len())
}

fn markdown_interrupts_math(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return true;
    }
    let is_setext = trimmed.chars().all(|ch| ch == '=')
        || (trimmed.len() >= 3 && trimmed.chars().all(|ch| ch == '-'));
    let is_fence = trimmed.starts_with("```") || trimmed.starts_with("~~~");
    let is_atx_heading = trimmed.starts_with('#');
    let is_quote = trimmed.starts_with('>');
    let is_list = trimmed.starts_with("- ")
        || trimmed.starts_with("* ")
        || trimmed.starts_with("+ ")
        || trimmed
            .split_once(['.', ')'])
            .is_some_and(|(number, rest)| {
                !number.is_empty()
                    && number.chars().all(|ch| ch.is_ascii_digit())
                    && rest.starts_with([' ', '\t'])
            });
    is_setext || is_fence || is_atx_heading || is_quote || is_list
}

fn normalize_math_fences(text: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out = String::with_capacity(text.len());
    let mut index = 0usize;

    while index < lines.len() {
        let Some((indent, marker, marker_len, info)) = opening_fence(lines[index]) else {
            out.push_str(lines[index]);
            if index + 1 < lines.len() {
                out.push('\n');
            }
            index += 1;
            continue;
        };

        let Some(close_offset) = lines[index + 1..]
            .iter()
            .position(|line| closing_fence(line, marker, marker_len))
        else {
            out.push_str(lines[index]);
            if index + 1 < lines.len() {
                out.push('\n');
            }
            index += 1;
            continue;
        };
        let close_index = index + 1 + close_offset;

        if is_math_fence_info(info) {
            let body = lines[index + 1..close_index].join("\n");
            let body = strip_outer_display_delimiters(&body);
            out.push_str(indent);
            out.push_str("$$\n");
            out.push_str(body.trim_matches('\n'));
            out.push('\n');
            out.push_str(indent);
            out.push_str("$$");
            if close_index + 1 < lines.len() {
                out.push('\n');
            }
        } else {
            for (offset, line) in lines[index..=close_index].iter().enumerate() {
                out.push_str(line);
                let is_closing_line = index + offset == close_index;
                if close_index + 1 < lines.len() || !is_closing_line {
                    out.push('\n');
                }
            }
        }
        index = close_index + 1;
    }

    out
}

fn strip_outer_display_delimiters(body: &str) -> &str {
    let trimmed = body.trim();
    if let Some(inner) = trimmed
        .strip_prefix("\\[")
        .and_then(|value| value.strip_suffix("\\]"))
    {
        return inner;
    }
    if let Some(inner) = trimmed
        .strip_prefix("$$")
        .and_then(|value| value.strip_suffix("$$"))
    {
        return inner;
    }
    body
}

fn opening_fence(line: &str) -> Option<(&str, char, usize, &str)> {
    let indent_len = line.len() - line.trim_start_matches([' ', '\t']).len();
    let indent = &line[..indent_len];
    if indent
        .chars()
        .map(|ch| if ch == '\t' { 4 } else { 1 })
        .sum::<usize>()
        > 3
    {
        return None;
    }
    let rest = &line[indent_len..];
    let marker = rest.chars().next()?;
    if !matches!(marker, '`' | '~') {
        return None;
    }
    let marker_len = rest.chars().take_while(|ch| *ch == marker).count();
    if marker_len < 3 {
        return None;
    }
    let marker_bytes = marker.len_utf8() * marker_len;
    Some((indent, marker, marker_len, rest[marker_bytes..].trim()))
}

fn closing_fence(line: &str, marker: char, minimum_len: usize) -> bool {
    let trimmed = line.trim_start_matches([' ', '\t']);
    let run = trimmed.chars().take_while(|ch| *ch == marker).count();
    run >= minimum_len && trimmed[run * marker.len_utf8()..].trim().is_empty()
}

fn is_math_fence_info(info: &str) -> bool {
    let language = info
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_matches(['{', '}', '.'])
        .to_ascii_lowercase();
    matches!(language.as_str(), "math" | "latex" | "tex" | "katex")
}

fn normalize_latex_delimiters_and_environments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut index = 0usize;
    let mut inline_ticks = 0usize;
    let mut fence: Option<(char, usize)> = None;
    let mut line_start = true;
    let mut leading_columns = 0usize;
    let mut math_delimiter = None;
    let mut wrapped_environment: Option<(String, usize)> = None;
    let mut literal_environment: Option<(String, usize)> = None;

    while index < text.len() {
        let rest = &text[index..];
        let ch = rest.chars().next().expect("index stays on a char boundary");

        if ch == '\n' {
            out.push(ch);
            index += 1;
            line_start = true;
            leading_columns = 0;
            continue;
        }

        if line_start && matches!(ch, ' ' | '\t') {
            leading_columns += if ch == '\t' { 4 } else { 1 };
            out.push(ch);
            index += ch.len_utf8();
            continue;
        }

        if inline_ticks == 0
            && math_delimiter.is_none()
            && line_start
            && leading_columns <= 3
            && matches!(ch, '`' | '~')
        {
            let run = rest
                .chars()
                .take_while(|candidate| *candidate == ch)
                .count();
            if run >= 3 {
                let current_line = rest.split_once('\n').map_or(rest, |(line, _)| line);
                let trailing = &current_line[run * ch.len_utf8()..];
                let is_close = fence.is_some_and(|(marker, len)| {
                    marker == ch && run >= len && trailing.trim().is_empty()
                });
                if fence.is_none() || is_close {
                    fence = if is_close { None } else { Some((ch, run)) };
                }
                out.push_str(&rest[..run * ch.len_utf8()]);
                index += run * ch.len_utf8();
                line_start = false;
                continue;
            }
        }
        line_start = false;

        // Four-space and tab-indented Markdown code is literal just like fenced
        // and inline code. Keep the whole logical line byte-for-byte unchanged.
        if leading_columns >= 4 {
            out.push(ch);
            index += ch.len_utf8();
            continue;
        }

        if fence.is_some() {
            out.push(ch);
            index += ch.len_utf8();
            continue;
        }

        if ch == '`' && math_delimiter.is_none() {
            let run = rest
                .chars()
                .take_while(|candidate| *candidate == '`')
                .count();
            if inline_ticks == 0 {
                inline_ticks = run;
            } else if inline_ticks == run {
                inline_ticks = 0;
            }
            out.push_str(&rest[..run]);
            index += run;
            continue;
        }
        if inline_ticks > 0 {
            out.push(ch);
            index += ch.len_utf8();
            continue;
        }

        if let Some((name, depth)) = literal_environment.as_mut() {
            if let Some((begin_name, marker_len)) = environment_marker(rest, "begin")
                && begin_name == name
            {
                *depth += 1;
                out.push_str(&rest[..marker_len]);
                index += marker_len;
                continue;
            }
            if let Some((end_name, marker_len)) = environment_marker(rest, "end")
                && end_name == name
            {
                *depth -= 1;
                out.push_str(&rest[..marker_len]);
                index += marker_len;
                if *depth == 0 {
                    literal_environment = None;
                }
                continue;
            }
            out.push(ch);
            index += ch.len_utf8();
            continue;
        }

        if ch == '$' && !is_escaped_at(text, index) {
            let run = rest
                .chars()
                .take_while(|candidate| *candidate == '$')
                .count()
                .min(2);
            let looks_like_currency = run == 1
                && rest[1..]
                    .chars()
                    .next()
                    .is_some_and(|next| next.is_ascii_digit());
            if !looks_like_currency {
                math_delimiter = match (math_delimiter, run) {
                    (Some(MathDelimiter::DollarInline), 1)
                    | (Some(MathDelimiter::DollarDisplay), 2) => None,
                    (None, 1) => Some(MathDelimiter::DollarInline),
                    (None, 2) => Some(MathDelimiter::DollarDisplay),
                    (current, _) => current,
                };
            }
            out.push_str(&rest[..run]);
            index += run;
            continue;
        }

        if math_delimiter.is_none()
            && rest.starts_with("\\(")
            && !is_escaped_at(text, index)
            && has_unescaped_delimiter(&rest[2..], "\\)")
            && !delimited_content_looks_like_prose(rest, "\\)")
        {
            out.push('$');
            math_delimiter = Some(MathDelimiter::LatexInline);
            index += 2;
            continue;
        }
        if math_delimiter == Some(MathDelimiter::LatexInline)
            && rest.starts_with("\\)")
            && !is_escaped_at(text, index)
        {
            out.push('$');
            math_delimiter = None;
            index += 2;
            continue;
        }
        if math_delimiter.is_none()
            && rest.starts_with("\\[")
            && !is_escaped_at(text, index)
            && has_unescaped_delimiter(&rest[2..], "\\]")
            && !looks_like_escaped_markdown_link(rest)
            && !delimited_content_looks_like_prose(rest, "\\]")
        {
            out.push_str("$$");
            math_delimiter = Some(MathDelimiter::LatexDisplay);
            index += 2;
            continue;
        }
        if math_delimiter == Some(MathDelimiter::LatexDisplay)
            && rest.starts_with("\\]")
            && !is_escaped_at(text, index)
        {
            out.push_str("$$");
            math_delimiter = None;
            index += 2;
            continue;
        }

        if ch == '\\'
            && math_delimiter.is_none()
            && wrapped_environment.is_none()
            && let Some((name, marker_len)) = environment_marker(rest, "begin")
            && DISPLAY_MATH_ENVIRONMENTS.contains(&name)
        {
            if has_matching_environment_end(rest, name) {
                out.push_str("$$\n");
                out.push_str(&rest[..marker_len]);
                wrapped_environment = Some((name.to_string(), 1));
            } else {
                out.push_str(&rest[..marker_len]);
                literal_environment = Some((name.to_string(), 1));
            }
            index += marker_len;
            continue;
        }

        if let Some((name, depth)) = wrapped_environment.as_mut() {
            if let Some((begin_name, marker_len)) = environment_marker(rest, "begin")
                && begin_name == name
            {
                *depth += 1;
                out.push_str(&rest[..marker_len]);
                index += marker_len;
                continue;
            }
            if let Some((end_name, marker_len)) = environment_marker(rest, "end")
                && end_name == name
            {
                *depth -= 1;
                out.push_str(&rest[..marker_len]);
                index += marker_len;
                if *depth == 0 {
                    out.push_str("\n$$");
                    wrapped_environment = None;
                }
                continue;
            }
        }

        out.push(ch);
        index += ch.len_utf8();
    }

    out
}

fn environment_marker<'a>(rest: &'a str, command: &str) -> Option<(&'a str, usize)> {
    let prefix = format!("\\{command}{{");
    let after_prefix = rest.strip_prefix(&prefix)?;
    let close = after_prefix.find('}')?;
    let name = &after_prefix[..close];
    Some((name, prefix.len() + close + 1))
}

fn has_matching_environment_end(source: &str, name: &str) -> bool {
    let begin_marker = format!("\\begin{{{name}}}");
    let end_marker = format!("\\end{{{name}}}");
    let mut search = begin_marker.len();
    let mut depth = 1usize;
    while search < source.len() {
        let next_begin = source[search..]
            .find(&begin_marker)
            .map(|offset| search + offset);
        let next_end = source[search..]
            .find(&end_marker)
            .map(|offset| search + offset);
        match (next_begin, next_end) {
            (_, None) => return false,
            (Some(begin), Some(end)) if begin < end => {
                depth += 1;
                search = begin + begin_marker.len();
            }
            (_, Some(end)) => {
                depth -= 1;
                if depth == 0 {
                    return true;
                }
                search = end + end_marker.len();
            }
        }
    }
    false
}

fn is_escaped_at(text: &str, index: usize) -> bool {
    let preceding = text[..index]
        .chars()
        .rev()
        .take_while(|ch| *ch == '\\')
        .count();
    preceding % 2 == 1
}

fn has_unescaped_delimiter(text: &str, delimiter: &str) -> bool {
    find_unescaped_delimiter(text, delimiter).is_some()
}

fn find_unescaped_delimiter(text: &str, delimiter: &str) -> Option<usize> {
    let mut search_from = 0usize;
    while let Some(offset) = text[search_from..].find(delimiter) {
        let index = search_from + offset;
        if !is_escaped_at(text, index) {
            return Some(index);
        }
        search_from = index + delimiter.len();
    }
    None
}

fn looks_like_escaped_markdown_link(rest: &str) -> bool {
    let Some(close) = find_unescaped_delimiter(&rest[2..], "\\]") else {
        return false;
    };
    rest[2 + close + 2..].starts_with('(')
}

/// Models often escape literal brackets in prose, e.g.
/// `\[Attached image associated with ...\]`. That is Markdown punctuation, not
/// LaTeX. Treat the delimited body as prose when it has no math signal at all
/// and reads like a sequence of words.
fn delimited_content_looks_like_prose(rest: &str, close_delimiter: &str) -> bool {
    let Some(close) = find_unescaped_delimiter(&rest[2..], close_delimiter) else {
        return false;
    };
    let body = &rest[2..2 + close];
    if body.chars().any(|ch| {
        matches!(
            ch,
            '\\' | '^' | '_' | '=' | '{' | '}' | '+' | '<' | '>' | '|' | '&'
        )
    }) {
        return false;
    }
    // A single-line body with any run of four or more letters and no LaTeX
    // command or operator is escaped Markdown brackets around words, e.g. a
    // TOML table header like `\[workspace\]`, or a placeholder like
    // `\[pasted N lines\]`. Real math would typeset that as an italic product
    // of variables. Multiline `\[ ... \]` blocks stay math.
    if !body.contains('\n')
        && body
            .split(|ch: char| !ch.is_alphabetic())
            .any(|run| run.chars().count() >= 4)
    {
        return true;
    }
    body.split_whitespace()
        .filter(|word| word.chars().filter(|ch| ch.is_alphabetic()).count() >= 2)
        .count()
        >= 3
}

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
    let mut code_fence: Option<(char, usize)> = None;
    let mut inline_code_len: usize = 0;
    let mut at_line_start = true;
    let mut leading_spaces = 0;

    let count_marker = |chars: &[char], start: usize, marker: char| {
        let mut j = start;
        while j < chars.len() && chars[j] == marker {
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
            leading_spaces += if c == '\t' { 4 } else { 1 };
            out.push(c);
            i += 1;
            continue;
        }

        let marker_run = if matches!(c, '`' | '~') {
            count_marker(&chars, i, c)
        } else {
            0
        };
        let maybe_fence = inline_code_len == 0 && marker_run >= 3;
        if maybe_fence && at_line_start && leading_spaces <= 3 {
            let run = marker_run;
            let line_end = chars[i + run..]
                .iter()
                .position(|ch| *ch == '\n')
                .map(|offset| i + run + offset)
                .unwrap_or(len);
            let trailing_is_blank = chars[i + run..line_end].iter().all(|ch| ch.is_whitespace());
            let is_close = code_fence.is_some_and(|(marker, minimum)| {
                marker == c && run >= minimum && trailing_is_blank
            });
            if code_fence.is_none() || is_close {
                code_fence = if is_close { None } else { Some((c, run)) };
            }
            for _ in 0..run {
                out.push(c);
            }
            i += run;
            at_line_start = false;
            leading_spaces = 0;
            continue;
        }

        if code_fence.is_some() {
            out.push(c);
            i += 1;
            at_line_start = false;
            leading_spaces = 0;
            continue;
        }

        if c == '`' {
            let run = count_marker(&chars, i, '`');
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

        if inline_code_len > 0 {
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
    use super::{escape_currency_dollars, normalize_latex_math};

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
        let tilde = "~~~text\n$5\n~~~";
        assert_eq!(escape_currency_dollars(tilde), tilde);
        let longer = "````text\n``` is content and $5\n````";
        assert_eq!(escape_currency_dollars(longer), longer);
    }

    #[test]
    fn preserves_already_escaped() {
        assert_eq!(escape_currency_dollars("\\$5"), "\\$5");
    }

    #[test]
    fn normalizes_parenthesized_and_bracketed_math() {
        assert_eq!(
            normalize_latex_math(r"Inline \(x^2 + \alpha\) done."),
            r"Inline $x^2 + \alpha$ done."
        );
        assert_eq!(
            normalize_latex_math("\\[\n\\frac{x+1}{y}\n\\]"),
            "$$\n\\frac{x+1}{y}\n$$"
        );
    }

    #[test]
    fn normalizes_standalone_display_environments() {
        let input = "before\n\\begin{align*}\nx &= 1 \\\\\ny &= 2\n\\end{align*}\nafter";
        let normalized = normalize_latex_math(input);
        assert!(normalized.contains("$$\n\\begin{align*}"), "{normalized}");
        assert!(normalized.contains("\\end{align*}\n$$"), "{normalized}");

        let nested = r"\begin{matrix}\begin{matrix}a\end{matrix}\end{matrix}";
        assert_eq!(normalize_latex_math(nested), format!("$$\n{nested}\n$$"));

        let unmatched = r"\begin{matrix}\begin{matrix}a\end{matrix}";
        assert_eq!(normalize_latex_math(unmatched), unmatched);
    }

    #[test]
    fn normalizes_explicit_math_fences() {
        assert_eq!(
            normalize_latex_math("```math\n\\frac{a}{b}\n```"),
            "$$\n\\frac{a}{b}\n$$"
        );
        assert_eq!(
            normalize_latex_math("~~~latex\n\\[\nx^2\n\\]\n~~~"),
            "$$\nx^2\n$$"
        );
    }

    #[test]
    fn preserves_literal_code_and_malformed_delimiters() {
        let inline = r"Use `\(x^2\)` literally";
        let fenced = "```rust\nlet latex = r\"\\[x\\]\";\n```";
        let plain_fenced = "```\nlet x = 1;\n```";
        let escaped_link = r"a \[link\](https://example.com) reference";
        let malformed = r"unfinished \[x^2";
        assert_eq!(normalize_latex_math(inline), inline);
        assert_eq!(normalize_latex_math(fenced), fenced);
        assert_eq!(normalize_latex_math(plain_fenced), plain_fenced);
        assert_eq!(normalize_latex_math(escaped_link), escaped_link);
        assert_eq!(normalize_latex_math(malformed), malformed);
    }

    #[test]
    fn currency_does_not_hide_later_latex_math() {
        assert_eq!(
            normalize_latex_math(r"Costs $35. Then \[x^2\]."),
            r"Costs $35. Then $$x^2$$."
        );
    }

    #[test]
    fn escaped_bracketed_prose_is_not_math() {
        let bracketed = r#"fold in the adjacent "\[Attached image associated...\]" label text"#;
        assert_eq!(normalize_latex_math(bracketed), bracketed);
        let parenthesized = r"see \(the optional part\) here";
        assert_eq!(normalize_latex_math(parenthesized), parenthesized);
        assert_eq!(normalize_latex_math(r"\[a b\]"), r"$$a b$$");
        assert_eq!(
            normalize_latex_math(r"\[\text{area} = \pi r^2\]"),
            r"$$\text{area} = \pi r^2$$"
        );
        assert_eq!(normalize_latex_math(r"\(x\)"), r"$x$");
    }

    #[test]
    fn escaped_single_word_brackets_are_not_math() {
        for input in [
            r"the nested directory's own \[workspace\] declaration",
            r"even though I added \[workspace\] to make this a root",
            r"set \[package.metadata\] keys",
            r"see \(optional\) here",
            r"collapsed into a placeholder like \[pasted N lines\] by jcode",
            r"\[image 1\]",
        ] {
            assert_eq!(normalize_latex_math(input), input);
        }
        assert_eq!(normalize_latex_math(r"\[xy\]"), r"$$xy$$");
        assert_eq!(normalize_latex_math(r"\[x+y\]"), r"$$x+y$$");
        assert_eq!(normalize_latex_math(r"\[\alpha\]"), r"$$\alpha$$");
    }
}
