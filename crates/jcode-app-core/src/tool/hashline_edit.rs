use super::{Tool, ToolContext, ToolOutput};
use crate::bus::{Bus, BusEvent, FileOp, FileTouch};
use anyhow::Result;
use async_trait::async_trait;
use hashline::sha256_window;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::Path;

pub struct HashlineEditTool;

impl Default for HashlineEditTool {
    fn default() -> Self {
        Self::new()
    }
}

impl HashlineEditTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct HashlineEditInput {
    #[serde(default)]
    intent: Option<String>,
    file_path: String,
    anchor: Anchor,
    old_string: String,
    new_string: String,
}

#[derive(Deserialize)]
struct Anchor {
    line: usize,
    hash_sha256: String,
    #[serde(default = "default_context_window")]
    context_window: usize,
}

fn default_context_window() -> usize {
    0
}

// Compute SHA-256 hash of the lines in the given range (1-indexed lines).
//
// The window math + hash format is implemented in the `hashline` crate
// (since 0.2.1). This file delegates so jcode and hashline stay in lock
// step on anchor semantics — bug fixes and edge-case handling land in
// one place.
//
// Helpers `hash_window`, `verify_anchor`, `apply_edit_within_window`
// previously defined here as ~180 lines of jcode-internal code now
// resolve to one-line wrappers around `hashline::sha256_window::*`.

#[cfg(test)]
#[inline]
fn hash_window(content: &str, start_line: usize, end_line: usize) -> String {
    sha256_window::hash_window(content, start_line, end_line)
}

#[inline]
fn verify_anchor(
    content: &str,
    anchor_line: usize,
    expected_hash: &str,
    context_window: usize,
) -> Result<()> {
    sha256_window::verify_anchor(content, anchor_line, expected_hash, context_window)
        .map_err(|e| anyhow::anyhow!("{e}"))
}

