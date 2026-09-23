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
    tool_inputs: HashMap<String, String>,
}

impl RemoteDiffTracker {
    pub(crate) fn handle_tool_start(&mut self, id: &str, _name: &str) {
        self.current_tool_id = Some(id.to_string());
        self.tool_inputs.insert(id.to_string(), String::new());
    }

    pub(crate) fn handle_tool_input(&mut self, id: Option<&str>, delta: &str) {
        if let Some(id) = id.or(self.current_tool_id.as_deref())
            && let Some(input) = self.tool_inputs.get_mut(id)
        {
            input.push_str(delta);
        }
    }

    pub(crate) fn tool_input_json(&self, id: &str) -> Value {
        self.tool_inputs
            .get(id)
            .and_then(|input| serde_json::from_str(input).ok())
            .unwrap_or(Value::Null)
    }

    pub(crate) fn handle_tool_exec(&mut self, id: &str, name: &str) {
        let input = self.tool_input_json(id);
        if show_diffs_enabled()
            && matches!(
                crate::tui::ui::tools_ui::canonical_tool_name(name),
                "edit" | "write" | "multiedit"
            )
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

        self.tool_inputs.remove(id);
        if self.current_tool_id.as_deref() == Some(id) {
            self.current_tool_id = None;
        }
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
        self.tool_inputs.clear();
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

#[cfg(test)]
mod keyed_tool_tests {
    use super::*;

    #[test]
    fn keyed_tool_inputs_survive_sibling_exec_and_preserve_legacy_fallback() {
        let mut tracker = RemoteDiffTracker::default();
        tracker.handle_tool_start("a", "read");
        tracker.handle_tool_input(Some("a"), r#"{"file_path":"a"#);
        tracker.handle_tool_start("b", "read");
        tracker.handle_tool_input(None, r#"{"file_path":"b"}"#);
        tracker.handle_tool_input(Some("a"), r#""}"#);
        tracker.handle_tool_input(Some("unknown"), "corruption");
        assert_eq!(tracker.tool_input_json("a")["file_path"], "a");
        tracker.handle_tool_exec("a", "read");
        assert_eq!(tracker.tool_input_json("a"), Value::Null);
        assert_eq!(tracker.tool_input_json("b")["file_path"], "b");
        assert_eq!(tracker.current_tool_id.as_deref(), Some("b"));
        tracker.handle_tool_exec("b", "read");
        assert_eq!(tracker.tool_input_json("b"), Value::Null);
        assert!(tracker.current_tool_id.is_none());
    }

    #[test]
    fn keyed_tool_inputs_snapshot_the_matching_file_for_each_diff() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        std::fs::write(&a, "before a\n").unwrap();
        std::fs::write(&b, "before b\n").unwrap();
        let mut tracker = RemoteDiffTracker::default();
        tracker.handle_tool_start("a", "write");
        tracker.handle_tool_start("b", "write");
        tracker.handle_tool_input(Some("a"), &serde_json::json!({"file_path": a}).to_string());
        tracker.handle_tool_input(Some("b"), &serde_json::json!({"file_path": b}).to_string());
        tracker.handle_tool_exec("a", "write");
        tracker.handle_tool_exec("b", "write");
        if show_diffs_enabled() {
            assert_eq!(tracker.pending_diffs["a"].original_content, "before a\n");
            assert_eq!(tracker.pending_diffs["b"].original_content, "before b\n");
            std::fs::write(&a, "after a\n").unwrap();
            std::fs::write(&b, "after b\n").unwrap();
            let a_diff = tracker.finish_tool("a", "write", "done");
            let b_diff = tracker.finish_tool("b", "write", "done");
            assert!(a_diff.contains("-before a\n+after a"));
            assert!(b_diff.contains("-before b\n+after b"));
        }
    }

    #[test]
    fn keyed_tool_inputs_clear_discards_all_partial_calls() {
        let mut tracker = RemoteDiffTracker::default();
        tracker.handle_tool_start("a", "read");
        tracker.handle_tool_input(None, "{}");
        tracker.handle_tool_start("b", "read");
        tracker.handle_tool_input(None, "{}");
        tracker.clear();
        assert!(tracker.tool_inputs.is_empty());
        assert!(tracker.current_tool_id.is_none());
        tracker.handle_tool_input(Some("a"), "{}");
        assert!(tracker.tool_inputs.is_empty());
    }
}
