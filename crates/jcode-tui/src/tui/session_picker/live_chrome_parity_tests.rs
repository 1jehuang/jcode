//! Compare the preview's data adapter to the real live status drawing entrypoint.
use super::*;
use ratatui::{Terminal, backend::TestBackend};

#[test]
fn picker_overscroll_matches_real_live_status_with_same_session_facts() {
    let _lock = viewport_snapshot_test_lock();
    let data = info_widget::InfoWidgetData {
        model: Some("future-model".into()),
        provider_name: Some("selected-provider".into()),
        working_dir: Some("/selected/project".into()),
        reasoning_effort: Some("high".into()),
        ..Default::default()
    };
    let state = TestState {
        provider_model: data.model.clone(),
        provider_name: data.provider_name.clone(),
        working_dir: data.working_dir.clone(),
        info_widget_data: data.clone(),
        ..Default::default()
    };
    for width in [1, 4, 12, 40, 100] {
        let mut live = Terminal::new(TestBackend::new(width, 1)).unwrap();
        let mut preview = Terminal::new(TestBackend::new(width, 1)).unwrap();
        live.draw(|frame| {
            let area = frame.area();
            input_ui::draw_overscroll_status(frame, &state, area);
        })
        .unwrap();
        preview
            .draw(|frame| {
                frame.render_widget(
                    Paragraph::new(input_ui::preview_overscroll_line(&data, width)),
                    frame.area(),
                );
            })
            .unwrap();
        assert_eq!(
            live.backend().buffer(),
            preview.backend().buffer(),
            "width={width}"
        );
    }
}
