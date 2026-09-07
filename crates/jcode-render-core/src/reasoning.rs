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
}
