use crate::message::ToolCall;

use super::{dim_color, rgb, tool_color, truncate_line_preserving_suffix_to_width};
use ratatui::prelude::*;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Map provider-side tool names to internal display names.
/// Mirrors `Registry::resolve_tool_name` so the TUI shows friendly names.
pub(crate) fn resolve_display_tool_name(name: &str) -> &str {
    match name {
        "communicate" => "swarm",
        "task" | "task_runner" => "subagent",
        "shell_exec" => "bash",
        "file_read" => "read",
        "file_write" => "write",
        "file_edit" => "edit",
        "file_glob" => "glob",
        "file_grep" => "grep",
        "todo_read" | "todo_write" | "todoread" | "todowrite" => "todo",
        other => other,
    }
}

pub(crate) fn canonical_tool_name(name: &str) -> &str {
    match name {
        "communicate" => "swarm",
        "Write" => "write",
        "Edit" => "edit",
        "MultiEdit" => "multiedit",
        "Patch" => "patch",
        "ApplyPatch" => "apply_patch",
        other => other,
    }
}

pub(crate) fn is_edit_tool_name(name: &str) -> bool {
    matches!(
        canonical_tool_name(name),
        "write" | "edit" | "multiedit" | "patch" | "apply_patch"
    )
}

fn parse_nonzero_exit_code_line(line: &str) -> bool {
    let trimmed = line.trim();
    if let Some(rest) = trimmed.strip_prefix("Exit code:") {
        return rest
            .trim()
            .parse::<i32>()
            .map(|code| code != 0)
            .unwrap_or(false);
    }
    if let Some(rest) = trimmed.strip_prefix("--- Command finished with exit code:") {
        return rest
            .trim()
            .trim_end_matches('-')
            .trim()
            .parse::<i32>()
            .map(|code| code != 0)
            .unwrap_or(false);
    }
    false
}

fn infer_bg_action_from_intent_for_display(intent: Option<&str>) -> Option<&'static str> {
    let intent = intent?.trim().to_ascii_lowercase();
    if intent.is_empty() {
        return None;
    }

    if intent.contains("wait") || intent.contains("await") {
        Some("wait")
    } else if intent.contains("tail") {
        Some("tail")
    } else if intent.contains("output") || intent.contains("log") {
        Some("output")
    } else if intent.contains("status") || intent.contains("progress") || intent.contains("check") {
        Some("status")
    } else if intent.contains("cancel") || intent.contains("stop") {
        Some("cancel")
    } else if intent.contains("clean") {
        Some("cleanup")
    } else if intent.contains("list") || intent.contains("show") {
        Some("list")
    } else {
        None
    }
}

fn normalize_backticked_identifier(text: &str) -> String {
    text.replace('`', "").trim().to_string()
}

pub(super) fn concise_tool_error_summary(content: &str) -> Option<String> {
    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }

        let detail = line
            .strip_prefix("Error:")
            .or_else(|| line.strip_prefix("error:"))
            .or_else(|| line.strip_prefix("Failed:"))
            .map(str::trim);
        if let Some(detail) = detail {
            if let Some(field) = detail.strip_prefix("missing field ") {
                return Some(format!(
                    "invalid input: missing {}",
                    normalize_backticked_identifier(field)
                ));
            }
            if detail.starts_with("invalid type") || detail.starts_with("unknown variant") {
                return Some(format!("invalid input: {}", detail));
            }
            if detail.contains("source metadata") && detail.contains("was for") {
                return Some("build source changed before reload".to_string());
            }
            if detail.starts_with("Refusing to publish") {
                return Some("reload refused: rebuild against current source".to_string());
            }
            return Some(format!("error: {}", truncate_middle_display(detail, 80)));
        }

        if line.contains("Compile terminated by signal") {
            return Some(line.to_string());
        }
        if let Some(rest) = line.strip_prefix("Exit code:")
            && let Ok(code) = rest.trim().parse::<i32>()
            && code != 0
        {
            return Some(format!("exit {}", code));
        }
        if let Some(rest) = line.strip_prefix("--- Command finished with exit code:") {
            let code = rest.trim().trim_end_matches('-').trim();
            if code != "0" && !code.is_empty() {
                return Some(format!("exit {}", code));
            }
        }
    }

    None
}

pub(crate) fn tool_output_looks_failed(content: &str) -> bool {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    if concise_tool_error_summary(trimmed).is_some()
        || lower.starts_with("error:")
        || lower.starts_with("failed:")
    {
        return true;
    }

    trimmed.lines().any(|line| {
        let line = line.trim();
        parse_nonzero_exit_code_line(line)
            || line.eq_ignore_ascii_case("Status: failed")
            || line.eq_ignore_ascii_case("failed to start")
            || line.eq_ignore_ascii_case("terminated")
    })
}

#[path = "ui_tools/batch.rs"]
mod batch;

pub(crate) use batch::batch_subcall_params;
#[cfg(test)]
pub(super) use batch::parse_batch_sub_outputs;
pub(super) use batch::{parse_batch_completion_counts, parse_batch_sub_outputs_by_index};

