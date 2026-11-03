use super::*;

/// Render markdown with optional width constraint for tables
pub fn wrap_line(line: Line<'static>, width: usize) -> Vec<Line<'static>> {
    wrap::wrap_line(line, width, repeated_gutter_prefix)
}

pub fn wrap_lines(lines: Vec<Line<'static>>, width: usize) -> Vec<Line<'static>> {
    wrap::wrap_lines(lines, width, repeated_gutter_prefix)
}

pub fn progress_bar(progress: f32, width: usize) -> String {
    wrap::progress_bar(progress, width)
}

pub fn progress_line(label: &str, progress: f32, width: usize) -> Line<'static> {
    wrap::progress_line(label, progress, width)
}
