#[test]
fn viewport_renderers_return_zero_for_empty_areas() {
    let area = ratatui::prelude::Rect::new(0, 0, 0, 0);
    let mut buf = ratatui::buffer::Buffer::empty(ratatui::prelude::Rect::new(0, 0, 1, 1));

    assert_eq!(
        super::render_image_widget_viewport(0xabc, area, &mut buf, 0, 0, 100, false),
        0
    );
    assert_eq!(
        super::render_image_widget_viewport_precise(0xabc, area, &mut buf, 0, 0, 1000, false),
        0
    );
}
