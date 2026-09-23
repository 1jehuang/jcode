//! Reasoning-line markdown formatting.
//!
//! Pure string helpers shared by the server/streaming path and the TUI renderer
//! so the wrapping/escaping rules stay in lockstep with the renderer that
//! consumes them. These live in `jcode-render-core` (a backend-neutral, pure
//! crate) rather than in `jcode-tui-markdown` so the foundation/streaming layer
//! can format reasoning lines without depending on any `jcode-tui-*` crate.

/// Invisible separator placed just inside both ends of an emphasis run so the
/// flanking `*` are always adjacent to non-whitespace (see
/// [`reasoning_line_markup`]).
pub const REASONING_SENTINEL: &str = "\u{2063}";

const REASONING_ESCAPES: &str = "\\*_`[]<>&~|$";

/// Recover the original Markdown from a line produced by
/// [`reasoning_line_markup`] or [`reasoning_partial_markup`]. Native frontends
/// can style reasoning themselves instead of interpreting the terminal's
/// escaped emphasis wrapper. Ordinary Markdown is never unescaped here.
pub fn reasoning_line_content(line: &str) -> Option<String> {
    let body = line
        .trim_end_matches([' ', '\r', '\n'])
        .strip_prefix("*\u{2063}")?
        .strip_suffix("\u{2063}*")?;
    let mut chars = body.chars().peekable();
    let mut content = String::with_capacity(body.len());
    while let Some(ch) = chars.next() {
        if ch == '\\'
            && chars
                .peek()
                .is_some_and(|ch| REASONING_ESCAPES.contains(*ch))
        {
            content.push(chars.next().unwrap());
        } else {
            content.push(ch);
        }
    }
    Some(content)
}

/// Return cleaned text for a parser text event that belongs to a complete
/// generated reasoning wrapper on its source line.
///
/// The source range is supplied by pulldown-cmark's offset iterator. Requiring
/// the complete wrapper prevents an ordinary U+2063 in Markdown from being
/// interpreted as control data, while removing only the two wrapper markers
/// preserves any U+2063 that was part of the wrapped body itself.
pub fn reasoning_text_event(
    markdown: &str,
    source_range: std::ops::Range<usize>,
    text: &str,
) -> Option<String> {
    if source_range.start > markdown.len()
        || source_range.end > markdown.len()
        || source_range.start > source_range.end
        || !markdown.is_char_boundary(source_range.start)
        || !markdown.is_char_boundary(source_range.end)
    {
        return None;
    }

    let line_start = markdown[..source_range.start]
        .rfind('\n')
        .map_or(0, |newline| newline + 1);
    let line_end = markdown[source_range.start..]
        .find('\n')
        .map_or(markdown.len(), |offset| source_range.start + offset);
    let line = &markdown[line_start..line_end];
    let trimmed_len = line.trim_end_matches([' ', '\r']).len();
    if reasoning_line_content(&line[..trimmed_len]).is_none() {
        return None;
    }

    let marker_len = REASONING_SENTINEL.len();
    let leading_marker = line_start + 1..line_start + 1 + marker_len;
    let trailing_marker_start = line_start + trimmed_len - marker_len - 1;
    let trailing_marker = trailing_marker_start..trailing_marker_start + marker_len;
    if source_range.end <= leading_marker.start || source_range.start >= trailing_marker.end {
        return None;
    }

    let mut cleaned = text.to_string();
    if source_range.start <= leading_marker.start
        && source_range.end >= leading_marker.end
        && let Some(without_marker) = cleaned.strip_prefix(REASONING_SENTINEL)
    {
        cleaned = without_marker.to_string();
    }
    if source_range.start <= trailing_marker.start
        && source_range.end >= trailing_marker.end
        && let Some(without_marker) = cleaned.strip_suffix(REASONING_SENTINEL)
    {
        cleaned = without_marker.to_string();
    }
    Some(cleaned)
}