pub(super) fn summarize_unified_patch_input(patch_text: &str) -> String {
    let lines = patch_text.lines().count();
    let mut files: Vec<String> = Vec::new();

    for line in patch_text.lines() {
        let Some(rest) = line
            .strip_prefix("--- ")
            .or_else(|| line.strip_prefix("+++ "))
        else {
            continue;
        };

        let without_tab_suffix = rest.split('\t').next().unwrap_or(rest);
        let path_token = without_tab_suffix.split_whitespace().next().unwrap_or("");
        let path = path_token
            .strip_prefix("a/")
            .or(path_token.strip_prefix("b/"))
            .unwrap_or(path_token);

        if path.is_empty() || path == "/dev/null" {
            continue;
        }
        if !files.iter().any(|f| f == path) {
            files.push(path.to_string());
        }
    }

    if files.len() == 1 {
        format!("{} ({} lines)", files[0], lines)
    } else if !files.is_empty() {
        format!("{} files ({} lines)", files.len(), lines)
    } else {
        format!("({} lines)", lines)
    }
}

pub(super) fn summarize_apply_patch_input(patch_text: &str) -> String {
    let lines = patch_text.lines().count();
    let mut files: Vec<String> = Vec::new();

    for line in patch_text.lines() {
        let trimmed = line.trim();
        let path = trimmed
            .strip_prefix("*** Add File: ")
            .or_else(|| trimmed.strip_prefix("*** Update File: "))
            .or_else(|| trimmed.strip_prefix("*** Delete File: "))
            .map(str::trim)
            .unwrap_or("");

        if path.is_empty() {
            continue;
        }
        if !files.iter().any(|f| f == path) {
            files.push(path.to_string());
        }
    }

    if files.len() == 1 {
        format!("{} ({} lines)", files[0], lines)
    } else if !files.is_empty() {
        format!("{} files ({} lines)", files.len(), lines)
    } else {
        format!("({} lines)", lines)
    }
}

fn parse_agentgrep_smart_subject_relation(
    input: &serde_json::Value,
) -> (Option<&str>, Option<&str>) {
    let mut subject = None;
    let mut relation = None;

    if let Some(terms) = input.get("terms").and_then(|v| v.as_array()) {
        for term in terms {
            if let Some(term) = term.as_str() {
                if let Some(value) = term.strip_prefix("subject:") {
                    subject = Some(value);
                } else if let Some(value) = term.strip_prefix("relation:") {
                    relation = Some(value);
                }
            }
        }
    }

    if (subject.is_none() || relation.is_none())
        && let Some(query) = input.get("query").and_then(|v| v.as_str())
    {
        for term in query.split_whitespace() {
            if subject.is_none()
                && let Some(value) = term.strip_prefix("subject:")
            {
                subject = Some(value);
            } else if relation.is_none()
                && let Some(value) = term.strip_prefix("relation:")
            {
                relation = Some(value);
            }
        }
    }

    (subject, relation)
}

pub(crate) fn extract_apply_patch_primary_file(patch_text: &str) -> Option<String> {
    for line in patch_text.lines() {
        let trimmed = line.trim();
        let path = trimmed
            .strip_prefix("*** Add File: ")
            .or_else(|| trimmed.strip_prefix("*** Update File: "))
            .or_else(|| trimmed.strip_prefix("*** Delete File: "))
            .map(str::trim)
            .unwrap_or("");

        if !path.is_empty() {
            return Some(path.to_string());
        }
    }

    None
}

pub(crate) fn extract_unified_patch_primary_file(patch_text: &str) -> Option<String> {
    for line in patch_text.lines() {
        let Some(rest) = line
            .strip_prefix("+++ ")
            .or_else(|| line.strip_prefix("--- "))
        else {
            continue;
        };

        let without_tab_suffix = rest.split('\t').next().unwrap_or(rest);
        let path_token = without_tab_suffix.split_whitespace().next().unwrap_or("");
        let path = path_token
            .strip_prefix("a/")
            .or(path_token.strip_prefix("b/"))
            .unwrap_or(path_token);

        if !path.is_empty() && path != "/dev/null" {
            return Some(path.to_string());
        }
    }

    None
}

fn display_prefix_by_width(s: &str, max_width: usize) -> &str {
    if max_width == 0 {
        return "";
    }
    let mut used = 0usize;
    let mut end = 0usize;
    for (idx, ch) in s.char_indices() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + cw > max_width {
            break;
        }
        used += cw;
        end = idx + ch.len_utf8();
    }
    &s[..end]
}

fn display_suffix_by_width(s: &str, max_width: usize) -> &str {
    if max_width == 0 {
        return "";
    }
    let mut used = 0usize;
    let mut start = s.len();
    for (idx, ch) in s.char_indices().rev() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + cw > max_width {
            break;
        }
        used += cw;
        start = idx;
    }
    &s[start..]
}

