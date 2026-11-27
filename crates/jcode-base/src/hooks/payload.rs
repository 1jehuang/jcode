//! Claude-shaped JSON payloads for hooks that receive a document on stdin.
//!
//! Field names follow Claude Code (`hook_event_name`, `session_id`, `cwd`) so
//! frameworks that already parse that contract work unchanged. jcode-only data
//! lives under a `jcode` object. The same builder is used by every event that
//! gets a JSON payload so the common fields stay identical across events.

use serde_json::{Map, Value};

/// Maximum chars of any free-text field (prompt, assistant text, reasons).
pub const TEXT_FIELD_LIMIT: usize = 10_000;

/// Truncate `text` to `limit` bytes on a UTF-8 boundary, marking the cut.
pub fn truncate_marked(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut end = limit;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[truncated]", &text[..end])
}

/// Common fields for an event payload. `hook_event_name` is the Claude-style
/// name (`UserPromptSubmit`) and `event` the jcode key (`user_prompt_submit`).
pub fn common_payload(
    hook_event_name: &str,
    event: &str,
    session_id: &str,
    cwd: Option<&str>,
) -> Map<String, Value> {
    let mut map = Map::new();
    map.insert("hook_event_name".into(), hook_event_name.into());
    map.insert("session_id".into(), session_id.into());
    if let Some(cwd) = cwd {
        map.insert("cwd".into(), cwd.into());
    }
    let mut jcode = Map::new();
    jcode.insert("event".into(), event.into());
    jcode.insert("scope".into(), "global".into());
    map.insert("jcode".into(), Value::Object(jcode));
    map
}

/// Mutable access to the `jcode` object of a payload built by
/// [`common_payload`].
pub fn jcode_object(payload: &mut Map<String, Value>) -> &mut Map<String, Value> {
    let entry = payload
        .entry("jcode")
        .or_insert_with(|| Value::Object(Map::new()));
    if !entry.is_object() {
        *entry = Value::Object(Map::new());
    }
    match entry {
        Value::Object(map) => map,
        _ => unreachable!("the entry was just made an object"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_payload_has_claude_fields_and_jcode_object() {
        let payload = common_payload("Stop", "stop", "ses_1", Some("/work"));
        assert_eq!(payload["hook_event_name"], "Stop");
        assert_eq!(payload["session_id"], "ses_1");
        assert_eq!(payload["cwd"], "/work");
        assert_eq!(payload["jcode"]["event"], "stop");
        assert_eq!(payload["jcode"]["scope"], "global");
    }

    #[test]
    fn truncate_marked_cuts_on_char_boundary_and_marks() {
        assert_eq!(truncate_marked("short", 100), "short");
        let cut = truncate_marked("héllo wörld", 2);
        assert!(cut.ends_with("[truncated]"));
        assert!(cut.starts_with('h'));
    }
}
