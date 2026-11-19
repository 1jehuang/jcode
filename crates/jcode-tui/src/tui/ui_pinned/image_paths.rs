use super::*;

pub(super) fn wrap_side_panel_markdown_lines(
    lines: Vec<Line<'static>>,
    width: usize,
) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .flat_map(|line| {
            if is_rendered_table_line(&line)
                || mermaid::parse_image_placeholder(&line).is_some()
                || mermaid::parse_inline_image_placeholder(&line).is_some()
            {
                vec![line]
            } else {
                markdown::wrap_line(line, width)
            }
        })
        .collect()
}

pub(super) fn markdown_image_line_to_placeholder(
    page: &crate::side_panel::SidePanelPage,
    line: Line<'static>,
) -> Result<Line<'static>, Line<'static>> {
    let text = super::super::line_plain_text(&line);
    let Some(path_text) = parse_rendered_markdown_image_path(&text) else {
        return Err(line);
    };
    let path = resolve_side_panel_image_path(page, path_text);
    let Some(hash) = mermaid::register_markdown_image(&path) else {
        return Err(line);
    };
    let marker = mermaid::image_widget_placeholder_markdown(hash)
        .trim_end()
        .to_string();
    Ok(Line::from(Span::styled(
        marker,
        Style::default().fg(Color::Black).bg(Color::Black),
    )))
}

pub(super) fn parse_rendered_markdown_image_path(text: &str) -> Option<&str> {
    let text = text.trim();
    if !text.starts_with("[image:") || !text.ends_with(')') {
        return None;
    }

    let start = text.rfind("] (")? + 3;
    let path = text.get(start..text.len().saturating_sub(1))?.trim();
    if path.is_empty()
        || path.starts_with("http://")
        || path.starts_with("https://")
        || path.starts_with("data:")
    {
        return None;
    }

    let lower = path.to_ascii_lowercase();
    if matches!(
        std::path::Path::new(&lower)
            .extension()
            .and_then(|extension| extension.to_str()),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "svg")
    ) {
        Some(path)
    } else {
        None
    }
}

pub(super) fn resolve_side_panel_image_path(
    page: &crate::side_panel::SidePanelPage,
    path_text: &str,
) -> std::path::PathBuf {
    let path = std::path::Path::new(path_text);
    if path.is_absolute() {
        return path.to_path_buf();
    }

    std::path::Path::new(&page.file_path)
        .parent()
        .map(|parent| parent.join(path))
        .unwrap_or_else(|| path.to_path_buf())
}

/// Rewrite Windows separators inside Markdown image destinations.
///
/// CommonMark treats a backslash in a link destination as an escape, so
/// `![shot](C:\Users\me\shot.png)` reaches the renderer as
/// `C:Usersmeshot.png` and can never resolve to a file. Forward slashes are an
/// equally valid separator on Windows, so destinations that are unambiguously
/// Windows paths (a drive letter or a UNC prefix) are rewritten before parsing.
/// Everything else in the document is copied through untouched, so backslashes
/// in prose, code spans and fences are unaffected.
pub(super) fn normalize_image_destinations(content: &str) -> std::borrow::Cow<'_, str> {
    if !content.contains("![") || !content.contains('\\') {
        return std::borrow::Cow::Borrowed(content);
    }
    let mut out = String::with_capacity(content.len());
    let mut rest = content;
    while let Some(bang) = rest.find("![") {
        let (prefix, after) = rest.split_at(bang);
        out.push_str(prefix);
        match image_destination_bounds(after) {
            Some((start, end)) => {
                out.push_str(&after[..start]);
                let destination = &after[start..end];
                if looks_like_windows_path(destination) {
                    out.push_str(&destination.replace('\\', "/"));
                } else {
                    out.push_str(destination);
                }
                rest = &after[end..];
            }
            None => {
                // Not an inline image with a destination; keep the "![" and
                // resume scanning after it so a later image is still found.
                out.push_str("![");
                rest = &after[2..];
            }
        }
    }
    out.push_str(rest);
    std::borrow::Cow::Owned(out)
}

/// Render side-panel Markdown with Windows image destinations normalized.
pub(super) fn render_page_markdown(
    page: &crate::side_panel::SidePanelPage,
    width: usize,
) -> Vec<Line<'static>> {
    let normalized = normalize_image_destinations(&page.content);
    markdown::render_markdown_with_width(&normalized, Some(width))
}

/// Byte range of the destination inside `![alt](destination)`, given a slice
/// that starts at `![`.
fn image_destination_bounds(text: &str) -> Option<(usize, usize)> {
    let close = text.find("](")?;
    let mut start = close + 2;
    while text[start..].starts_with(char::is_whitespace) {
        start += 1;
    }
    if text[start..].starts_with('<') {
        let inner_start = start + 1;
        let inner_end = inner_start + text[inner_start..].find('>')?;
        return Some((inner_start, inner_end));
    }
    // The destination ends at the closing paren, or at whitespace when a title
    // follows it.
    let inner = &text[start..start + text[start..].find(')')?];
    let end = start + inner.find(char::is_whitespace).unwrap_or(inner.len());
    Some((start, end))
}

fn looks_like_windows_path(value: &str) -> bool {
    let bytes = value.as_bytes();
    let drive = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/');
    let unc = value.starts_with("\\\\");
    drive || unc
}

#[cfg(test)]
mod destination_tests {
    use super::normalize_image_destinations;

    #[test]
    fn drive_letter_destinations_are_rewritten() {
        let text = "![shot](C:\\Users\\me\\shots\\a.png) after";
        assert_eq!(
            normalize_image_destinations(text),
            "![shot](C:/Users/me/shots/a.png) after"
        );
    }

    #[test]
    fn unc_destinations_are_rewritten() {
        let text = "![shot](\\\\server\\share\\a.png)";
        assert_eq!(
            normalize_image_destinations(text),
            "![shot](//server/share/a.png)"
        );
    }

    #[test]
    fn angle_bracket_destinations_are_rewritten() {
        let text = "![shot](<C:\\a b\\c.png>)";
        assert_eq!(
            normalize_image_destinations(text),
            "![shot](<C:/a b/c.png>)"
        );
    }

    #[test]
    fn prose_code_and_non_windows_destinations_are_untouched() {
        for text in [
            "plain \\ backslash",
            "```\nC:\\Users\\me\n```",
            "![shot](https://example.test/a.png)",
            "![shot](/home/me/a.png)",
            "![no destination]",
        ] {
            assert_eq!(normalize_image_destinations(text), text, "{text}");
        }
    }

    #[test]
    fn several_images_and_surrounding_text_are_preserved() {
        let text = "a ![x](C:\\one.png) b ![y](D:\\two\\three.png) c";
        assert_eq!(
            normalize_image_destinations(text),
            "a ![x](C:/one.png) b ![y](D:/two/three.png) c"
        );
    }
}
