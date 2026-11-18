//! Browser tool summaries.

use super::*;

pub(super) fn browser_summary(tool: &ToolCall, max_width: Option<usize>) -> String {
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
            let key = tool.input.get("key").and_then(|v| v.as_str());
            match (key, browser_target_summary(tool, max_width, false)) {
                (Some(key), Some(target)) => format!("press {} on {}", key, target),
                (Some(key), None) => format!("press {}", key),
                (None, Some(target)) => format!("press {}", target),
                (None, None) => "press".to_string(),
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
