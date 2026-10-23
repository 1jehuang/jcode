use super::*;

pub(crate) fn line_left_margins_for_area(lines: &[Line<'static>], area_width: u16) -> Vec<u16> {
    lines
        .iter()
        .map(|line| {
            let used = line.width().min(area_width as usize) as u16;
            let total_margin = area_width.saturating_sub(used);
            match line.alignment.unwrap_or(Alignment::Left) {
                Alignment::Left => 0,
                Alignment::Center => total_margin / 2,
                Alignment::Right => total_margin,
            }
        })
        .collect()
}
