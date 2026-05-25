use serde_json::Value;
use similar::TextDiff;
use std::collections::HashMap;
use std::path::PathBuf;

/// Tracks a pending file edit for diff generation.
pub(crate) struct PendingFileDiff {
    pub(crate) file_path: String,
    pub(crate) original_content: String,
}

#[derive(Default)]
pub(crate) struct RemoteDiffTracker {
    pub(crate) pending_diffs: HashMap<String, PendingFileDiff>,
    pub(crate) current_tool_id: Option<String>,
    pub(crate) current_tool_name: Option<String>,
    pub(crate) current_tool_input: String,
}

impl RemoteDiffTracker {
    pub(crate) fn handle_tool_start(&mut self, id: &str, name: &str) {
        self.current_tool_id = Some(id.to_string());
        self.current_tool_name = Some(name.to_string());
        self.current_tool_input.clear();
    }

    pub(crate) fn handle_tool_input(&mut self, delta: &str) {
        self.current_tool_input.push_str(delta);
    }

    pub(crate) fn current_tool_input_json(&self) -> Value {
        serde_json::from_str(&self.current_tool_input).unwrap_or(Value::Null)
    }

    pub(crate) fn handle_tool_exec(&mut self, id: &str, name: &str) {
        if show_diffs_enabled()
            && matches!(
                crate::tui::ui::tools_ui::canonical_tool_name(name),
                "edit" | "write" | "multiedit"
            )
            && let Ok(input) = serde_json::from_str::<Value>(&self.current_tool_input)
            && let Some(file_path) = input.get("file_path").and_then(|v| v.as_str())
        {
            let resolved = resolve_diff_path(file_path);
            let original = std::fs::read_to_string(&resolved).unwrap_or_default();
            self.pending_diffs.insert(
                id.to_string(),
                PendingFileDiff {
                    file_path: resolved.to_string_lossy().to_string(),
                    original_content: original,
                },
            );
        }

        self.current_tool_id = None;
        self.current_tool_name = None;
        self.current_tool_input.clear();
    }

    pub(crate) fn finish_tool(&mut self, id: &str, name: &str, output: &str) -> String {
        if let Some(pending) = self.pending_diffs.remove(id) {
            let new_content = std::fs::read_to_string(&pending.file_path).unwrap_or_default();
            let diff =
                generate_unified_diff(&pending.original_content, &new_content, &pending.file_path);
            if !diff.is_empty() {
                return format!("[{}] {}\n{}", name, pending.file_path, diff);
            }
        }

        format!("[{}] {}", name, output)
    }

    pub(crate) fn clear(&mut self) {
        self.pending_diffs.clear();
        self.current_tool_id = None;
        self.current_tool_name = None;
        self.current_tool_input.clear();
    }
}

/// Check if client-side diff generation is enabled.
pub(crate) fn show_diffs_enabled() -> bool {
    std::env::var("JCODE_SHOW_DIFFS")
        .map(|v| v != "0" && v.to_lowercase() != "false")
        .unwrap_or(true)
}

/// Resolve a file path for client-side diff generation.
/// Expands `~` to home directory and resolves relative paths against cwd.
pub(crate) fn resolve_diff_path(raw: &str) -> PathBuf {
    let expanded = if let Some(stripped) = raw.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            home.join(stripped)
        } else {
            PathBuf::from(raw)
        }
    } else {
        PathBuf::from(raw)
    };

    if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir().unwrap_or_default().join(expanded)
    }
}

/// Generate a unified diff between two strings.
pub(crate) fn generate_unified_diff(old: &str, new: &str, file_path: &str) -> String {
    let diff = TextDiff::from_lines(old, new);
    let mut output = String::new();

    output.push_str(&format!("--- a/{}\n", file_path));
    output.push_str(&format!("+++ b/{}\n", file_path));

    for hunk in diff.unified_diff().context_radius(3).iter_hunks() {
        output.push_str(&format!("{}", hunk));
    }

    output
}
