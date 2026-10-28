//! Bounded UTF-8-safe transcript search excerpts.
use super::*;

pub(crate) fn push_with_byte_budget(dst: &mut String, src: &str, budget: &mut usize) {
    if *budget == 0 || src.is_empty() {
        return;
    }

    let mut end = src.len().min(*budget);
    while end > 0 && !src.is_char_boundary(end) {
        end -= 1;
    }
    if end == 0 {
        return;
    }

    dst.push_str(&src[..end]);
    *budget = budget.saturating_sub(end);
}

pub(crate) fn suffix_at_most(value: &str, max_bytes: usize) -> &str {
    let mut start = value.len().saturating_sub(max_bytes);
    while start < value.len() && !value.is_char_boundary(start) {
        start += 1;
    }
    &value[start..]
}

/// Keep a bounded sample from both ends of a growing transcript. Keeping only
/// the first 64 KiB made `/resume` search silently blind to every later turn in
/// a long session. The first half retains titles and early prompts while the
/// second half continuously follows the newest transcript content.
pub(crate) fn push_sampled_search_text(dst: &mut String, src: &str, limit: usize) {
    if src.is_empty() || limit == 0 {
        return;
    }
    if dst.len().saturating_add(1).saturating_add(src.len()) <= limit {
        dst.push(' ');
        dst.push_str(src);
        return;
    }

    // An oversized first message has no existing session head to preserve. Keep
    // both ends of that message instead of retaining only its suffix.
    if dst.is_empty() {
        let head_budget = limit / 2;
        let mut head_end = src.len().min(head_budget);
        while head_end > 0 && !src.is_char_boundary(head_end) {
            head_end -= 1;
        }
        dst.push_str(&src[..head_end]);
        dst.push_str(suffix_at_most(src, limit.saturating_sub(head_end)));
        return;
    }

    let head_budget = limit / 2;
    let mut head_end = dst.len().min(head_budget);
    while head_end > 0 && !dst.is_char_boundary(head_end) {
        head_end -= 1;
    }
    let head = dst[..head_end].to_string();

    let tail_budget = limit.saturating_sub(head.len());
    let tail = if src.len().saturating_add(1) >= tail_budget {
        suffix_at_most(src, tail_budget).to_string()
    } else {
        let old_budget = tail_budget.saturating_sub(src.len() + 1);
        let old_tail = suffix_at_most(&dst[head_end..], old_budget);
        let mut tail = String::with_capacity(old_tail.len() + 1 + src.len());
        tail.push_str(old_tail);
        tail.push(' ');
        tail.push_str(src);
        tail
    };

    dst.clear();
    dst.push_str(&head);
    dst.push_str(&tail);
}

pub(crate) fn build_search_index(
    id: &str,
    short_name: &str,
    title: &str,
    working_dir: Option<&str>,
    save_label: Option<&str>,
    messages_preview: &[PreviewMessage],
) -> String {
    let mut combined = String::new();
    combined.push_str(title);
    combined.push(' ');
    combined.push_str(short_name);
    combined.push(' ');
    combined.push_str(id);

    if let Some(dir) = working_dir {
        combined.push(' ');
        combined.push_str(dir);
    }

    if let Some(label) = save_label {
        combined.push(' ');
        combined.push_str(label);
    }

    let mut budget = SEARCH_CONTENT_BUDGET_BYTES;
    for msg in messages_preview {
        let content = msg.content.trim();
        if content.is_empty() {
            continue;
        }
        combined.push(' ');
        push_with_byte_budget(&mut combined, content, &mut budget);
        if budget == 0 {
            break;
        }
    }

    combined.to_lowercase()
}

pub(crate) fn raw_value_search_excerpt(raw: &RawValue, budget: usize) -> Option<String> {
    if budget == 0 {
        return None;
    }
    let raw = raw.get();
    let budget = budget.min(MESSAGE_SEARCH_EXCERPT_BYTES);
    let mut excerpt = String::new();
    push_sampled_search_text(&mut excerpt, raw, budget);
    (!excerpt.is_empty()).then_some(excerpt)
}

pub(crate) fn raw_value_display_text(raw: &RawValue) -> Option<String> {
    fn collect_text(value: &serde_json::Value, out: &mut String) {
        match value {
            serde_json::Value::String(text) => {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(text);
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    collect_text(item, out);
                }
            }
            serde_json::Value::Object(map) => {
                if let Some(text) = map.get("text").and_then(|v| v.as_str()) {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(text);
                }
            }
            _ => {}
        }
    }

    let value: serde_json::Value = match serde_json::from_str(raw.get()) {
        Ok(value) => value,
        Err(error) => {
            crate::logging::debug(&format!("Invalid raw transcript search content: {error}"));
            return None;
        }
    };
    let mut text = String::new();
    collect_text(&value, &mut text);
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}