/// Escape the characters that would otherwise be interpreted as inline markdown
/// inside a reasoning line, so the body renders literally inside the dim/italic
/// emphasis run.
fn escape_reasoning_inline_markdown(line: &str) -> String {
    let mut out = String::with_capacity(line.len() + 8);
    for ch in line.chars() {
        if REASONING_ESCAPES.contains(ch) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// Wrap a completed reasoning line as dim+italic markdown.
///
/// Empty lines become a bare newline (no empty emphasis run). The result always
/// ends in a CommonMark hard break (`"  \n"`).
///
/// The trailing two spaces are a CommonMark *hard break*: without them,
/// consecutive reasoning lines (each terminated by a single `\n`) collapse into
/// one paragraph where the line breaks render as spaces, so multi-line thinking
/// shows up as a single run-on line. The hard break keeps each reasoning line on
/// its own visual row, matching the model's line structure.
///
/// The sentinel must wrap both ends because CommonMark's emphasis flanking rules
/// require the opening `*` to not be followed by whitespace and the closing `*`
/// to not be preceded by whitespace. A reasoning line that starts or ends with
/// whitespace (or is whitespace-only) would otherwise leave the asterisks as
/// literal text and break the dim/italic styling. The zero-width sentinels
/// guarantee both asterisks are flanked by non-whitespace regardless of the body.
pub fn reasoning_line_markup(line: &str) -> String {
    if line.is_empty() {
        "\n".to_string()
    } else {
        format!(
            "*{0}{1}{0}*  \n",
            REASONING_SENTINEL,
            escape_reasoning_inline_markdown(line)
        )
    }
}

/// Wrap the in-progress (not yet newline-terminated) reasoning line as dim+italic
/// markdown, identical to [`reasoning_line_markup`] but *without* the trailing
/// newline so it renders as the live tail of the streaming buffer. Callers
/// truncate and re-emit this tail on each streamed delta so reasoning trickles in
/// token-by-token instead of one whole line at a time. An empty line yields an
/// empty string (nothing to render yet).
pub fn reasoning_partial_markup(line: &str) -> String {
    if line.is_empty() {
        String::new()
    } else {
        format!(
            "*{0}{1}{0}*",
            REASONING_SENTINEL,
            escape_reasoning_inline_markdown(line)
        )
    }
}

/// One-line collapsed reasoning summary markup (e.g. `▸ thought (3 lines)`),
/// styled dim+italic like the live reasoning lines. Used to fold a persisted
/// reasoning block down to a single trace line when the transcript is
/// re-rendered from history in `current` reasoning-display mode (so reloaded /
/// resumed sessions match the live collapse instead of replaying every line).
///
/// Lives here (a backend-neutral, pure crate) rather than in `jcode-tui-markdown`
/// so the foundation/streaming layer can format the summary without depending on
/// any `jcode-tui-*` crate. Re-exported from `jcode-tui-markdown` for the
/// existing `jcode_tui_markdown::reasoning_summary_line_markup` path.
pub fn reasoning_summary_line_markup(line_count: usize) -> String {
    let label = match line_count {
        0 | 1 => "▸ thought".to_string(),
        n => format!("▸ thought ({} lines)", n),
    };
    reasoning_line_markup(&label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasoning_markup_round_trips_original_markdown() {
        for text in [
            "**Checking top live tabs**",
            "  **é文字** and `code`  ",
            r"literal \*star\*, C:\work, [docs](https://example.com), $x_1$",
            "\\*_`[]<>&~|$",
            " ",
        ] {
            for markup in [reasoning_line_markup(text), reasoning_partial_markup(text)] {
                assert_eq!(reasoning_line_content(&markup).as_deref(), Some(text));
            }
        }
    }

    #[test]
    fn reasoning_decoder_requires_both_sentinels_and_wrapper() {
        for text in [
            "**heading**",
            r"\*literal\*",
            "*\u{2063}unfinished",
            "*ordinary*",
            "",
        ] {
            assert_eq!(reasoning_line_content(text), None);
        }
    }

    #[test]
    fn reasoning_text_event_removes_only_wrapper_markers() {
        let body = format!("plain{REASONING_SENTINEL}body");
        let markup = reasoning_line_markup(&body);
        let mut options = pulldown_cmark::Options::empty();
        options.insert(pulldown_cmark::Options::ENABLE_SMART_PUNCTUATION);
        let mut visible = String::new();

        for (event, source_range) in
            pulldown_cmark::Parser::new_ext(&markup, options).into_offset_iter()
        {
            if let pulldown_cmark::Event::Text(text) = event {
                let cleaned = reasoning_text_event(&markup, source_range.clone(), &text)
                    .unwrap_or_else(|| {
                        panic!("generated wrapper not recognized at {source_range:?}")
                    });
                visible.push_str(&cleaned);
            }
        }

        assert_eq!(visible, body);
    }
}
