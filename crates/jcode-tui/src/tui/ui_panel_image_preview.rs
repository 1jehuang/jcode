//! Click targets are rebuilt from actual image widget rectangles each frame.
//! Keeping them separate from text selection allows a click to preview an image
//! without changing the existing drag-to-copy behavior in the side panel.
use super::*;
use std::cell::RefCell;

thread_local! {
    static IMAGE_REGIONS: RefCell<Vec<(Rect, u64)>> = const { RefCell::new(Vec::new()) };
}

pub(crate) fn clear_regions() {
    IMAGE_REGIONS.with(|regions| regions.borrow_mut().clear());
}

pub(super) fn record_image(area: Rect, hash: u64) {
    if area.width > 0 && area.height > 0 {
        IMAGE_REGIONS.with(|regions| regions.borrow_mut().push((area, hash)));
    }
}

pub(crate) fn image_at(column: u16, row: u16) -> Option<u64> {
    IMAGE_REGIONS.with(|regions| {
        regions
            .borrow()
            .iter()
            .rev()
            .find_map(|(area, hash)| area.contains((column, row).into()).then_some(*hash))
    })
}

pub(super) fn draw_preview(frame: &mut Frame, area: Rect, hash: u64) {
    let block = ratatui::widgets::Block::bordered()
        .title(" Image preview ")
        .title_bottom(" Click or Esc to close ")
        .border_style(Style::default().fg(tool_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.is_empty() {
        return;
    }
    if let Some((_, width, height)) = mermaid::get_cached_png(hash) {
        let image_area = preview_image_area(inner, width, height, mermaid::get_font_size());
        mermaid::render_image_widget_scale(hash, image_area, frame.buffer_mut(), false);
    } else {
        frame.render_widget(
            Paragraph::new("Image is no longer available. Press Esc to return.")
                .style(Style::default().fg(dim_color())),
            inner,
        );
    }
}

fn preview_image_area(area: Rect, width: u32, height: u32, font_size: Option<(u16, u16)>) -> Rect {
    let (cell_w, cell_h) = font_size.unwrap_or((8, 16));
    let scale = (f64::from(area.width) * f64::from(cell_w.max(1)) / f64::from(width.max(1)))
        .min(f64::from(area.height) * f64::from(cell_h.max(1)) / f64::from(height.max(1)));
    let fitted_width = ((f64::from(width) * scale / f64::from(cell_w.max(1))).floor() as u16)
        .max(1)
        .min(area.width);
    let fitted_height = ((f64::from(height) * scale / f64::from(cell_h.max(1))).floor() as u16)
        .max(1)
        .min(area.height);
    Rect::new(
        area.x + area.width.saturating_sub(fitted_width) / 2,
        area.y + area.height.saturating_sub(fitted_height) / 2,
        fitted_width,
        fitted_height,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_image_preview_targets_only_visible_image_cells() {
        clear_regions();
        record_image(Rect::new(40, 5, 20, 10), 42);
        assert_eq!(image_at(40, 5), Some(42));
        assert_eq!(image_at(59, 14), Some(42));
        for (x, y) in [(39, 5), (60, 5), (40, 4), (40, 15)] {
            assert_eq!(image_at(x, y), None);
        }
        clear_regions();
        assert_eq!(image_at(40, 5), None);
    }

    #[test]
    fn panel_image_preview_fits_and_centers_wide_and_tall_images() {
        let area = Rect::new(1, 1, 100, 30);
        assert_eq!(
            preview_image_area(area, 1600, 400, Some((8, 16))),
            Rect::new(1, 10, 100, 12)
        );
        assert_eq!(
            preview_image_area(area, 200, 1200, Some((8, 16))),
            Rect::new(46, 1, 10, 30)
        );
        // Small images are enlarged too, rather than stuck at native size.
        assert_eq!(
            preview_image_area(area, 16, 4, Some((8, 16))),
            Rect::new(1, 10, 100, 12)
        );
    }
}
