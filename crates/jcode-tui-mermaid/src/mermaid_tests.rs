use super::*;
use ratatui::prelude::Line;

#[test]
fn mermaid_language_detection_accepts_common_variants() {
    assert!(is_mermaid_lang("mermaid"));
    assert!(is_mermaid_lang("Mermaid"));
    assert!(is_mermaid_lang("mermaid-flowchart"));
    assert!(is_mermaid_lang("mermaid:sequence"));

    assert!(!is_mermaid_lang(""));
    assert!(!is_mermaid_lang("graphviz"));
    assert!(!is_mermaid_lang("not-mermaid"));
}

#[test]
fn image_placeholder_markdown_round_trips_hash() {
    let hash = 0x0123_4567_89ab_cdef;
    let markdown = image_widget_placeholder_markdown(hash);
    let line = Line::from(markdown.trim_end().to_string());

    assert_eq!(parse_image_placeholder(&line), Some(hash));
}

#[test]
fn image_placeholder_parser_rejects_malformed_markers() {
    for content in [
        "",
        "not a marker",
        "<!-- JCODE_MERMAID_IMAGE: -->",
        "<!-- JCODE_MERMAID_IMAGE: not-hex -->",
        "prefix <!-- JCODE_MERMAID_IMAGE: 0123456789abcdef -->",
        "<!-- JCODE_MERMAID_IMAGE: 0123456789abcdef --> suffix",
    ] {
        let line = Line::from(content.to_string());
        assert_eq!(parse_image_placeholder(&line), None, "content: {content:?}");
    }
}

#[test]
fn estimate_image_height_uses_terminal_cell_aspect_and_caps_large_images() {
    assert_eq!(estimate_image_height(400, 200, 40), 10);
    assert_eq!(estimate_image_height(400, 1, 40), 1);
    assert_eq!(estimate_image_height(1, 400, 40), 30);
}

#[test]
fn proc_status_parser_reads_kib_values_as_bytes() {
    let status = "Name:\tjcode\nVmRSS:\t  1234 kB\nVmPeak:\t 5 kB\n";

    assert_eq!(
        parse_proc_status_value_bytes(status, "VmRSS:"),
        Some(1234 * 1024)
    );
    assert_eq!(
        parse_proc_status_value_bytes(status, "VmPeak:"),
        Some(5 * 1024)
    );
    assert_eq!(parse_proc_status_value_bytes(status, "VmSize:"), None);
}

#[test]
fn error_lines_include_error_message() {
    let lines = error_to_lines("parse failed");
    let rendered = lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("mermaid error"));
    assert!(rendered.contains("parse failed"));
}
