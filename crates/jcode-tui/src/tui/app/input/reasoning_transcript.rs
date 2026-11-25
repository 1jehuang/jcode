/// Remove reasoning-marked lines from committed transcript text. Reasoning lines
/// are wrapped in emphasis containing the invisible [`REASONING_SENTINEL`]
/// (see `jcode_tui_markdown::reasoning_line_markup`). Trailing blank lines left
/// behind are trimmed so the remaining answer renders cleanly.
pub(in crate::tui::app) fn strip_reasoning_lines(content: &str) -> String {
    let sentinel = jcode_tui_markdown::REASONING_SENTINEL;
    let mut out_lines: Vec<&str> = Vec::new();
    for line in content.split('\n') {
        if line.contains(sentinel) {
            continue;
        }
        out_lines.push(line);
    }
    // Collapse runs of blank lines created by removed reasoning blocks, and trim
    // leading/trailing blank lines.
    let mut result = String::with_capacity(content.len());
    let mut prev_blank = true; // suppress leading blanks
    for line in out_lines {
        let is_blank = line.trim().is_empty();
        if is_blank && prev_blank {
            continue;
        }
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str(line);
        prev_blank = is_blank;
    }
    result.trim_end().to_string()
}
