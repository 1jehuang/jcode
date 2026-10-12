use super::*;

#[test]
fn compact_picker_frames_render_selected_title_without_panicking() {
    for (width, height) in [(38, 14), (20, 5), (57, 1), (1, 1)] {
        let session = make_session(
            "compact-session",
            "compact-session",
            false,
            SessionStatus::Closed,
        );
        let mut picker = SessionPicker::new(vec![session]);
        picker.last_list_area = Some(Rect::new(0, 0, width, height));
        picker.last_preview_area = Some(Rect::new(0, 0, width, height));
        let backend = ratatui::backend::TestBackend::new(width, height);
        let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");

        terminal
            .draw(|frame| picker.render(frame))
            .expect("compact picker render");

        assert_eq!(terminal.backend().buffer()[(0, 0)].symbol(), "T");
        assert!(picker.last_list_area.is_none());
        assert!(picker.last_preview_area.is_none());
    }
}
