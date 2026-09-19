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
    pub(crate) parsed_inputs: HashMap<String, Value>,
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
        if let Some(id) = self.current_tool_id.as_deref()
            && let Ok(input) = serde_json::from_str::<Value>(&self.current_tool_input)
        {
            self.parsed_inputs.insert(id.to_string(), input);
        }
    }

    pub(crate) fn current_tool_input_json(&self) -> Value {
        serde_json::from_str(&self.current_tool_input).unwrap_or(Value::Null)
    }

    pub(crate) fn tool_input_for(&self, id: &str) -> Value {
        self.parsed_inputs
            .get(id)
            .cloned()
            .unwrap_or(self.current_tool_input_json())
    }

    pub(crate) fn handle_tool_exec(&mut self, id: &str, name: &str) {
        let parsed = serde_json::from_str::<Value>(&self.current_tool_input).ok();
        if let Some(input) = parsed.clone() {
            self.parsed_inputs.insert(id.to_string(), input);
        }
        if show_diffs_enabled()
            && matches!(
                crate::tui::ui::tools_ui::canonical_tool_name(name),
                "edit" | "write" | "multiedit"
            )
            && let Some(input) = parsed
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
        let disk_diff = self.pending_diffs.remove(id).and_then(|pending| {
            let new_content = std::fs::read_to_string(&pending.file_path).unwrap_or_default();
            let diff =
                generate_unified_diff(&pending.original_content, &new_content, &pending.file_path);
            (!diff.is_empty()).then(|| format!("[{name}] {}\n{diff}", pending.file_path))
        });
        self.parsed_inputs.remove(id);
        if let Some(diff) = disk_diff {
            return diff;
        }
        if looks_like_unified_diff(output) {
            return output.to_string();
        }
        format!("[{name}] {output}")
    }

    pub(crate) fn clear(&mut self) {
        self.pending_diffs.clear();
        self.parsed_inputs.clear();
        self.current_tool_id = None;
        self.current_tool_name = None;
        self.current_tool_input.clear();
    }
}

fn looks_like_unified_diff(output: &str) -> bool {
    output.contains("\n--- a/")
        || output.contains("\n+++ b/")
        || output.trim_start().starts_with("--- a/")
        || output.trim_start().starts_with("+++ b/")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_parsed_input_per_tool_id_after_exec() {
        let mut tracker = RemoteDiffTracker::default();
        tracker.handle_tool_start("call-a", "edit");
        tracker.handle_tool_input(r#"{"file_path":"a.go","old_string":"old","new_string":"new"}"#);
        tracker.handle_tool_exec("call-a", "edit");
        let input = tracker.tool_input_for("call-a");
        assert_eq!(input["file_path"], "a.go");
        assert_eq!(input["old_string"], "old");
        assert_eq!(input["new_string"], "new");
    }

    #[test]
    fn finish_tool_keeps_existing_unified_diff() {
        let mut tracker = RemoteDiffTracker::default();
        let output = "\n--- a/demo.go\n+++ b/demo.go\n@@ -1 +1 @@\n-old\n+new\n";
        let finished = tracker.finish_tool("call-a", "edit", output);
        assert!(finished.contains("--- a/demo.go"), "{finished}");
        assert!(!finished.starts_with("[edit] ---"), "{finished}");
        assert!(finished.contains("-old"), "{finished}");
        assert!(finished.contains("+new"), "{finished}");
    }
}
