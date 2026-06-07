use ratatui::prelude::*;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum RenderResult {
    Image {
        hash: u64,
        path: std::path::PathBuf,
        width: u32,
        height: u32,
    },
    Error(String),
}

pub fn is_mermaid_lang(lang: &str) -> bool {
    lang.eq_ignore_ascii_case("mermaid") || lang.eq_ignore_ascii_case("mmd")
}

pub fn image_protocol_available() -> bool {
    false
}

pub fn render_mermaid_deferred_with_stream_scope(
    _content: &str,
    _terminal_width: Option<u16>,
    _stream_sequence: u64,
) -> Option<RenderResult> {
    Some(RenderResult::Error(
        "Mermaid rendering is disabled".to_string(),
    ))
}

pub fn render_mermaid_deferred_with_registration(
    _content: &str,
    _terminal_width: Option<u16>,
    _register_active: bool,
) -> Option<RenderResult> {
    Some(RenderResult::Error(
        "Mermaid rendering is disabled".to_string(),
    ))
}

pub fn render_mermaid_untracked(_content: &str, _terminal_width: Option<u16>) -> RenderResult {
    RenderResult::Error("Mermaid rendering is disabled".to_string())
}

pub fn render_mermaid_sized(_content: &str, _terminal_width: Option<u16>) -> RenderResult {
    RenderResult::Error("Mermaid rendering is disabled".to_string())
}

pub fn set_streaming_preview_diagram(
    _hash: u64,
    _width: u32,
    _height: u32,
    _label: Option<String>,
) {
}

pub fn result_to_lines(result: RenderResult, _max_width: Option<usize>) -> Vec<Line<'static>> {
    match result {
        RenderResult::Image { .. } => Vec::new(),
        RenderResult::Error(message) => vec![Line::from(message)],
    }
}
