//! Authoritative, full-file diffs for clients. Keep these in the text result as
//! well as metadata: persisted ToolResult and ToolDone currently carry only text.
use super::ToolOutput;
use similar::TextDiff;

/// None means unreadable, not an empty file. Never fabricate its prior text.
pub(super) async fn snapshot(path: &std::path::Path) -> Option<(bool, String)> {
    match tokio::fs::read_to_string(path).await {
        Ok(content) => Some((true, content)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some((false, String::new())),
        Err(_) => None,
    }
}

pub(super) fn unified(old_path: &str, new_path: &str, old: &str, new: &str) -> String {
    // Quote control characters in headers, rather than letting a filename inject
    // extra diff lines. Ordinary paths (including spaces) remain unchanged.
    fn header(path: &str, prefix: &str) -> String {
        let path = if path == "/dev/null" {
            path.to_owned()
        } else {
            format!("{prefix}/{path}")
        };
        let path = path.as_str();
        if path
            .chars()
            .any(|c| c.is_control() || c == '"' || c == '\\')
        {
            let mut quoted = String::from("\"");
            for byte in path.bytes() {
                match byte {
                    b'\n' => quoted.push_str("\\n"),
                    b'\r' => quoted.push_str("\\r"),
                    b'\t' => quoted.push_str("\\t"),
                    b'\\' => quoted.push_str("\\\\"),
                    b'"' => quoted.push_str("\\\""),
                    0..=31 | 127..=255 => quoted.push_str(&format!("\\{byte:03o}")),
                    _ => quoted.push(char::from(byte)),
                }
            }
            quoted.push('"');
            quoted
        } else {
            path.to_owned()
        }
    }
    let old_header = header(old_path, "a");
    let new_header = header(new_path, "b");
    if old == new {
        return format!("--- {old_header}\n+++ {new_header}\n");
    }
    TextDiff::from_lines(old, new)
        .unified_diff()
        .context_radius(3)
        .header(&old_header, &new_header)
        .to_string()
}

pub(super) fn attach(mut output: ToolOutput, diff: String) -> ToolOutput {
    // Header-only entries identify known no-ops. No entries means unknown,
    // rather than claiming all requested files had no net changes.
    if diff.is_empty() {
        return output;
    }
    output.output.push_str("\n\nFile diff:\n```diff\n");
    output.output.push_str(&diff);
    if !diff.is_empty() && !diff.ends_with('\n') {
        output.output.push('\n');
    }
    output.output.push_str("```\n");
    let metadata = output.metadata.get_or_insert_with(|| serde_json::json!({}));
    metadata["diff"] = serde_json::Value::String(diff);
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_file_positions_and_disjoint_hunks() {
        let old: String = (1..=100).map(|n| format!("line {n}\n")).collect();
        let new = old
            .replace("line 40\n", "  replaced\n\n")
            .replace("line 90\n", "end\n");
        let diff = unified("file.rs", "file.rs", &old, &new);
        assert!(diff.contains("@@ -37,7 +37,8 @@"), "{diff}");
        assert!(diff.contains("@@ -87,7 +88,7 @@"), "{diff}");
        assert!(diff.contains("+  replaced\n+\n"));
    }

    #[test]
    fn creation_deletion_eof_and_noop() {
        let created = unified("/dev/null", "new file", "", "last");
        assert!(created.contains("@@ -0,0 +1 @@"), "{created}");
        assert!(created.contains("\\ No newline at end of file"));
        assert!(unified("f", "/dev/null", "old\n", "").contains("@@ -1 +0,0 @@"));
        assert_eq!(unified("f", "f", "same\n", "same\n"), "--- a/f\n+++ b/f\n");
        assert!(
            attach(ToolOutput::new("unknown"), String::new())
                .metadata
                .is_none()
        );
    }

    #[test]
    fn text_and_metadata_share_exact_diff() {
        let diff = unified("tab\tand\nnewline", "f", "old\n", "new\n");
        assert!(diff.starts_with("--- \"a/tab\\tand\\nnewline\"\n"));
        assert!(unified("control\u{1}", "f", "old", "new").starts_with("--- \"a/control\\001\"\n"));
        let output = attach(ToolOutput::new("summary"), diff.clone());
        assert!(output.output.contains(&format!("```diff\n{diff}```")));
        assert_eq!(output.metadata.unwrap()["diff"], diff);
    }
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn authoritative_diffs_cover_successful_tool_mutations() {
        use crate::tool::{Tool, ToolContext, ToolExecutionMode};
        use serde_json::json;
        let _lock = crate::storage::lock_test_env();
        let home = tempfile::tempdir().unwrap();
        struct Home(Option<std::ffi::OsString>);
        impl Drop for Home {
            fn drop(&mut self) {
                if let Some(old) = self.0.take() {
                    crate::env::set_var("JCODE_HOME", old);
                } else {
                    crate::env::remove_var("JCODE_HOME");
                }
            }
        }
        let _home = Home(std::env::var_os("JCODE_HOME"));
        crate::env::set_var("JCODE_HOME", home.path());
        let ctx = ToolContext {
            session_id: "file_diff_test".into(),
            message_id: "m".into(),
            tool_call_id: "t".into(),
            working_dir: Some(home.path().into()),
            stdin_request_tx: None,
            graceful_shutdown_signal: None,
            execution_mode: ToolExecutionMode::Direct,
        };
        let path = home.path().join("file.rs");
        let original: String = (1..=100).map(|n| format!("line {n}\n")).collect();
        std::fs::write(&path, &original).unwrap();
        let edited = super::super::edit::EditTool::new()
            .execute(
                json!({
                    "file_path": "file.rs", "old_string": "ine 40", "new_string": "ong 40"
                }),
                ctx.clone(),
            )
            .await
            .unwrap();
        let after_edit = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            edited.metadata.as_ref().unwrap()["diff"],
            unified("file.rs", "file.rs", &original, &after_edit)
        );
        assert!(edited.output.contains("@@ -37,7 +37,7 @@"));
        assert!(edited.output.contains("40- ine 40"));
        let multi = super::super::multiedit::MultiEditTool::new()
            .execute(
                json!({
                    "file_path": "file.rs", "edits": [
                        {"old_string":"line 10\n", "new_string":"inserted\nextra\n"},
                        {"old_string":"line 90", "new_string":"tail"},
                        {"old_string":"missing", "new_string":"must not appear"}
                    ]
                }),
                ctx.clone(),
            )
            .await
            .unwrap();
        let after_multi = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            multi.metadata.as_ref().unwrap()["diff"],
            unified("file.rs", "file.rs", &after_edit, &after_multi)
        );
        assert!(multi.output.contains("@@ -87,7 +88,7 @@"));
        let repeated = super::super::edit::EditTool::new().execute(json!({
            "file_path":"file.rs", "old_string":"line", "new_string":"row", "replace_all":true
        }), ctx.clone()).await.unwrap();
        let after_repeat = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            repeated.metadata.as_ref().unwrap()["diff"],
            unified("file.rs", "file.rs", &after_multi, &after_repeat)
        );
        let written = super::super::write::WriteTool::new()
            .execute(
                json!({
                    "file_path":"file.rs", "content":"replacement\n"
                }),
                ctx.clone(),
            )
            .await
            .unwrap();
        assert_eq!(
            written.metadata.as_ref().unwrap()["diff"],
            unified("file.rs", "file.rs", &after_repeat, "replacement\n")
        );
        let patch = super::super::apply_patch::ApplyPatchTool::new().execute(json!({
            "patch_text":"*** Begin Patch\n*** Update File: file.rs\n*** Move to: moved.rs\n@@\n-replacement\n+patched\n*** Add File: created.rs\n+created\n*** End Patch"
        }), ctx.clone()).await.unwrap();
        let diff = patch.metadata.as_ref().unwrap()["diff"].as_str().unwrap();
        assert!(diff.contains("--- a/file.rs\n+++ b/moved.rs\n"), "{diff}");
        assert!(diff.contains("--- /dev/null\n+++ b/created.rs\n"), "{diff}");
        let noop = super::super::multiedit::MultiEditTool::new()
            .execute(
                json!({
                    "file_path":"moved.rs", "edits":[
                        {"old_string":"patched", "new_string":"temporary"},
                        {"old_string":"temporary", "new_string":"patched"},
                        {"old_string":"missing", "new_string":"not applied"}
                    ]
                }),
                ctx.clone(),
            )
            .await
            .unwrap();
        assert_eq!(
            noop.metadata.as_ref().unwrap()["diff"],
            "--- a/moved.rs\n+++ b/moved.rs\n"
        );
        assert!(
            noop.output
                .contains("```diff\n--- a/moved.rs\n+++ b/moved.rs\n```")
        );
        let failed = super::super::edit::EditTool::new()
            .execute(
                json!({
                    "file_path":"moved.rs", "old_string":"missing", "new_string":"not applied"
                }),
                ctx.clone(),
            )
            .await;
        assert!(failed.is_err());
        // A binary overwrite must not pretend the old file was empty.
        std::fs::write(&path, [0xff]).unwrap();
        let unknown = super::super::write::WriteTool::new()
            .execute(
                json!({
                    "file_path":"file.rs", "content":"text"
                }),
                ctx.clone(),
            )
            .await
            .unwrap();
        assert!(unknown.metadata.is_none());
        assert!(!unknown.output.contains("```diff"));
        std::fs::write(&path, [0xff]).unwrap();
        let mixed = super::super::apply_patch::ApplyPatchTool::new().execute(json!({
            "patch_text":"*** Begin Patch\n*** Add File: file.rs\n+text\n*** Update File: moved.rs\n@@\n-patched\n+patched\n*** End Patch"
        }), ctx.clone()).await.unwrap();
        assert_eq!(
            mixed.metadata.as_ref().unwrap()["diff"],
            "--- a/moved.rs\n+++ b/moved.rs\n"
        );
        std::fs::write(&path, [0xff]).unwrap();
        let all_unknown = super::super::apply_patch::ApplyPatchTool::new()
            .execute(
                json!({
                    "patch_text":"*** Begin Patch\n*** Add File: file.rs\n+text\n*** End Patch"
                }),
                ctx,
            )
            .await
            .unwrap();
        assert!(all_unknown.metadata.is_none());
        assert!(!all_unknown.output.contains("```diff"));
    }
}
