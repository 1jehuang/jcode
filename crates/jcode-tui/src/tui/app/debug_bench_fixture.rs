use super::*;

impl App {
    pub(super) fn build_side_panel_latency_snapshot(
        diagrams: usize,
        padding: usize,
    ) -> crate::side_panel::SidePanelSnapshot {
        let content = Self::build_scroll_test_content(diagrams, padding, None);
        crate::side_panel::SidePanelSnapshot {
            focus_revision: 0,
            focused_page_id: Some("latency_bench".to_string()),
            pages: vec![crate::side_panel::SidePanelPage {
                id: "latency_bench".to_string(),
                title: "Latency Bench".to_string(),
                file_path: "latency_bench.md".to_string(),
                format: crate::side_panel::SidePanelPageFormat::Markdown,
                pdf_data: None,
                source: crate::side_panel::SidePanelPageSource::Managed,
                content,
                updated_at_ms: 1,
            }],
        }
    }
}
