// The chat scrollbar shares the rightmost column with info widgets that dock
// against the right edge. Widgets render after the transcript, so without a
// second pass their border would paint over the thumb.

use super::super::NativeScrollbar;
use ratatui::{Terminal, backend::TestBackend, layout::Rect, widgets::Block, widgets::Borders};

#[test]
fn scrollbar_thumb_overlay_draws_in_front_of_widget_border() {
    let backend = TestBackend::new(10, 10);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let scrollbar = NativeScrollbar {
        area: Rect::new(9, 0, 1, 10),
        scroll: 0,
        total_lines: 40,
        visible_height: 10,
        focused: false,
    };
    terminal
        .draw(|frame| {
            scrollbar.render(frame);
            // Info widget box whose right border lands in the scrollbar column.
            frame.render_widget(
                Block::default().borders(Borders::ALL),
                Rect::new(4, 0, 6, 8),
            );
            scrollbar.render_thumb_overlay(frame);
        })
        .expect("draw");
    let buf = terminal.backend().buffer();
    // Thumb: 10 rows * 10 / 40 = 3 rows at the top.
    assert_eq!(buf[(9, 0)].symbol(), "╷");
    assert_eq!(buf[(9, 1)].symbol(), "│");
    assert_eq!(buf[(9, 2)].symbol(), "╵");
    // Track rows below the thumb keep the widget border visible.
    assert_eq!(buf[(9, 3)].symbol(), "│");
    assert_eq!(buf[(9, 7)].symbol(), "┘");
    // Below the widget the track stays empty.
    assert_eq!(buf[(9, 8)].symbol(), " ");
}
