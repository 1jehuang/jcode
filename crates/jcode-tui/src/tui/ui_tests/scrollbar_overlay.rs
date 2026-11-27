// The chat scrollbar shares the rightmost column with info widgets that dock
// against the right edge. Widgets render after the transcript, so without a
// second pass their border would paint over the thumb.

use super::super::NativeScrollbar;
use ratatui::{Terminal, backend::TestBackend, layout::Rect, widgets::Block, widgets::Borders};

#[test]
fn scrollbar_junctions_survive_every_thumb_position() {
    use ratatui::widgets::BorderType;
    for focused in [false, true] {
        for scroll in 0..=30 {
            let mut terminal = Terminal::new(TestBackend::new(20, 12)).unwrap();
            let scrollbar = NativeScrollbar {
                area: Rect::new(19, 1, 1, 10),
                scroll,
                total_lines: 40,
                visible_height: 10,
                focused,
            };
            terminal
                .draw(|frame| {
                    scrollbar.render(frame);
                    for y in [2, 7] {
                        frame.render_widget(
                            Block::default()
                                .borders(Borders::ALL)
                                .border_type(BorderType::Rounded),
                            Rect::new(8, y, 12, 3),
                        );
                    }
                    scrollbar.render_thumb_overlay(frame);
                })
                .unwrap();
            let buf = terminal.backend().buffer();
            let thumb =
                super::super::native_scrollbar_thumb(scrollbar.area, scroll, 40, 10).unwrap();
            for y in [2, 4, 7, 9] {
                let expected = if thumb.contains(usize::from(y - 1)) {
                    "┤"
                } else if y == 2 || y == 7 {
                    "╮"
                } else {
                    "╯"
                };
                assert_eq!(buf[(19, y)].symbol(), expected, "scroll={scroll}, y={y}");
                assert_eq!(buf[(18, y)].symbol(), "─");
                if thumb.contains(usize::from(y - 1)) {
                    assert_eq!(
                        buf[(19, y)].fg,
                        super::super::native_scrollbar_thumb_color(focused)
                    );
                }
            }
            // Between/outside widgets, only the thumb may occupy the track.
            for y in [1, 5, 6, 10] {
                assert_eq!(
                    buf[(19, y)].symbol() == " ",
                    !thumb.contains(usize::from(y - 1)),
                    "unexpected track at scroll={scroll}, y={y}"
                );
            }
            assert_eq!(buf[(19, 0)].symbol(), " ");
            assert_eq!(buf[(19, 11)].symbol(), " ");
        }
    }
}

#[test]
fn scrollbar_overlay_without_overflow_leaves_corners_unchanged() {
    let mut terminal = Terminal::new(TestBackend::new(10, 10)).unwrap();
    terminal
        .draw(|frame| {
            frame.render_widget(
                Block::default().borders(Borders::ALL),
                Rect::new(4, 1, 6, 8),
            );
            NativeScrollbar {
                area: Rect::new(9, 0, 1, 10),
                scroll: 0,
                total_lines: 10,
                visible_height: 10,
                focused: false,
            }
            .render_thumb_overlay(frame);
        })
        .unwrap();
    assert_eq!(terminal.backend().buffer()[(9, 1)].symbol(), "┐");
    assert_eq!(terminal.backend().buffer()[(9, 8)].symbol(), "┘");
}

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
    assert_eq!(buf[(9, 0)].symbol(), "┤");
    assert_eq!(buf[(9, 1)].symbol(), "│");
    assert_eq!(buf[(9, 2)].symbol(), "╵");
    // Track rows below the thumb keep the widget border visible.
    assert_eq!(buf[(9, 3)].symbol(), "│");
    assert_eq!(buf[(9, 7)].symbol(), "┘");
    // Outside the thumb and widget, the track remains invisible.
    assert_eq!(buf[(9, 8)].symbol(), " ");
}
