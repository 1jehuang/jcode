use super::*;

/// `None` means use the existing source-code block renderer, optionally with a
/// D2 diagnostic. An unfinished fence never launches the external compiler.
pub(super) fn render_d2_block(
    language: Option<&str>,
    source: &str,
    max_width: Option<usize>,
    register: bool,
    stream_scope: Option<u64>,
    closed: bool,
) -> (Option<Vec<Line<'static>>>, Option<String>) {
    if !language.is_some_and(mermaid::is_d2_lang)
        || !mermaid::native_image_protocol_available()
        || !mermaid::d2_rendering_enabled()
        || !closed
    {
        return (None, None);
    }
    let width = max_width.and_then(|width| u16::try_from(width).ok());
    match mermaid::render_d2_deferred_with_registration(source, width, register, stream_scope) {
        Some(mermaid::RenderResult::Error(error)) => (None, Some(error)),
        Some(result) => {
            if stream_scope.is_some()
                && let mermaid::RenderResult::Image {
                    hash,
                    width,
                    height,
                    ..
                } = &result
            {
                mermaid::set_streaming_preview_diagram(*hash, *width, *height, Some("D2".into()));
            }
            (Some(mermaid::result_to_lines(result, max_width)), None)
        }
        None => (
            Some(vec![d2_notice_line("↻ rendering D2 diagram...", max_width)]),
            None,
        ),
    }
}

pub(super) fn d2_notice_line(text: &str, max_width: Option<usize>) -> Line<'static> {
    let text: String = text.chars().filter(|ch| !ch.is_control()).collect();
    let limit = max_width.unwrap_or(80);
    let mut output = String::new();
    if text.width() <= limit {
        output = text;
    } else if limit == 1 && text.starts_with('↻') {
        output.push('↻');
    } else if limit > 0 {
        let mut width = 0;
        for ch in text.chars() {
            let cells = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            if width + cells >= limit {
                break;
            }
            output.push(ch);
            width += cells;
        }
        output.push('…');
    }
    mermaid_sidebar_placeholder(&output)
}

/// Pulldown-cmark closes fences implicitly at EOF too. Inspect the original
/// block range, including blockquote gutters, to require a real closing fence.
pub(super) fn fenced_block_is_closed(block: &str) -> bool {
    fn ungutter(mut line: &str) -> &str {
        line = line.trim_start();
        while let Some(rest) = line.strip_prefix('>') {
            line = rest.trim_start();
        }
        line
    }
    let mut lines = block.trim_end().lines();
    let Some(first) = lines.next() else {
        return false;
    };
    let first = ungutter(first);
    let Some(marker @ ('`' | '~')) = first.chars().next() else {
        return false;
    };
    let count = first.chars().take_while(|ch| *ch == marker).count();
    let Some(last) = lines.last() else {
        return false;
    };
    let last = ungutter(last).trim_end();
    let closing = last.chars().take_while(|ch| *ch == marker).count();
    count >= 3 && closing >= count && last.chars().skip(closing).all(char::is_whitespace)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_fence_is_required_for_standard_nested_and_tilde_blocks() {
        for markdown in [
            "```d2\na -> b\n```\n",
            "~~~D2\na -> b\n~~~",
            "> ```d2\n> a -> b\n> ```",
            "- ```d2\n  a -> b\n  ```",
        ] {
            let (event, range) = Parser::new(markdown)
                .into_offset_iter()
                .find(|(event, _)| matches!(event, Event::Start(Tag::CodeBlock(_))))
                .unwrap();
            assert!(matches!(event, Event::Start(Tag::CodeBlock(_))));
            assert!(fenced_block_is_closed(&markdown[range]), "{markdown}");
        }
        for markdown in ["```d2\na -> b", "```d2\na -> b\n``", "~~~d2\na -> b\n```"] {
            assert!(!fenced_block_is_closed(markdown), "{markdown}");
        }
    }

    #[test]
    fn d2_notice_never_wraps_or_loses_narrow_pending_detection() {
        for width in 1..40 {
            let line = d2_notice_line("↻ rendering D2 diagram...", Some(width));
            let plain: String = line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect();
            assert!(plain.width() <= width);
            assert!(
                line_is_mermaid_pending_placeholder(&line),
                "{width}: {plain}"
            );
        }
    }
}
