//! Source-aware hit testing for rendered markdown links.
//!
//! Ratatui spans do not carry hyperlink destinations. Resolve them against the
//! owning message, not a global label registry, so identical labels in separate
//! messages (or in the same message) keep their own destinations.
use crate::*;

/// Resolve a display-cell position in rendered lines against markdown source.
/// Only blue, underlined spans participate. Ordinary prose and URL suffixes do
/// not consume source links. A mismatch fails closed rather than opening a
/// destination belonging to a different label.
pub fn markdown_link_target(
    source: &str,
    lines: &[Line<'_>],
    line_index: usize,
    column: usize,
) -> Option<String> {
    let mut links = Vec::new();
    let mut current: Option<(String, String)> = None;
    let mut table_depth = 0usize;
    let mut image_depth = 0usize;
    for event in Parser::new_ext(source, Options::all()) {
        match event {
            Event::Start(Tag::Table(_)) => table_depth += 1,
            Event::End(TagEnd::Table) => table_depth = table_depth.saturating_sub(1),
            Event::Start(Tag::Image { .. }) => image_depth += 1,
            Event::End(TagEnd::Image) => image_depth = image_depth.saturating_sub(1),
            Event::Start(Tag::Link { dest_url, .. }) if table_depth == 0 => {
                current = Some((String::new(), dest_url.into_string()));
            }
            Event::End(TagEnd::Link) => {
                if let Some((label, target)) = current.take() {
                    let label: String = label.chars().filter(|c| !c.is_whitespace()).collect();
                    if !label.is_empty() {
                        links.push((label, target));
                    }
                }
            }
            Event::Text(text) | Event::Code(text) if image_depth == 0 => {
                if let Some((label, _)) = current.as_mut() {
                    label.push_str(&text);
                }
            }
            _ => {}
        }
    }
    let mut links = links.into_iter();
    let (label, mut target) = links.next()?;
    let mut expected: Vec<char> = label.chars().collect();
    let mut offset = 0;
    let mut hit = false;
    for (row, line) in lines.iter().enumerate() {
        let mut col = 0;
        for span in &line.spans {
            let is_link = span.style.fg == Some(link_fg())
                && span.style.add_modifier.contains(Modifier::UNDERLINED);
            for ch in span.content.chars() {
                let width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                if is_link {
                    if row == line_index && column >= col && column < col + width {
                        hit = true;
                    }
                    if !ch.is_whitespace() {
                        if expected.get(offset) != Some(&ch) {
                            return None;
                        }
                        offset += 1;
                        if offset == expected.len() {
                            if hit {
                                return (!target.is_empty()).then_some(target);
                            }
                            let (label, next_target) = links.next()?;
                            expected = label.chars().collect();
                            target = next_target;
                            offset = 0;
                        }
                    }
                }
                col += width;
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_resolve_without_visible_destinations() {
        let source =
            "Plain guide, then [**guide**](https://one.example) and [guide](https://two.example).";
        let lines = render_markdown_with_width(source, Some(80));
        let line = lines
            .iter()
            .position(|line| line.to_string().starts_with("Plain"))
            .unwrap();
        assert_eq!(markdown_link_target(source, &lines, line, 6), None);
        assert_eq!(
            markdown_link_target(source, &lines, line, 18).as_deref(),
            Some("https://one.example")
        );
        assert_eq!(
            markdown_link_target(source, &lines, line, 28).as_deref(),
            Some("https://two.example")
        );
    }

    #[test]
    fn wrapped_unicode_labels_keep_their_destination() {
        let source = "🙂 [界界 long label with words](docs/guide.md#setup) after";
        let lines = crate::wrap_lines(render_markdown_with_width(source, Some(12)), 12);
        let row = lines
            .iter()
            .position(|line| line.to_string().contains("words"))
            .unwrap();
        let text = lines[row].to_string();
        let col = UnicodeWidthStr::width(&text[..text.find("words").unwrap()]);
        assert_eq!(
            markdown_link_target(source, &lines, row, col).as_deref(),
            Some("docs/guide.md#setup")
        );
        assert_eq!(markdown_link_target(source, &lines, 0, 0), None);
    }
}
