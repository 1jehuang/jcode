use super::*;
use crate::tui::info_widget_overview::compute_page_layout;
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthStr;

fn pending() -> InfoWidgetData {
    InfoWidgetData {
        startup_runtime_pending: true,
        startup_usage_pending: true,
        provider_name: Some("openai".into()),
        auth_method: AuthMethod::OpenAIOAuth,
        git_info: Some(GitInfo {
            branch: "master".into(),
            modified: 1,
            dirty_files: vec![DirtyFile::new('M', "src/main.rs")],
            dirty_total: 1,
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

#[test]
fn startup_slots_keep_changes_at_same_row_as_runtime_then_usage_arrive() {
    let initial = pending();
    let mut runtime = initial.clone();
    runtime.startup_runtime_pending = false;
    runtime.service_tier = Some("priority".into());
    runtime.session_name = Some("otter".into());
    let mut ready = runtime.clone();
    ready.startup_usage_pending = false;
    ready.usage_info = Some(UsageInfo {
        provider: UsageProvider::OpenAI,
        available: true,
        primary_limit_label: Some("5-hour".into()),
        five_hour: 0.42,
        ..Default::default()
    });

    for width in [100, 140] {
        let inner = Rect::new(0, 0, width, 40);
        let frames: Vec<_> = [&initial, &runtime, &ready]
            .into_iter()
            .map(|data| render_sections(data, inner, None))
            .collect();
        let row = |lines: &Vec<Line<'static>>| {
            lines
                .iter()
                .position(|line| line_text(line).contains("src/main.rs"))
                .expect("changed file rendered")
        };
        assert_eq!(row(&frames[0]), row(&frames[1]));
        assert_eq!(row(&frames[1]), row(&frames[2]));
        for (data, lines) in [&initial, &runtime, &ready].into_iter().zip(&frames) {
            let layout = compute_page_layout(data, width as usize, 40);
            assert_eq!(layout.max_page_height as usize, lines.len());
            assert_eq!(
                calculate_widget_height(WidgetKind::Overview, data, width, 40),
                calculate_widget_height(WidgetKind::Overview, &initial, width, 40)
            );
        }
        for data in [&initial, &runtime, &ready] {
            assert_eq!(
                render_widget_content(WidgetKind::UsageLimits, data, inner)
                    .lines
                    .len(),
                1
            );
            assert_eq!(
                calculate_widget_height(WidgetKind::UsageLimits, data, width, 40),
                3
            );
        }
        let mut anchors = Vec::new();
        let mut overview_rects = Vec::new();
        let margins = Margins {
            right_widths: vec![40; 40],
            ..Default::default()
        };
        for data in [&initial, &runtime, &ready] {
            let placed = crate::tui::info_widget_layout::calculate_placements_anchored(
                inner, &margins, data, true, &anchors,
            );
            overview_rects.push(
                placed
                    .visible
                    .iter()
                    .find(|widget| widget.kind == WidgetKind::Overview)
                    .expect("overview should be placed")
                    .rect,
            );
            anchors = placed.anchors;
        }
        assert_eq!(overview_rects[0], overview_rects[1]);
        assert_eq!(overview_rects[1], overview_rects[2]);
        let text = frames[0]
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Loading…"));
        assert!(!text.contains('%'));
        assert!(!text.contains("fast tier"));
    }
}

#[test]
fn startup_runtime_slots_preserve_partial_and_extra_metadata() {
    let mut data = InfoWidgetData {
        startup_runtime_pending: true,
        ..Default::default()
    };
    assert_eq!(model_info_height(&data), 2);
    data.session_name = Some("otter".into());
    assert_eq!(model_info_height(&data), 2);
    data.connection_type = Some("websocket".into());
    data.upstream_provider = Some("upstream".into());
    assert_eq!(model_info_height(&data), 3);
    data.startup_runtime_pending = false;
    assert_eq!(model_info_height(&data), 3);
    data.session_name = None;
    data.connection_type = None;
    data.upstream_provider = None;
    assert_eq!(model_info_height(&data), 0);
}

#[test]
fn startup_usage_slots_are_only_for_pending_openai_oauth() {
    for auth in [
        AuthMethod::Unknown,
        AuthMethod::OpenAIApiKey,
        AuthMethod::AnthropicOAuth,
    ] {
        let data = InfoWidgetData {
            auth_method: auth,
            ..pending()
        };
        assert!(!crate::tui::info_widget_overview::startup_usage_pending(
            &data
        ));
        assert!(!data.has_data_for(WidgetKind::UsageLimits));
    }
    let data = InfoWidgetData {
        startup_runtime_pending: false,
        startup_usage_pending: false,
        git_info: None,
        ..pending()
    };
    assert!(!data.has_data_for(WidgetKind::UsageLimits));
    assert_eq!(model_info_height(&data), 0);
    assert!(render_sections(&data, Rect::new(0, 0, 30, 10), None).is_empty());
}

#[test]
fn startup_placeholders_fit_even_zero_or_one_cell_width() {
    let data = InfoWidgetData {
        git_info: None,
        ..pending()
    };
    for width in 0..20 {
        let lines = render_sections(&data, Rect::new(0, 0, width, 10), None);
        assert_eq!(lines.len(), 4);
        for line in lines {
            let text = line_text(&line);
            assert!(
                UnicodeWidthStr::width(text.as_str()) <= width as usize,
                "{width}: {text:?}"
            );
        }
    }
}

#[test]
fn known_usage_and_limit_warning_override_startup_reservation() {
    for limit_reached in [false, true] {
        let data = InfoWidgetData {
            usage_info: Some(UsageInfo {
                provider: UsageProvider::OpenAI,
                available: !limit_reached,
                limit_reached,
                primary_limit_label: Some("5-hour".into()),
                secondary_limit_label: Some("Weekly".into()),
                ..Default::default()
            }),
            ..pending()
        };
        assert!(!crate::tui::info_widget_overview::startup_usage_pending(
            &data
        ));
        let rows = crate::tui::info_widget_overview::render_overview_usage(&data, 40);
        assert_eq!(rows.len(), if limit_reached { 1 } else { 3 });
        assert!(!rows.iter().any(|line| line_text(line).contains("Loading")));
        let all = render_sections(&data, Rect::new(0, 0, 40, 30), None);
        assert_eq!(
            compute_page_layout(&data, 40, 30).max_page_height as usize,
            all.len()
        );
    }
}
