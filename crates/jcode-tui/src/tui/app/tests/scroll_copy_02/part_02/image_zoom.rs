use super::*;

#[test]
fn test_ctrl_wheel_over_inline_image_zooms_without_wrapping() {
    use crate::tui::ui::inline_image_ui::{
        AllFit, ImageExpandLevel, InlineImageItem, build_section,
    };
    use jcode_tui_messages::PreparedChatFrame;

    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();

    const IMAGE_ID: u64 = 0xB1AC;
    let chat_width: u16 = 80;
    let items = vec![InlineImageItem {
        id: IMAGE_ID,
        width: 320,
        height: 200,
        label: "pinch.png".to_string(),
        uses_text_fallback: false,
        collapsed_by_default: false,
    }];
    let section = build_section(&items, chat_width, 40, false, true, &AllFit);
    let region = *section
        .image_regions
        .iter()
        .find(|r| r.hash == IMAGE_ID)
        .expect("section should carry the image region");
    let prepared =
        std::sync::Arc::new(PreparedChatFrame::from_single(std::sync::Arc::new(section)));
    let visible_end = prepared.wrapped_plain_line_count();
    let content_area = Rect::new(0, 0, chat_width, visible_end as u16 + 1);
    crate::tui::ui::clear_copy_viewport_snapshot();
    crate::tui::ui::record_copy_viewport_frame_snapshot_for_test(
        prepared,
        0,
        visible_end,
        content_area,
        &vec![0u16; visible_end],
    );

    let row = content_area.y + region.abs_line_idx as u16 + 1;
    let col = content_area.x + region.width / 2;
    let wheel = |app: &mut App, kind: MouseEventKind, column: u16| {
        app.handle_mouse_event(MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::CONTROL,
        })
    };

    wheel(&mut app, MouseEventKind::ScrollUp, col);
    assert_eq!(app.image_expand_level(IMAGE_ID), ImageExpandLevel::Large);
    // Full has the same geometry as Large here, so zooming in further holds.
    wheel(&mut app, MouseEventKind::ScrollUp, col);
    assert_eq!(app.image_expand_level(IMAGE_ID), ImageExpandLevel::Large);
    wheel(&mut app, MouseEventKind::ScrollDown, col);
    assert_eq!(app.image_expand_level(IMAGE_ID), ImageExpandLevel::Fit);
    wheel(&mut app, MouseEventKind::ScrollDown, col);
    assert_eq!(app.image_expand_level(IMAGE_ID), ImageExpandLevel::Fit);

    // Off the image, Ctrl+wheel falls through to chat scrolling.
    wheel(&mut app, MouseEventKind::ScrollUp, chat_width - 2);
    assert_eq!(app.image_expand_level(IMAGE_ID), ImageExpandLevel::Fit);
}
