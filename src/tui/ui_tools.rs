use crate::message::ToolCall;

use super::{dim_color, tool_color};
use ratatui::prelude::*;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Map provider-side tool names to internal display names.
/// Mirrors `Registry::resolve_tool_name` so the TUI shows friendly names.
pub(super) fn resolve_display_tool_name(name: &str) -> &str {
    match name {
        "task" | "task_runner" => "subagent",
        "shell_exec" => "bash",
        "file_read" => "read",
        "file_write" => "write",
        "file_edit" => "edit",
        "file_glob" => "glob",
        "file_grep" => "grep",
        "todo_read" => "todoread",
        "todo_write" => "todowrite",
        other => other,
    }
}

/// Parse batch result content to determine per-sub-call success/error.
/// Returns a Vec<bool> where `true` means that sub-call errored.
/// The batch output format is:
///   --- [1] tool_name ---
///   <output or Error: ...>
///   --- [2] tool_name ---
///   ...
pub(super) fn parse_batch_sub_results(content: &str) -> Vec<bool> {
    let mut results = Vec::new();
    let mut current_errored = false;
    let mut in_section = false;

    for line in content.lines() {
        if line.starts_with("--- [") && line.ends_with(" ---") {
            if in_section {
                results.push(current_errored);
            }
            in_section = true;
            current_errored = false;
        } else if in_section
            && (line.starts_with("Error:")
                || line.starts_with("error:")
                || line.starts_with("Failed:"))
        {
            current_errored = true;
        }
    }
    if in_section {
        results.push(current_errored);
    }
    results
}

/// Normalize a batch sub-call object to the effective parameters payload.
/// Supports both canonical shape ({"tool": "...", "parameters": {...}})
/// and recovered flat shape ({"tool": "...", "file_path": "...", ...}).
pub(super) fn batch_subcall_params(call: &serde_json::Value) -> serde_json::Value {
    if let Some(params) = call.get("parameters") {
        return params.clone();
    }

    if let Some(obj) = call.as_object() {
        let mut flat = serde_json::Map::new();
        for (k, v) in obj {
            if k != "tool" && k != "name" {
                flat.insert(k.clone(), v.clone());
            }
        }
        return serde_json::Value::Object(flat);
    }

    serde_json::Value::Object(serde_json::Map::new())
}

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

pub(super) fn extract_apply_patch_primary_file(patch_text: &str) -> Option<String> {
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

pub(super) fn extract_unified_patch_primary_file(patch_text: &str) -> Option<String> {
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

fn truncate_chars(s: &str, max_chars: usize) -> String {
    truncate_end_display(s, max_chars)
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
pub(super) fn get_tool_summary(tool: &ToolCall) -> String {
    get_tool_summary_with_budget(tool, 50, None)
}

pub(super) fn get_tool_summary_with_bash_limit(tool: &ToolCall, bash_max_chars: usize) -> String {
    get_tool_summary_with_budget(tool, bash_max_chars, None)
}

pub(super) fn get_tool_summary_with_budget(
    tool: &ToolCall,
    bash_max_chars: usize,
    max_width: Option<usize>,
) -> String {
    let bounded = |preferred: usize| max_width.unwrap_or(preferred);

    match tool.name.as_str() {
        "bash" => tool
            .input
            .get("command")
            .and_then(|v| v.as_str())
            .map(|cmd| {
                let cmd_budget = max_width
                    .map(|w| w.saturating_sub(2))
                    .unwrap_or(bash_max_chars);
                format!("$ {}", truncate_command_display(cmd, cmd_budget))
            })
            .unwrap_or_default(),
        "read" => {
            let path = tool
                .input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let offset = tool.input.get("offset").and_then(|v| v.as_u64());
            let limit = tool.input.get("limit").and_then(|v| v.as_u64());
            match (offset, limit) {
                (Some(o), Some(l)) => {
                    let suffix = format!(":{}-{}", o, o + l);
                    max_width
                        .map(|w| truncate_path_with_suffix(path, suffix.as_str(), w))
                        .unwrap_or_else(|| format!("{}{}", path, suffix))
                }
                (Some(o), None) => {
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
        "patch" | "Patch" => tool
            .input
            .get("patch_text")
            .and_then(|v| v.as_str())
            .map(summarize_unified_patch_input)
            .unwrap_or_default(),
        "apply_patch" | "ApplyPatch" => tool
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
        "open" | "launch" => {
            let action = tool
                .input
                .get("mode")
                .and_then(|v| v.as_str())
                .or_else(|| tool.input.get("action").and_then(|v| v.as_str()))
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
        "todoread" => "todos".to_string(),
        "todowrite" => {
            let count = tool
                .input
                .get("todos")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            format!("{} items", count)
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
                .unwrap_or("?");
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
                    let id = tool.input.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                    format!("forget {}", truncate_identifier_display(id, bounded(30)))
                }
                "tag" => {
                    let id = tool.input.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                    format!("tag {}", truncate_identifier_display(id, bounded(30)))
                }
                "link" => "link".to_string(),
                "related" => {
                    let id = tool.input.get("id").and_then(|v| v.as_str()).unwrap_or("?");
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
                .unwrap_or("?");
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
                .unwrap_or("?");
            action.to_string()
        }
        "communicate" => {
            let action = tool
                .input
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let target = tool
                .input
                .get("to_session")
                .or_else(|| tool.input.get("target_session"))
                .or_else(|| tool.input.get("channel"))
                .and_then(|v| v.as_str());
            if let Some(t) = target {
                format!(
                    "{} → {}",
                    action,
                    truncate_identifier_display(t, bounded(25))
                )
            } else {
                action.to_string()
            }
        }
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
                .unwrap_or("?");
            let file = tool
                .input
                .get("filePath")
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
                .unwrap_or("?");
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
                .unwrap_or("?");
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
) -> Line<'static> {
    let display_name = resolve_display_tool_name(&tool.name).to_string();
    let reserved = UnicodeWidthStr::width(format!("    {} {}", icon, display_name).as_str()) + 1;
    let summary_budget = max_width.map(|w| w.saturating_sub(reserved));
    let summary = get_tool_summary_with_budget(tool, bash_max_chars, summary_budget);

    let mut spans = vec![
        Span::styled(format!("    {} ", icon), Style::default().fg(icon_color)),
        Span::styled(display_name, Style::default().fg(tool_color())),
    ];
    if !summary.is_empty() {
        spans.push(Span::styled(
            format!(" {}", summary),
            Style::default().fg(dim_color()),
        ));
    }

    Line::from(spans)
}

pub(super) fn format_batch_running_tool(tool: &ToolCall, bash_max_chars: usize) -> String {
    let prefix = batch_subcall_index(&tool.id)
        .map(|idx| format!("#{} ", idx))
        .unwrap_or_default();
    let detail = get_tool_summary_with_bash_limit(tool, bash_max_chars);

    if detail.is_empty() {
        format!("{}{}", prefix, tool.name)
    } else {
        format!("{}{} ({})", prefix, tool.name, detail)
    }
}

pub(super) fn summarize_batch_running_tools(
    running: &[ToolCall],
    max_visible: usize,
    bash_max_chars: usize,
) -> Option<String> {
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

    let visible = max_visible.max(1);
    let mut labels: Vec<String> = running_sorted
        .iter()
        .take(visible)
        .map(|tool| format_batch_running_tool(tool, bash_max_chars))
        .collect();

    if running_sorted.len() > visible {
        labels.push(format!("+{} more", running_sorted.len() - visible));
    }

    Some(labels.join(", "))
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