fn truncate_end_display(s: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(s) <= max_width {
        return s.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    if max_width == 1 {
        return "…".to_string();
    }
    format!(
        "{}…",
        display_prefix_by_width(s, max_width.saturating_sub(1))
    )
}

fn truncate_middle_display(s: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(s) <= max_width {
        return s.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    if max_width == 1 {
        return "…".to_string();
    }
    let remaining = max_width.saturating_sub(1);
    let head = remaining / 2 + remaining % 2;
    let tail = remaining / 2;
    format!(
        "{}…{}",
        display_prefix_by_width(s, head),
        display_suffix_by_width(s, tail)
    )
}

fn truncate_swarm_text(value: &str, max_width: usize) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    truncate_query_display(trimmed, max_width)
}

fn summarize_swarm_tool_action(tool: &ToolCall, bounded: &dyn Fn(usize) -> usize) -> String {
    let action = tool
        .input
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("action missing");
    let target = tool
        .input
        .get("to_session")
        .or_else(|| tool.input.get("target_session"))
        .or_else(|| tool.input.get("channel"))
        .and_then(|v| v.as_str())
        .map(|value| truncate_identifier_display(value, bounded(24)));
    let prompt = tool
        .input
        .get("prompt")
        .or_else(|| tool.input.get("message"))
        .and_then(|v| v.as_str())
        .map(|value| truncate_swarm_text(value, bounded(34)));

    let summary = match action {
        "spawn" => {
            if let Some(prompt) = prompt.as_deref().filter(|value| !value.is_empty()) {
                format!("spawn '{}'", prompt)
            } else if let Some(dir) = tool.input.get("working_dir").and_then(|v| v.as_str()) {
                format!("spawn in {}", truncate_path_display(dir, bounded(28)))
            } else {
                "spawn".to_string()
            }
        }
        "dm" | "message" => {
            let base = target
                .as_deref()
                .map(|target| format!("{} → {}", action, target))
                .unwrap_or_else(|| action.to_string());
            if let Some(prompt) = prompt.as_deref().filter(|value| !value.is_empty()) {
                format!("{} '{}'", base, prompt)
            } else {
                base
            }
        }
        "channel" | "broadcast" => {
            let base = target
                .as_deref()
                .map(|target| format!("{} {}", action, target))
                .unwrap_or_else(|| action.to_string());
            if let Some(prompt) = prompt.as_deref().filter(|value| !value.is_empty()) {
                format!("{} '{}'", base, prompt)
            } else {
                base
            }
        }
        "summary" | "read_context" | "stop" | "approve_plan" | "reject_plan" | "assign_task"
        | "assign_next" | "fill_slots" | "assign_role" | "await_members" | "start" | "wake"
        | "resume" | "retry" | "reassign" | "replace" | "salvage" => target
            .as_deref()
            .map(|target| format!("{} {}", action, target))
            .unwrap_or_else(|| action.to_string()),
        _ => target
            .as_deref()
            .map(|target| format!("{} {}", action, target))
            .unwrap_or_else(|| action.to_string()),
    };

    truncate_end_display(summary.as_str(), bounded(42))
}

fn truncate_path_display(path: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(path) <= max_width {
        return path.to_string();
    }
    if max_width == 0 {
        return String::new();
    }

    let normalized = path.replace('\\', "/");
    let parts: Vec<&str> = normalized
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    if parts.is_empty() {
        return truncate_middle_display(path, max_width);
    }

    let marker = if normalized.starts_with("~/") {
        "~/…/"
    } else if normalized.starts_with("./") {
        "./…/"
    } else if normalized.starts_with('/') {
        "/…/"
    } else {
        "…/"
    };

    let mut kept: Vec<&str> = Vec::new();
    let mut joined = String::new();
    for part in parts.iter().rev() {
        let candidate = if joined.is_empty() {
            (*part).to_string()
        } else {
            format!("{}/{}", part, joined)
        };
        if UnicodeWidthStr::width(marker) + UnicodeWidthStr::width(candidate.as_str()) <= max_width
        {
            joined = candidate;
            kept.push(part);
        } else {
            break;
        }
    }

    if !joined.is_empty() {
        return format!("{}{}", marker, joined);
    }

    let last = parts.last().copied().unwrap_or(path);
    let suffix_budget = max_width.saturating_sub(UnicodeWidthStr::width("…/"));
    if suffix_budget > 0 {
        format!("…/{}", truncate_middle_display(last, suffix_budget))
    } else {
        truncate_middle_display(path, max_width)
    }
}

fn browser_target_summary(
    tool: &ToolCall,
    max_width: Option<usize>,
    include_text_target: bool,
) -> Option<String> {
    let bounded = |preferred: usize| max_width.unwrap_or(preferred);

    if let Some(selector) = tool.input.get("selector").and_then(|v| v.as_str()) {
        return Some(truncate_middle_display(selector, bounded(36)));
    }
    if let Some(text) = tool.input.get("contains").and_then(|v| v.as_str()) {
        return Some(format!(
            "contains '{}'",
            truncate_query_display(text, bounded(24).saturating_sub(11))
        ));
    }
    if include_text_target && let Some(text) = tool.input.get("text").and_then(|v| v.as_str()) {
        return Some(format!(
            "'{}'",
            truncate_query_display(text, bounded(26).saturating_sub(2))
        ));
    }
    match (
        tool.input.get("x").and_then(|v| v.as_f64()),
        tool.input.get("y").and_then(|v| v.as_f64()),
    ) {
        (Some(x), Some(y)) => Some(format!("@{:.0},{:.0}", x, y)),
        _ => None,
    }
}

fn browser_summary(tool: &ToolCall, max_width: Option<usize>) -> String {
    let bounded = |preferred: usize| max_width.unwrap_or(preferred);
    let action = tool
        .input
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("browser");

    let summary = match action {
        "open" | "new_tab" => {
            let label = action.replace('_', " ");
            let url = tool.input.get("url").and_then(|v| v.as_str()).unwrap_or("");
            if url.is_empty() {
                label
            } else {
                format!("{} {}", label, truncate_url_display(url, bounded(44)))
            }
        }
        "snapshot" | "interactables" | "screenshot" => {
            if let Some(target) = browser_target_summary(tool, max_width, true) {
                format!("{} {}", action, target)
            } else {
                action.to_string()
            }
        }
        "get_content" => {
            let format_name = tool
                .input
                .get("format")
                .and_then(|v| v.as_str())
                .unwrap_or("text");
            if let Some(target) = browser_target_summary(tool, max_width, true) {
                format!("content {} {}", format_name, target)
            } else {
                format!("content {}", format_name)
            }
        }
        "click" | "wait" | "hover" | "select" => {
            if let Some(target) = browser_target_summary(tool, max_width, action != "select") {
                format!("{} {}", action, target)
            } else {
                action.to_string()
            }
        }
        "type" => {
            let chars = tool
                .input
                .get("text")
                .and_then(|v| v.as_str())
                .map(|text| text.chars().count());
            match (browser_target_summary(tool, max_width, false), chars) {
                (Some(target), Some(chars)) => format!("type {} ({} chars)", target, chars),
                (Some(target), None) => format!("type {}", target),
                (None, Some(chars)) => format!("type ({} chars)", chars),
                (None, None) => "type".to_string(),
            }
        }
        "fill_form" => {
            let count = tool
                .input
                .get("fields")
                .and_then(|v| v.as_array())
                .map(|fields| fields.len())
                .unwrap_or(0);
            format!("fill {} field{}", count, if count == 1 { "" } else { "s" })
        }
        "upload" => {
            let path = tool
                .input
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let target = browser_target_summary(tool, max_width, false);
            let file = if path.is_empty() {
                None
            } else {
                Some(truncate_path_display(path, bounded(28)))
            };
            match (target, file) {
                (Some(target), Some(file)) => format!("upload {} ← {}", target, file),
                (Some(target), None) => format!("upload {}", target),
                (None, Some(file)) => format!("upload {}", file),
                (None, None) => "upload".to_string(),
            }
        }
        "eval" => {
            let script = tool
                .input
                .get("script")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if script.is_empty() {
                "eval".to_string()
            } else {
                format!("eval {}", truncate_middle_display(script, bounded(42)))
            }
        }
        "scroll" => {
            if let Some(position) = tool.input.get("position").and_then(|v| v.as_str()) {
                format!("scroll {}", position)
            } else if let Some(scroll_to) = tool.input.get("scroll_to") {
                let x = scroll_to.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let y = scroll_to.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
                format!("scroll to {:.0},{:.0}", x, y)
            } else {
                let x = tool.input.get("x").and_then(|v| v.as_f64());
                let y = tool.input.get("y").and_then(|v| v.as_f64());
                match (x, y) {
                    (Some(x), Some(y)) => format!("scroll {:.0},{:.0}", x, y),
                    _ => "scroll".to_string(),
                }
            }
        }
        "press" => {
            let key = tool
                .input
                .get("key")
                .and_then(|v| v.as_str())
                .unwrap_or("key missing");
            if let Some(target) = browser_target_summary(tool, max_width, false) {
                format!("press {} on {}", key, target)
            } else {
                format!("press {}", key)
            }
        }
        "provider_command" => tool
            .input
            .get("provider_action")
            .and_then(|v| v.as_str())
            .map(|value| format!("provider {}", truncate_middle_display(value, bounded(36))))
            .unwrap_or_else(|| "provider".to_string()),
        "status" | "setup" | "list_tabs" | "get_active_tab" | "list_frames" => {
            action.replace('_', " ")
        }
        "select_tab" => tool
            .input
            .get("tab_id")
            .and_then(|v| v.as_i64())
            .map(|tab_id| format!("select tab {}", tab_id))
            .unwrap_or_else(|| "select tab".to_string()),
        _ => action.replace('_', " "),
    };

    max_width
        .map(|width| truncate_end_display(summary.as_str(), width))
        .unwrap_or(summary)
}

fn truncate_path_with_suffix(path: &str, suffix: &str, max_width: usize) -> String {
    let full = format!("{}{}", path, suffix);
    if UnicodeWidthStr::width(full.as_str()) <= max_width {
        return full;
    }
    let suffix_width = UnicodeWidthStr::width(suffix);
    if suffix_width >= max_width {
        return truncate_middle_display(full.as_str(), max_width);
    }
    format!(
        "{}{}",
        truncate_path_display(path, max_width.saturating_sub(suffix_width)),
        suffix
    )
}

fn is_search_token_char(ch: char) -> bool {
    ch.is_alphanumeric() || matches!(ch, '_' | '-')
}

fn best_search_token_range(s: &str) -> Option<(usize, usize)> {
    let mut best: Option<(usize, usize, usize)> = None;
    let mut current_start: Option<usize> = None;

    for (idx, ch) in s.char_indices() {
        if is_search_token_char(ch) {
            current_start.get_or_insert(idx);
        } else if let Some(start) = current_start.take() {
            let end = idx;
            let width = UnicodeWidthStr::width(&s[start..end]);
            if width >= 4 {
                match best {
                    Some((_, _, best_width)) if best_width >= width => {}
                    _ => best = Some((start, end, width)),
                }
            }
        }
    }

    if let Some(start) = current_start {
        let end = s.len();
        let width = UnicodeWidthStr::width(&s[start..end]);
        if width >= 4 {
            match best {
                Some((_, _, best_width)) if best_width >= width => {}
                _ => best = Some((start, end, width)),
            }
        }
    }

    best.map(|(start, end, _)| (start, end))
}

fn truncate_focus_token_display(s: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(s) <= max_width {
        return s.to_string();
    }

    let Some((start, end)) = best_search_token_range(s) else {
        return truncate_middle_display(s, max_width);
    };

    let token = &s[start..end];
    let token_width = UnicodeWidthStr::width(token);
    if token_width >= max_width {
        return truncate_middle_display(token, max_width);
    }

    let left_full = &s[..start];
    let right_full = &s[end..];
    let remaining = max_width.saturating_sub(token_width);
    let mut left_budget = remaining / 2;
    let mut right_budget = remaining.saturating_sub(left_budget);

    let mut left = display_suffix_by_width(left_full, left_budget);
    let mut right = display_prefix_by_width(right_full, right_budget);
    let mut left_marker = if left.len() < left_full.len() {
        "…"
    } else {
        ""
    };
    let mut right_marker = if right.len() < right_full.len() {
        "…"
    } else {
        ""
    };

    let mut current_width = UnicodeWidthStr::width(left_marker)
        + UnicodeWidthStr::width(left)
        + token_width
        + UnicodeWidthStr::width(right)
        + UnicodeWidthStr::width(right_marker);

    while current_width > max_width {
        if !right.is_empty() {
            right_budget = right_budget.saturating_sub(1);
            right = display_prefix_by_width(right_full, right_budget);
        } else if !left.is_empty() {
            left_budget = left_budget.saturating_sub(1);
            left = display_suffix_by_width(left_full, left_budget);
        } else if !right_marker.is_empty() {
            right_marker = "";
        } else if !left_marker.is_empty() {
            left_marker = "";
        } else {
            break;
        }
        current_width = UnicodeWidthStr::width(left_marker)
            + UnicodeWidthStr::width(left)
            + token_width
            + UnicodeWidthStr::width(right)
            + UnicodeWidthStr::width(right_marker);
    }

    format!("{}{}{}{}{}", left_marker, left, token, right, right_marker)
}

fn truncate_regex_display(pattern: &str, max_width: usize) -> String {
    truncate_focus_token_display(pattern, max_width)
}

fn truncate_query_display(query: &str, max_width: usize) -> String {
    truncate_focus_token_display(query, max_width)
}

fn truncate_command_display(command: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(command) <= max_width {
        return command.to_string();
    }
    if max_width <= 1 {
        return "…".to_string();
    }

    let tokens: Vec<&str> = command.split_whitespace().collect();
    if tokens.len() >= 3 {
        let candidates = [
            format!(
                "{} {} … {} {}",
                tokens[0],
                tokens[1],
                tokens[tokens.len() - 2],
                tokens[tokens.len() - 1]
            ),
            format!("{} {} … {}", tokens[0], tokens[1], tokens[tokens.len() - 1]),
            format!("{} … {}", tokens[0], tokens[tokens.len() - 1]),
        ];
        for candidate in candidates {
            if UnicodeWidthStr::width(candidate.as_str()) <= max_width {
                return candidate;
            }
        }
    }

    truncate_middle_display(command, max_width)
}

fn truncate_url_display(url: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(url) <= max_width {
        return url.to_string();
    }

    if let Some((scheme, rest)) = url.split_once("://") {
        let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
        if path.is_empty() {
            return truncate_middle_display(url, max_width);
        }
        let tail = path.rsplit('/').find(|seg| !seg.is_empty()).unwrap_or(path);
        let candidate = format!("{}://{}/…/{}", scheme, host, tail);
        if UnicodeWidthStr::width(candidate.as_str()) <= max_width {
            return candidate;
        }
    }

    truncate_middle_display(url, max_width)
}

fn truncate_identifier_display(value: &str, max_width: usize) -> String {
    truncate_middle_display(value, max_width)
}

pub(super) fn batch_subcall_index(id: &str) -> Option<usize> {
    id.strip_prefix("batch-")?
        .split('-')
        .next()?
        .parse::<usize>()
        .ok()
}

pub(super) fn is_memory_store_tool(tc: &ToolCall) -> bool {
    match tc.name.as_str() {
        "memory" => tc
            .input
            .get("action")
            .and_then(|v| v.as_str())
            .is_some_and(|a| a == "remember"),
        _ => false,
    }
}

pub(super) fn is_memory_recall_tool(tc: &ToolCall) -> bool {
    match tc.name.as_str() {
        "memory" => tc
            .input
            .get("action")
            .and_then(|v| v.as_str())
            .is_some_and(|a| a == "recall"),
        _ => false,
    }
}

/// Extract a brief summary from a tool call input (file path, command, etc.)
pub(crate) fn get_tool_summary(tool: &ToolCall) -> String {
    get_tool_summary_with_budget(tool, 50, None)
}

pub(super) fn get_tool_summary_with_budget(
    tool: &ToolCall,
    bash_max_chars: usize,
    max_width: Option<usize>,
) -> String {
    let bounded = |preferred: usize| max_width.unwrap_or(preferred);

    match canonical_tool_name(&tool.name) {
        "bash" => tool
            .input
            .get("command")
            .and_then(|v| v.as_str())
            .map(|cmd| {
                let has_intent = tool
                    .intent
                    .as_deref()
                    .map(str::trim)
                    .is_some_and(|intent| !intent.is_empty());
                let cmd_budget = max_width
                    .map(|w| w.saturating_sub(2))
                    .unwrap_or(bash_max_chars)
                    .min(if has_intent { 28 } else { usize::MAX });
                format!("$ {}", truncate_command_display(cmd, cmd_budget))
            })
            .unwrap_or_default(),
        "read" => {
            let path = tool
                .input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let start_line = tool.input.get("start_line").and_then(|v| v.as_u64());
            let end_line = tool.input.get("end_line").and_then(|v| v.as_u64());
            let offset = tool.input.get("offset").and_then(|v| v.as_u64());
            let limit = tool.input.get("limit").and_then(|v| v.as_u64());
            match (start_line, end_line, offset, limit) {
                (Some(start), Some(end), _, _) => {
                    let suffix = format!(":{}-{}", start, end);
                    max_width
                        .map(|w| truncate_path_with_suffix(path, suffix.as_str(), w))
                        .unwrap_or_else(|| format!("{}{}", path, suffix))
                }
                (Some(start), None, _, _) => {
                    let suffix = format!(":{}-", start);
                    max_width
                        .map(|w| truncate_path_with_suffix(path, suffix.as_str(), w))
                        .unwrap_or_else(|| format!("{}{}", path, suffix))
                }
                (None, Some(end), _, _) => {
                    let suffix = format!(":1-{}", end);
                    max_width
                        .map(|w| truncate_path_with_suffix(path, suffix.as_str(), w))
                        .unwrap_or_else(|| format!("{}{}", path, suffix))
                }
                (None, None, Some(o), Some(l)) => {
                    let suffix = format!(":{}-{}", o, o + l);
                    max_width
                        .map(|w| truncate_path_with_suffix(path, suffix.as_str(), w))
                        .unwrap_or_else(|| format!("{}{}", path, suffix))
                }
                (None, None, Some(o), None) => {
                    let suffix = format!(":{}", o);
                    max_width
                        .map(|w| truncate_path_with_suffix(path, suffix.as_str(), w))
                        .unwrap_or_else(|| format!("{}{}", path, suffix))
                }
                _ => max_width
                    .map(|w| truncate_path_display(path, w))
                    .unwrap_or_else(|| path.to_string()),
            }
        }
        "write" | "edit" => tool
            .input
            .get("file_path")
            .and_then(|v| v.as_str())
            .map(|p| {
                max_width
                    .map(|w| truncate_path_display(p, w))
                    .unwrap_or_else(|| p.to_string())
            })
            .unwrap_or_default(),
        "multiedit" => {
            let path = tool
                .input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let count = tool
                .input
                .get("edits")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            let suffix = format!(" ({} edits)", count);
            max_width
                .map(|w| truncate_path_with_suffix(path, suffix.as_str(), w))
                .unwrap_or_else(|| format!("{}{}", path, suffix))
        }
        "glob" => tool
            .input
            .get("pattern")
            .and_then(|v| v.as_str())
            .map(|p| {
                let budget = bounded(40).saturating_sub(2);
                format!("'{}'", truncate_middle_display(p, budget))
            })
            .unwrap_or_default(),
        "grep" => {
            let pattern = tool
                .input
                .get("pattern")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let path = tool.input.get("path").and_then(|v| v.as_str());
            if let Some(p) = path {
                if let Some(width) = max_width {
                    let infix = "'";
                    let middle = "' in ";
                    let min_path = 8usize.min(width.saturating_sub(6));
                    let mut path_budget = (width / 3).max(min_path);
                    if path_budget >= width {
                        path_budget = width / 2;
                    }
                    let pattern_budget = width
                        .saturating_sub(path_budget)
                        .saturating_sub(UnicodeWidthStr::width(middle));
                    let path_summary = truncate_path_display(p, path_budget.max(4));
                    let pattern_summary = truncate_regex_display(pattern, pattern_budget.max(4));
                    let combined =
                        format!("{}{}{}{}", infix, pattern_summary, middle, path_summary);
                    if UnicodeWidthStr::width(combined.as_str()) <= width {
                        combined
                    } else {
                        truncate_middle_display(combined.as_str(), width)
                    }
                } else {
                    format!("'{}' in {}", truncate_regex_display(pattern, 30), p)
                }
            } else {
                let budget = bounded(40).saturating_sub(2);
                format!("'{}'", truncate_regex_display(pattern, budget))
            }
        }
        "agentgrep" => {
            let Some(mode) = tool.input.get("mode").and_then(|v| v.as_str()) else {
                return "grep".to_string();
            };
            match mode {
                "grep" | "find" => {
                    let query = tool
                        .input
                        .get("query")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if query.is_empty() {
                        mode.to_string()
                    } else {
                        format!(
                            "{} '{}'",
                            mode,
                            truncate_query_display(
                                query,
                                bounded(36).saturating_sub(mode.len() + 3)
                            )
                        )
                    }
                }
                "smart" => {
                    let (subject, relation) = parse_agentgrep_smart_subject_relation(&tool.input);

                    match (subject, relation) {
                        (Some(subject), Some(relation)) => format!(
                            "smart {}:{}",
                            truncate_identifier_display(subject, bounded(18)),
                            truncate_identifier_display(relation, bounded(14))
                        ),
                        _ => "smart".to_string(),
                    }
                }
                other => other.to_string(),
            }
        }
        "ls" => tool
            .input
            .get("path")
            .and_then(|v| v.as_str())
            .map(|path| {
                max_width
                    .map(|w| truncate_path_display(path, w))
                    .unwrap_or_else(|| path.to_string())
            })
            .unwrap_or_else(|| ".".to_string()),
        "task" => {
            let desc = tool
                .input
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("task");
            let agent_type = tool
                .input
                .get("subagent_type")
                .and_then(|v| v.as_str())
                .unwrap_or("agent");
            let summary = format!("{} ({})", desc, agent_type);
            max_width
                .map(|w| truncate_end_display(summary.as_str(), w))
                .unwrap_or(summary)
        }
        "patch" => tool
            .input
            .get("patch_text")
            .and_then(|v| v.as_str())
            .map(summarize_unified_patch_input)
            .unwrap_or_default(),
        "apply_patch" => tool
            .input
            .get("patch_text")
            .and_then(|v| v.as_str())
            .map(summarize_apply_patch_input)
            .unwrap_or_default(),
        "webfetch" => tool
            .input
            .get("url")
            .and_then(|v| v.as_str())
            .map(|u| truncate_url_display(u, bounded(50)))
            .unwrap_or_default(),
        "websearch" => tool
            .input
            .get("query")
            .and_then(|v| v.as_str())
            .map(|q| {
                format!(
                    "'{}'",
                    truncate_query_display(q, bounded(40).saturating_sub(2))
                )
            })
            .unwrap_or_default(),
        "browser" => browser_summary(tool, max_width),
        "open" | "launch" => {
            let action = tool
                .input
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("open");
            let target = tool
                .input
                .get("target")
                .and_then(|v| v.as_str())
                .map(|t| {
                    let budget = bounded(40);
                    if t.contains("://") {
                        truncate_url_display(t, budget)
                    } else {
                        truncate_path_display(t, budget)
                    }
                })
                .unwrap_or_default();
            format!("{} {}", action, target).trim().to_string()
        }
        "mcp" => {
            let action = tool
                .input
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let server = tool.input.get("server_name").and_then(|v| v.as_str());
            if let Some(s) = server {
                format!("{} {}", action, s)
            } else {
                action.to_string()
            }
        }
        "todo" => {
            if let Some(count) = tool
                .input
                .get("todos")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
            {
                format!("{} items", count)
            } else {
                "todos".to_string()
            }
        }
        "skill" => tool
            .input
            .get("skill")
            .and_then(|v| v.as_str())
            .map(|s| format!("/{}", s))
            .unwrap_or_default(),
        "codesearch" => tool
            .input
            .get("query")
            .and_then(|v| v.as_str())
            .map(|q| {
                format!(
                    "'{}'",
                    truncate_query_display(q, bounded(40).saturating_sub(2))
                )
            })
            .unwrap_or_default(),
        "memory" => {
            let action = tool
                .input
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("action missing");
            match action {
                "remember" => {
                    let content = tool
                        .input
                        .get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    format!("remember: {}", truncate_end_display(content, bounded(35)))
                }
                "recall" => {
                    let query = tool.input.get("query").and_then(|v| v.as_str());
                    if let Some(q) = query {
                        format!(
                            "recall '{}'",
                            truncate_query_display(q, bounded(35).saturating_sub(2))
                        )
                    } else {
                        "recall (recent)".to_string()
                    }
                }
                "search" => {
                    let query = tool
                        .input
                        .get("query")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    format!(
                        "search '{}'",
                        truncate_query_display(query, bounded(35).saturating_sub(2))
                    )
                }
                "forget" => {
                    let id = tool
                        .input
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("id missing");
                    format!("forget {}", truncate_identifier_display(id, bounded(30)))
                }
                "tag" => {
                    let id = tool
                        .input
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("id missing");
                    format!("tag {}", truncate_identifier_display(id, bounded(30)))
                }
                "link" => "link".to_string(),
                "related" => {
                    let id = tool
                        .input
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("id missing");
                    format!("related {}", truncate_identifier_display(id, bounded(30)))
                }
                _ => action.to_string(),
            }
        }
        "goal" => {
            let action = tool
                .input
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("action missing");
            let id = tool.input.get("id").and_then(|v| v.as_str());
            let title = tool.input.get("title").and_then(|v| v.as_str());
            match (action, id, title) {
                ("create", _, Some(title)) => format!(
                    "create '{}'",
                    truncate_end_display(title, bounded(30).saturating_sub(2))
                ),
                ("show" | "focus" | "update" | "checkpoint", Some(id), _) => {
                    format!(
                        "{} {}",
                        action,
                        truncate_identifier_display(id, bounded(30))
                    )
                }
                ("resume", _, _) => "resume".to_string(),
                _ => action.to_string(),
            }
        }
        "selfdev" => {
            let action = tool
                .input
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("action missing");
            action.to_string()
        }
        "side_panel" => {
            let action = tool
                .input
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("action missing");
            let target = tool
                .input
                .get("title")
                .or_else(|| tool.input.get("page_id"))
                .or_else(|| tool.input.get("file_path"))
                .and_then(|v| v.as_str());
            if let Some(target) = target {
                let target = max_width
                    .map(|w| truncate_middle_display(target, w.saturating_sub(action.len() + 1)))
                    .unwrap_or_else(|| target.to_string());
                format!("{} {}", action, target).trim().to_string()
            } else {
                action.to_string()
            }
        }
        "swarm" => summarize_swarm_tool_action(tool, &bounded),
        "session_search" => tool
            .input
            .get("query")
            .and_then(|v| v.as_str())
            .map(|q| {
                format!(
                    "'{}'",
                    truncate_query_display(q, bounded(40).saturating_sub(2))
                )
            })
            .unwrap_or_default(),
        "conversation_search" => {
            if let Some(q) = tool.input.get("query").and_then(|v| v.as_str()) {
                format!(
                    "'{}'",
                    truncate_query_display(q, bounded(40).saturating_sub(2))
                )
            } else if tool
                .input
                .get("stats")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                "stats".to_string()
            } else {
                "history".to_string()
            }
        }
        "lsp" => {
            let op = tool
                .input
                .get("operation")
                .and_then(|v| v.as_str())
                .unwrap_or("command missing");
            let file = tool
                .input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let short_file = file.rsplit('/').next().unwrap_or(file);
            let line = tool.input.get("line").and_then(|v| v.as_u64()).unwrap_or(0);
            format!("{} {}:{}", op, short_file, line)
        }
        "bg" => {
            let action = tool
                .input
                .get("action")
                .and_then(|v| v.as_str())
                .or_else(|| {
                    infer_bg_action_from_intent_for_display(
                        tool.intent
                            .as_deref()
                            .or_else(|| tool.input.get("intent").and_then(|value| value.as_str())),
                    )
                })
                .unwrap_or("action missing");
            let task_id = tool.input.get("task_id").and_then(|v| v.as_str());
            if let Some(id) = task_id {
                format!(
                    "{} {}",
                    action,
                    truncate_identifier_display(id, bounded(20))
                )
            } else {
                action.to_string()
            }
        }
        "batch" => {
            let count = tool
                .input
                .get("tool_calls")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            format!("{} calls", count)
        }
        "subagent" => {
            let desc = tool
                .input
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("task");
            let agent_type = tool
                .input
                .get("subagent_type")
                .and_then(|v| v.as_str())
                .unwrap_or("agent");
            format!("{} ({})", desc, agent_type)
        }
        "debug_socket" => {
            let cmd = tool
                .input
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("command missing");
            truncate_middle_display(cmd, bounded(40))
        }
        name if name.starts_with("mcp__") => tool
            .input
            .as_object()
            .and_then(|obj| obj.iter().find(|(_, v)| v.is_string()))
            .and_then(|(_, v)| v.as_str())
            .map(|s| truncate_middle_display(s, bounded(40)))
            .unwrap_or_default(),
        _ => String::new(),
    }
}

