use ratatui::text::Line;

pub(crate) fn is_rendered_table_line(line: &Line<'_>) -> bool {
    let text: String = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();
    text.contains(" │ ") || text.contains("─┼─")
}
