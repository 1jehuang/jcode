use ratatui::prelude::*;

const INLINE_IMAGE_MARKER_PREFIX: &str = "\x00IIMG:";
const INLINE_IMAGE_MARKER_SUFFIX: &str = ":END";

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

pub fn native_image_protocol_available() -> bool {
    false
}

#[cfg(test)]
pub fn with_image_protocol_override<T>(_enabled: Option<bool>, f: impl FnOnce() -> T) -> T {
    f()
}

pub fn get_font_size() -> Option<(u16, u16)> {
    None
}

/// Monotonic deferred-render epoch. The fallback renderer never defers, so
/// the epoch never advances.
pub fn deferred_render_epoch() -> u64 {
    0
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

pub fn parse_image_placeholder(_line: &Line<'_>) -> Option<u64> {
    None
}

pub fn inline_image_placeholder_lines(hash: u64, rows: u16, cols: u16) -> Vec<Line<'static>> {
    let rows = rows.max(1);
    let mut lines = Vec::with_capacity(rows as usize);
    lines.push(Line::from(format!(
        "{INLINE_IMAGE_MARKER_PREFIX}{hash:016x}:{rows:04x}:{cols:04x}{INLINE_IMAGE_MARKER_SUFFIX}"
    )));
    lines.extend((1..rows).map(|_| Line::default()));
    lines
}

pub fn parse_inline_image_placeholder(line: &Line<'_>) -> Option<(u64, u16, u16)> {
    let content = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .find(|content| !content.trim().is_empty())?;
    let rest = content.strip_prefix(INLINE_IMAGE_MARKER_PREFIX)?;
    let rest = rest.strip_suffix(INLINE_IMAGE_MARKER_SUFFIX)?;
    let mut parts = rest.split(':');
    let hash = u64::from_str_radix(parts.next()?, 16).ok()?;
    let rows = u16::from_str_radix(parts.next()?, 16).ok()?;
    let cols = u16::from_str_radix(parts.next()?, 16).ok()?;
    (parts.next().is_none()).then_some((hash, rows, cols))
}

pub fn register_external_image(_path: &std::path::Path, _width: u32, _height: u32) -> u64 {
    0
}