#[inline]
fn apply_edit_within_window(
    content: &str,
    anchor_line: usize,
    old_string: &str,
    new_string: &str,
    context_window: usize,
) -> Result<(String, usize, usize)> {
    sha256_window::apply_edit_within_window(
        content,
        anchor_line,
        old_string,
        new_string,
        context_window,
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}

#[async_trait]
impl Tool for HashlineEditTool {
    fn name(&self) -> &str {
        "hashline_edit"
    }

    fn description(&self) -> &str {
        "Perform surgical file edits anchored by line hash verification. Fails closed if the file drifted since planning."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["file_path", "anchor", "old_string", "new_string"],
            "properties": {
                "intent": super::intent_schema_property(),
                "file_path": {
                    "type": "string",
                    "description": "File path."
                },
                "anchor": {
                    "type": "object",
                    "required": ["line", "hash_sha256"],
                    "description": "Line hash anchor for edit verification.",
                    "properties": {
                        "line": {
                            "type": "integer",
                            "description": "1-based line number in the file."
                        },
                        "hash_sha256": {
                            "type": "string",
                            "description": "SHA-256 hash of the anchor window (line ± context_window)."
                        },
                        "context_window": {
                            "type": "integer",
                            "description": "Number of surrounding lines to include in hash (default: 0)."
                        }
                    }
                },
                "old_string": {
                    "type": "string",
                    "description": "Exact text to replace within the verified anchor window."
                },
                "new_string": {
                    "type": "string",
                    "description": "Replacement text."
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: HashlineEditInput = serde_json::from_value(input)?;

        if params.old_string == params.new_string {
            return Err(anyhow::anyhow!(
                "old_string and new_string must be different"
            ));
        }

        let path = ctx.resolve_path_checked(Path::new(&params.file_path))?;

        if !path.exists() {
            return Err(anyhow::anyhow!("File not found: {}", params.file_path));
        }

        let content = tokio::fs::read_to_string(&path).await?;

        // Step 1: Verify the anchor hash
        verify_anchor(
            &content,
            params.anchor.line,
            &params.anchor.hash_sha256,
            params.anchor.context_window,
        )?;

        // Step 2: Apply edit within the anchor window
        let (new_content, start_line, end_line) = apply_edit_within_window(
            &content,
            params.anchor.line,
            &params.old_string,
            &params.new_string,
            params.anchor.context_window,
        )?;

        // Step 3: Write back atomically via temp file + rename. Preserve
        // the original extension in the temp name (e.g. `foo.rs` →
        // `foo.rs.jcode-tmp`) so file watchers / build systems that filter
        // by extension don't trip on the temp file. Append a process-id
        // suffix so concurrent edits to different files don't collide.
        let pid = std::process::id();
        let temp_name = match path.file_name().and_then(|n| n.to_str()) {
            Some(name) => format!("{name}.jcode-tmp.{pid}"),
            None => format!("jcode-tmp.{pid}"),
        };
        let temp_path = path.with_file_name(temp_name);
        if let Err(e) = tokio::fs::write(&temp_path, &new_content).await {
            // Best-effort cleanup if write partially succeeded.
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(anyhow::anyhow!(
                "failed to write temp file {}: {}",
                temp_path.display(),
                e
            ));
        }
        if let Err(e) = tokio::fs::rename(&temp_path, &path).await {
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(anyhow::anyhow!(
                "failed to atomically rename {} → {}: {}",
                temp_path.display(),
                path.display(),
                e
            ));
        }

        // Publish file touch event for swarm coordination
        let detail = Some(format!(
            "lines {}-{}: {} → {}",
            start_line,
            end_line,
            params.old_string.lines().next().unwrap_or(""),
            params.new_string.lines().next().unwrap_or("")
        ));
        Bus::global().publish(BusEvent::FileTouch(FileTouch {
            session_id: ctx.session_id.clone(),
            path: path.to_path_buf(),
            op: FileOp::Edit,
            intent: params
                .intent
                .clone()
                .filter(|value| !value.trim().is_empty()),
            summary: Some(format!("hashline edit lines {}-{}", start_line, end_line)),
            detail,
        }));

        Ok(ToolOutput::new(format!(
            "Edited {}: lines {}-{} (anchor verified)\n  old: {}\n  new: {}",
            params.file_path,
            start_line,
            end_line,
            params.old_string.lines().next().unwrap_or(""),
            params.new_string.lines().next().unwrap_or("")
        ))
        .with_title(params.file_path.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_content() -> &'static str {
        "fn main() {\n    println!(\"hello\");\n    let x = 1;\n    println!(\"x={}\", x);\n}\n"
    }

    #[test]
    fn test_hash_window_single_line() {
        let content = test_content();
        // Line 1: "fn main() {"
        let h1 = hash_window(content, 1, 1);
        assert!(!h1.is_empty());

        // Line 2: "    println!(\"hello\");"
        let h2 = hash_window(content, 2, 2);
        assert!(!h2.is_empty());
        assert_ne!(h1, h2);

        // Hash is consistent
        assert_eq!(h1, hash_window(content, 1, 1));
        assert_eq!(h2, hash_window(content, 2, 2));
    }

    #[test]
    fn test_hash_window_multiple_lines() {
        let content = test_content();
        // Lines 2-3
        let h = hash_window(content, 2, 3);
        assert!(!h.is_empty());

        // Same range always produces same hash
        assert_eq!(h, hash_window(content, 2, 3));
    }

    #[test]
    fn test_hash_window_out_of_range() {
        let content = test_content();
        // Out of range returns empty
        assert!(hash_window(content, 100, 105).is_empty());
    }

    #[test]
    fn test_verify_anchor_success() {
        let content = test_content();
        let line = 2;
        let hash = hash_window(content, line, line);
        assert!(verify_anchor(content, line, &hash, 0).is_ok());
    }

    #[test]
    fn test_verify_anchor_with_context() {
        let content = test_content();
        let center = 2;
        let window_start = 1;
        let window_end = 3;
        let hash = hash_window(content, window_start, window_end);
        assert!(verify_anchor(content, center, &hash, 1).is_ok());
    }

    #[test]
    fn test_verify_anchor_drifted() {
        let content = test_content();
        let wrong_hash = "deadbeef".to_string();
        let result = verify_anchor(content, 2, &wrong_hash, 0);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("anchor drifted"));
    }

    #[test]
    fn test_verify_anchor_out_of_range() {
        let content = test_content();
        let hash = hash_window(content, 1, 1);
        let result = verify_anchor(content, 99, &hash, 0);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("out of range"));
    }

    #[test]
    fn test_apply_edit_success() {
        let content = test_content();
        let (new, start, _end) = apply_edit_within_window(
            content,
            2,
            "    println!(\"hello\");",
            "    println!(\"world\");",
            0,
        )
        .unwrap();
        assert!(new.contains("world"));
        assert!(!new.contains("hello"));
        assert_eq!(start, 2);
    }

    #[test]
    fn test_apply_edit_not_in_window() {
        let content = test_content();
        // Anchor at line 5, but content has only 5 lines total (line 5 = last line)
        // The edit targets "println" which is at line 2 - should fail with "anchor window out of range"
        let result = apply_edit_within_window(
            content,
            5,
            "    println!(\"hello\");",
            "    println!(\"world\");",
            0,
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("anchor window out of range")
                || err.to_string().contains("old_string not found")
        );
    }

    #[test]
    fn test_apply_edit_ambiguous() {
        let content = "    x = 1;\n    x = 2;\n";
        // With context_window=1, anchor=1 covers both lines.
        // Both lines contain "    x = " — must reject as ambiguous.
        let result = apply_edit_within_window(content, 1, "    x = ", "    y = ", 1);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("found 2 times"));
    }

    // Sanity: ctx=0 means "this line only" — even if the same string appears
    // on a sibling line, ctx=0 must not consider it.
    #[test]
    fn test_apply_edit_ctx_zero_isolates_to_anchor_line() {
        let content = "    x = 1;\n    x = 2;\n";
        let (new_content, start, end) =
            apply_edit_within_window(content, 1, "    x = ", "    y = ", 0)
                .expect("ctx=0 must operate only on the anchor line");
        assert_eq!(start, 1);
        assert_eq!(end, 1);
        // Only line 1 changed.
        let lines: Vec<&str> = new_content.lines().collect();
        assert!(lines[0].contains("y = 1"));
        assert!(lines[1].contains("x = 2"));
    }

    #[test]
    fn test_crlf_normalization() {
        // Windows line endings
        let content = "line1\r\nline2\r\nline3\r\n";
        // Hash should be consistent regardless of CRLF normalization
        let h = hash_window(content, 2, 2);
        assert!(!h.is_empty());
    }

    #[test]
    fn test_multibyte_content() {
        // CJK and emoji
        let content = "fn main() {\n    println!(\"你好\");\n    let emoji = \"🎉\";\n}\n";
        let h = hash_window(content, 2, 3);
        assert!(!h.is_empty());

        // Edit within that window should work
        let (_, start, _end) = apply_edit_within_window(
            content,
            2,
            "    println!(\"你好\");",
            "    println!(\"hola\");",
            0,
        )
        .unwrap();
        assert_eq!(start, 2);
    }

    // Regression: edits to the last line of a file used to fail with
    // "anchor window out of range" because the 0-indexed slice end was
    // confused with the 1-indexed line number, rejecting any anchor at
    // total_lines.
    #[test]
    fn test_apply_edit_on_last_line() {
        let content = "first\nsecond\nlast\n";
        let total_lines = content.lines().count();
        assert_eq!(total_lines, 3);

        let (new_content, start, end) = apply_edit_within_window(
            content, 3, // anchor on the last line
            "last", "final", 0,
        )
        .expect("editing the last line must work");

        assert!(new_content.contains("final"));
        assert!(!new_content.contains("last"));
        assert_eq!(start, 3);
        assert_eq!(end, 3);
    }

    #[test]
    fn test_apply_edit_on_last_line_with_context() {
        // Anchor at last line with context_window=1 — window covers lines 2-3.
        let content = "first\nsecond\nthird\n";
        let (new_content, _, _) = apply_edit_within_window(content, 3, "third", "fourth", 1)
            .expect("last line + context window must still resolve");
        assert!(new_content.contains("fourth"));
    }

    #[test]
    fn test_apply_edit_on_only_line() {
        // Single-line file.
        let content = "only\n";
        let (new_content, start, end) = apply_edit_within_window(content, 1, "only", "changed", 0)
            .expect("single-line file must be editable");
        assert!(new_content.starts_with("changed"));
        assert_eq!(start, 1);
        assert_eq!(end, 1);
    }

    #[test]
    fn test_verify_anchor_and_apply_edit_use_consistent_window() {
        // Hash computed for verify_anchor must match the window apply_edit
        // operates on, otherwise the verified region differs from the edited
        // region. This is the fundamental correctness invariant of the tool.
        let content =
            "fn main() {\n    println!(\"a\");\n    println!(\"b\");\n    println!(\"c\");\n}\n";
        for anchor in 1usize..=5 {
            for ctx in 0usize..=2 {
                let h = {
                    let total = content.lines().count();
                    let start = anchor.saturating_sub(ctx + 1);
                    let end = (anchor + ctx).min(total);
                    hash_window(content, start + 1, end)
                };
                let v = verify_anchor(content, anchor, &h, ctx);
                assert!(
                    v.is_ok(),
                    "verify failed for anchor={anchor}, ctx={ctx}, hash={h}: {:?}",
                    v.err()
                );
            }
        }
    }
}
