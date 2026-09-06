#[cfg(test)]
use super::TuiState;
#[cfg(test)]
use ratatui::text::Line;

#[cfg(test)]
pub(crate) fn inline_ui_gap_height(app: &dyn TuiState) -> u16 {
    if app.inline_ui_state().is_some() {
        1
    } else {
        0
    }
}

#[cfg(test)]
pub(crate) fn extract_line_text(line: &Line) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}
