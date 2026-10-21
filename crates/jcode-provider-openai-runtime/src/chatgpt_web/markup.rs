//! Remove renderer decoration without rewriting JSON tool arguments.

use super::{TOOL_CALL_END, TOOL_CALL_START};

pub(super) fn strip_web_markup(response: &str) -> String {
    let mut text = String::with_capacity(response.len());
    let mut rest = response;
    let (mut in_tool, mut in_string, mut escaped) = (false, false, false);
    while !rest.is_empty() {
        if !in_string {
            if rest.starts_with(TOOL_CALL_START) {
                in_tool = true;
                text.push_str(TOOL_CALL_START);
                rest = &rest[TOOL_CALL_START.len()..];
                continue;
            }
            if rest.starts_with(TOOL_CALL_END) {
                in_tool = false;
                text.push_str(TOOL_CALL_END);
                rest = &rest[TOOL_CALL_END.len()..];
                continue;
            }
            if let Some(tag) = ["<escape>", "</escape>"]
                .into_iter()
                .find(|tag| rest.starts_with(*tag))
            {
                rest = &rest[tag.len()..];
                continue;
            }
            if rest.starts_with("<Cite ") {
                if let Some(end) = rest.find("/>") {
                    text.truncate(text.trim_end_matches(' ').len());
                    rest = &rest[end + 2..];
                    continue;
                }
            }
        }
        let ch = rest.chars().next().expect("nonempty suffix");
        text.push(ch);
        rest = &rest[ch.len_utf8()..];
        if in_tool {
            if in_string {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    in_string = false;
                }
            } else if ch == '"' {
                in_string = true;
            }
        }
    }
    text
}
