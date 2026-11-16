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
