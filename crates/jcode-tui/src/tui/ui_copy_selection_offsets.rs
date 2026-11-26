use super::*;

fn copy_pane_line_text(pane: crate::tui::CopySelectionPane, abs_line: usize) -> Option<String> {
    copy_snapshot_for_pane(pane)?
        .wrapped_plain_line(abs_line)
        .map(str::to_owned)
}

pub(crate) fn copy_viewport_line_text(abs_line: usize) -> Option<String> {
    copy_pane_line_text(crate::tui::CopySelectionPane::Chat, abs_line)
}

pub(crate) fn side_pane_line_text(abs_line: usize) -> Option<String> {
    copy_pane_line_text(crate::tui::CopySelectionPane::SidePane, abs_line)
}

pub(crate) fn input_pane_line_text(abs_line: usize) -> Option<String> {
    copy_pane_line_text(crate::tui::CopySelectionPane::Input, abs_line)
}

fn copy_pane_line_count(pane: crate::tui::CopySelectionPane) -> Option<usize> {
    Some(copy_snapshot_for_pane(pane)?.wrapped_plain_line_count())
}

pub(crate) fn copy_viewport_line_count() -> Option<usize> {
    copy_pane_line_count(crate::tui::CopySelectionPane::Chat)
}

pub(crate) fn side_pane_line_count() -> Option<usize> {
    copy_pane_line_count(crate::tui::CopySelectionPane::SidePane)
}

pub(crate) fn input_pane_line_count() -> Option<usize> {
    copy_pane_line_count(crate::tui::CopySelectionPane::Input)
}

pub(crate) fn input_byte_offset_for_copy_point(
    input: &str,
    point: crate::tui::CopySelectionPoint,
) -> Option<usize> {
    if point.pane != crate::tui::CopySelectionPane::Input {
        return None;
    }
    let snapshot = copy_snapshot_for_pane(crate::tui::CopySelectionPane::Input)?;
    let (raw_line, column) = copy_selection::raw_line_column_from_snapshot(&snapshot, point)?;
    let mut offset = 0usize;
    for (index, line) in input.split('\n').enumerate() {
        if index == raw_line {
            return Some(offset + display_width::display_col_to_byte_offset(line, column));
        }
        offset += line.len() + 1;
    }
    Some(input.len())
}

pub(crate) fn input_copy_point_for_byte_offset(
    input: &str,
    byte: usize,
) -> Option<crate::tui::CopySelectionPoint> {
    let before = input.get(..byte)?;
    let raw_line = before.bytes().filter(|&byte| byte == b'\n').count();
    let column = before.rsplit('\n').next()?.width();
    let snapshot = copy_snapshot_for_pane(crate::tui::CopySelectionPane::Input)?;
    (0..snapshot.wrapped_plain_line_count()).find_map(|abs_line| {
        let map = snapshot.wrapped_line_map(abs_line)?;
        (map.raw_line == raw_line && (map.start_col..=map.end_col).contains(&column)).then(|| {
            crate::tui::CopySelectionPoint {
                pane: crate::tui::CopySelectionPane::Input,
                abs_line,
                column: column - map.start_col
                    + snapshot.wrapped_copy_offset(abs_line).unwrap_or(0),
            }
        })
    })
}
