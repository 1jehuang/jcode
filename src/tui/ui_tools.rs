use crate::message::ToolCall;

use super::{dim_color, tool_color};
use ratatui::prelude::*;

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

fn truncate_chars(s: &str, max_chars: usize) -> String {
    match s.char_indices().nth(max_chars) {
        Some((byte_idx, _)) => format!("{}...", &s[..byte_idx]),
        None => s.to_string(),
    }
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
    get_tool_summary_with_bash_limit(tool, 50)
}

pub(super) fn get_tool_summary_with_bash_limit(tool: &ToolCall, bash_max_chars: usize) -> String {
    let truncate = |s: &str, max_chars: usize| truncate_chars(s, max_chars);

    match tool.name.as_str() {
        "bash" => tool
            .input
            .get("command")
            .and_then(|v| v.as_str())
            .map(|cmd| format!("$ {}", truncate(cmd, bash_max_chars)))
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
                (Some(o), Some(l)) => format!("{}:{}-{}", path, o, o + l),
                (Some(o), None) => format!("{}:{}", path, o),
                _ => path.to_string(),
            }
        }
        "write" | "edit" => tool
            .input
            .get("file_path")
            .and_then(|v| v.as_str())
            .map(|p| p.to_string())
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
            format!("{} ({} edits)", path, count)
        }
        "glob" => tool
            .input
            .get("pattern")
            .and_then(|v| v.as_str())
            .map(|p| format!("'{}'", p))
            .unwrap_or_default(),
        "grep" => {
            let pattern = tool
                .input
                .get("pattern")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let path = tool.input.get("path").and_then(|v| v.as_str());
            if let Some(p) = path {
                format!("'{}' in {}", truncate(pattern, 30), p)
            } else {
                format!("'{}'", truncate(pattern, 40))
            }
        }
        "ls" => tool
            .input
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or(".")
            .to_string(),
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
            format!("{} ({})", desc, agent_type)
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
            .map(|u| truncate(u, 50))
            .unwrap_or_default(),
        "websearch" => tool
            .input
            .get("query")
            .and_then(|v| v.as_str())
            .map(|q| format!("'{}'", truncate(q, 40)))
            .unwrap_or_default(),
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
            .map(|q| format!("'{}'", truncate(q, 40)))
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
                    format!("remember: {}", truncate(content, 35))
                }
                "recall" => {
                    let query = tool.input.get("query").and_then(|v| v.as_str());
                    if let Some(q) = query {
                        format!("recall '{}'", truncate(q, 35))
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
                    format!("search '{}'", truncate(query, 35))
                }
                "forget" => {
                    let id = tool.input.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                    format!("forget {}", truncate(id, 30))
                }
                "tag" => {
                    let id = tool.input.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                    format!("tag {}", truncate(id, 30))
                }
                "link" => "link".to_string(),
                "related" => {
                    let id = tool.input.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                    format!("related {}", truncate(id, 30))
                }
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
                format!("{} → {}", action, truncate(t, 25))
            } else {
                action.to_string()
            }
        }
        "session_search" => tool
            .input
            .get("query")
            .and_then(|v| v.as_str())
            .map(|q| format!("'{}'", truncate(q, 40)))
            .unwrap_or_default(),
        "conversation_search" => {
            if let Some(q) = tool.input.get("query").and_then(|v| v.as_str()) {
                format!("'{}'", truncate(q, 40))
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
                format!("{} {}", action, truncate(id, 20))
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
            truncate(cmd, 40)
        }
        name if name.starts_with("mcp__") => tool
            .input
            .as_object()
            .and_then(|obj| obj.iter().find(|(_, v)| v.is_string()))
            .and_then(|(_, v)| v.as_str())
            .map(|s| truncate(s, 40))
            .unwrap_or_default(),
        _ => String::new(),
    }
}

pub(super) fn render_batch_subcall_line(
    tool: &ToolCall,
    icon: &str,
    icon_color: Color,
    bash_max_chars: usize,
) -> Line<'static> {
    let display_name = resolve_display_tool_name(&tool.name).to_string();
    let summary = get_tool_summary_with_bash_limit(tool, bash_max_chars);

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
