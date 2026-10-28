//! Native scrollbar geometry and rendering.
use super::*;

pub(crate) fn split_native_scrollbar_area(area: Rect, enabled: bool) -> (Rect, Option<Rect>) {
    if !enabled || area.width <= 1 {
        return (area, None);
    }

    let content = Rect {
        width: area.width.saturating_sub(1),
        ..area
    };
    let scrollbar = Rect {
        x: area.x.saturating_add(area.width.saturating_sub(1)),
        y: area.y,
        width: 1,
        height: area.height,
    };
    (content, Some(scrollbar))
}

pub(crate) fn native_scrollbar_visible(
    enabled: bool,
    total_lines: usize,
    visible_height: usize,
) -> bool {
    enabled && visible_height > 0 && total_lines > visible_height
}

/// Geometry of a native scrollbar drawn this frame, kept so the thumb can be
/// re-stamped on top of overlays (info widgets) that render afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NativeScrollbar {
    pub area: Rect,
    pub scroll: usize,
    pub total_lines: usize,
    pub visible_height: usize,
    pub focused: bool,
}

impl NativeScrollbar {
    pub(crate) fn render(&self, frame: &mut Frame) {
        render_native_scrollbar(
            frame,
            self.area,
            self.scroll,
            self.total_lines,
            self.visible_height,
            self.focused,
        );
    }

    /// Merge only visible thumb cells with docked widget borders. Leave the
    /// invisible track and all corners outside the thumb untouched.
    pub(crate) fn render_thumb_overlay(&self, frame: &mut Frame) {
        let Some(thumb) = native_scrollbar_thumb(
            self.area,
            self.scroll,
            self.total_lines,
            self.visible_height,
        ) else {
            return;
        };
        let color = native_scrollbar_thumb_color(self.focused);
        let buf = frame.buffer_mut();
        let x = self.area.x;
        for row in thumb.offset..thumb.offset + thumb.height {
            let y = self.area.y + row as u16;
            if let Some(cell) = buf.cell_mut((x, y)) {
                let junction = matches!(cell.symbol(), "╮" | "╯" | "┐" | "┘" | "┤");
                cell.set_symbol(if junction { "┤" } else { thumb.glyph(row) })
                    .set_fg(color);
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct NativeScrollbarThumb {
    pub(crate) offset: usize,
    pub(crate) height: usize,
}

impl NativeScrollbarThumb {
    pub(crate) fn contains(&self, row: usize) -> bool {
        row >= self.offset && row < self.offset + self.height
    }

    pub(crate) fn glyph(&self, row: usize) -> &'static str {
        if self.height == 1 {
            "•"
        } else if row == self.offset {
            "╷"
        } else if row + 1 == self.offset + self.height {
            "╵"
        } else {
            "│"
        }
    }
}

pub(crate) fn native_scrollbar_thumb(
    area: Rect,
    scroll: usize,
    total_lines: usize,
    visible_height: usize,
) -> Option<NativeScrollbarThumb> {
    if area.width == 0
        || area.height == 0
        || !native_scrollbar_visible(true, total_lines, visible_height)
    {
        return None;
    }

    let track_height = area.height as usize;
    let height = ((visible_height * track_height).div_ceil(total_lines)).clamp(1, track_height);
    let max_thumb_offset = track_height.saturating_sub(height);
    let max_scroll = total_lines.saturating_sub(visible_height);
    let offset = if max_scroll == 0 {
        0
    } else {
        scroll.min(max_scroll) * max_thumb_offset / max_scroll
    };
    Some(NativeScrollbarThumb { offset, height })
}

pub(crate) fn native_scrollbar_thumb_color(focused: bool) -> Color {
    if focused {
        rgb(188, 208, 240)
    } else {
        rgb(136, 148, 172)
    }
}

pub(crate) fn render_native_scrollbar(
    frame: &mut Frame,
    area: Rect,
    scroll: usize,
    total_lines: usize,
    visible_height: usize,
    focused: bool,
) {
    let Some(thumb) = native_scrollbar_thumb(area, scroll, total_lines, visible_height) else {
        return;
    };
    let thumb_color = native_scrollbar_thumb_color(focused);
    let lines: Vec<Line> = (0..area.height as usize)
        .map(|row| {
            let (glyph, color) = if thumb.contains(row) {
                (thumb.glyph(row), thumb_color)
            } else {
                (" ", Color::Reset)
            };
            Line::from(Span::styled(glyph, Style::default().fg(color)))
        })
        .collect();

    frame.render_widget(Paragraph::new(lines), area);
}
