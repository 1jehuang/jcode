use super::App;

impl App {
    pub(in crate::tui::app) fn build_side_panel_latency_snapshot(
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

    pub(in crate::tui::app) fn build_scroll_test_content(
        diagrams: usize,
        padding: usize,
        override_diagram: Option<&str>,
    ) -> String {
        let mut out = String::new();
        let intro_lines = padding.max(4);
        for i in 0..intro_lines {
            out.push_str(&format!(
                "Intro line {:02} - quick brown fox jumps over the lazy dog.\n",
                i + 1
            ));
        }

        let diagram_templates = [
            r#"flowchart TD
    A[Start] --> B{Decision}
    B -->|Yes| C[Process 1]
    B -->|No| D[Process 2]
    C --> E[Merge]
    D --> E
    E --> F[End]"#,
            r#"sequenceDiagram
    participant U as User
    participant A as App
    participant S as Service
    U->>A: Scroll request
    A->>S: Render diagram
    S-->>A: PNG
    A-->>U: Draw frame"#,
            r#"stateDiagram-v2
    [*] --> Idle
    Idle --> Scrolling: input
    Scrolling --> Rendering: diagram
    Rendering --> Idle: frame drawn"#,
        ];

        for idx in 0..diagrams {
            let diagram =
                override_diagram.unwrap_or(diagram_templates[idx % diagram_templates.len()]);
            out.push_str("```mermaid\n");
            out.push_str(diagram);
            out.push_str("\n```\n");

            for j in 0..padding {
                out.push_str(&format!(
                    "After diagram {} line {:02} - stretch content for scrolling.\n",
                    idx + 1,
                    j + 1
                ));
            }
        }

        out
    }
}