pub(super) fn render_batch_subcall_line(
    tool: &ToolCall,
    icon: &str,
    icon_color: Color,
    bash_max_chars: usize,
    max_width: Option<usize>,
    output_content: Option<&str>,
) -> Line<'static> {
    let display_name = resolve_display_tool_name(&tool.name).to_string();
    let token_badge = output_content.map(|content| {
        let tokens = crate::util::estimate_tokens(content);
        let color = match crate::util::approx_tool_output_token_severity(tokens) {
            crate::util::ApproxTokenSeverity::Normal => rgb(118, 118, 118),
            crate::util::ApproxTokenSeverity::Warning => rgb(214, 184, 92),
            crate::util::ApproxTokenSeverity::Danger => rgb(224, 118, 118),
        };
        (crate::util::format_approx_token_count(tokens), color)
    });
    let intent = tool
        .intent
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let intent_display = intent.map(|intent| {
        max_width
            .map(|width| truncate_end_display(intent, (width / 3).max(16)))
            .unwrap_or_else(|| intent.to_string())
    });
    let intent_width = intent_display.as_ref().map_or(0, |intent| {
        UnicodeWidthStr::width(" · ") + UnicodeWidthStr::width(intent.as_str())
    });
    let reserved = UnicodeWidthStr::width(format!("    {} {}", icon, display_name).as_str())
        + 1
        + intent_width
        + token_badge.as_ref().map_or(0, |(label, _)| {
            UnicodeWidthStr::width(format!(" · {label}").as_str())
        });
    let summary_budget = max_width.map(|w| w.saturating_sub(reserved));
    let summary = output_content
        .and_then(concise_tool_error_summary)
        .unwrap_or_else(|| get_tool_summary_with_budget(tool, bash_max_chars, summary_budget));

    let mut spans = vec![
        Span::styled(format!("    {} ", icon), Style::default().fg(icon_color)),
        Span::styled(display_name, Style::default().fg(tool_color())),
    ];
    if let Some(intent) = intent_display {
        spans.push(Span::styled(" · ", Style::default().fg(dim_color())));
        spans.push(Span::styled(
            intent.clone(),
            Style::default().fg(tool_color()),
        ));
        if !summary.is_empty() && summary != intent {
            spans.push(Span::styled(" · ", Style::default().fg(dim_color())));
            spans.push(Span::styled(summary, Style::default().fg(dim_color())));
        }
    } else if !summary.is_empty() {
        spans.push(Span::styled(
            format!(" {}", summary),
            Style::default().fg(dim_color()),
        ));
    }
    let token_suffix = token_badge.map(|(label, color)| {
        Line::from(vec![
            Span::styled(" · ", Style::default().fg(dim_color())),
            Span::styled(label, Style::default().fg(color)),
        ])
    });

    if let (Some(max_width), Some(token_suffix)) = (max_width, token_suffix.as_ref()) {
        return truncate_line_preserving_suffix_to_width(
            &Line::from(spans),
            token_suffix,
            max_width,
        );
    }

    if let Some(token_suffix) = token_suffix {
        spans.extend(token_suffix.spans);
    }

    Line::from(spans)
}

pub(super) fn summarize_batch_running_tools_compact(running: &[ToolCall]) -> Option<String> {
    if running.is_empty() {
        return None;
    }

    let mut running_sorted = running.to_vec();
    running_sorted.sort_by(|a, b| {
        batch_subcall_index(&a.id)
            .unwrap_or(usize::MAX)
            .cmp(&batch_subcall_index(&b.id).unwrap_or(usize::MAX))
            .then_with(|| a.id.cmp(&b.id))
    });

    let first = &running_sorted[0];
    let label = match batch_subcall_index(&first.id) {
        Some(idx) => format!("#{} {}", idx, first.name),
        None => first.name.clone(),
    };

    if running_sorted.len() == 1 {
        Some(label)
    } else {
        Some(format!("{} +{}", label, running_sorted.len() - 1))
    }
}
