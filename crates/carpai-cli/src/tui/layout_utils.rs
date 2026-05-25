use super::visual_debug::RectCapture;
pub(crate) use jcode_tui_render::layout::{parse_area_spec, point_in_rect, rect_contains};
use ratatui::layout::Rect;

pub(crate) fn rect_from_capture(rect: RectCapture) -> Rect {
    Rect {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_from_capture_copies_all_fields() {
        let rect = rect_from_capture(RectCapture {
            x: 3,
            y: 5,
            width: 8,
            height: 13,
        });

        assert_eq!(rect, Rect::new(3, 5, 8, 13));
    }
}
