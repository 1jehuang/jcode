use super::{Tool, ToolContext, ToolOutput};
use crate::bus::{Bus, BusEvent, FileOp, FileTouch};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use similar::{ChangeTag, TextDiff};
use std::path::Path;

const FILE_TOUCH_PREVIEW_MAX_LINES: usize = 6;
const FILE_TOUCH_PREVIEW_MAX_BYTES: usize = 240;

pub struct EditTool;

impl EditTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct EditInput {
    #[serde(default)]
    intent: Option<String>,
    file_path: String,
    #[serde(default)]
    edits: Option<Vec<EditOperation>>,
    #[serde(default)]
    old_string: Option<String>,
    #[serde(default)]
    new_string: Option<String>,
    #[serde(default)]
    replace_all: bool,
}

#[derive(Deserialize, Clone)]
struct EditOperation {
    old_string: String,
    new_string: String,
    #[serde(default)]
    replace_all: bool,
}

impl EditInput {
    /// Accept either the `edits` array or the single-edit shorthand, never both.
    fn operations(&self) -> Result<Vec<EditOperation>> {
        let single = self.old_string.is_some() || self.new_string.is_some();
        match (&self.edits, single) {
            (Some(_), true) => Err(anyhow::anyhow!(
                "Use either `edits` or `old_string`/`new_string`, not both."
            )),
            (Some(edits), false) if edits.is_empty() => {
                Err(anyhow::anyhow!("`edits` must contain at least one edit."))
            }
            (Some(edits), false) => Ok(edits.clone()),
            (None, true) => Ok(vec![EditOperation {
                old_string: self.old_string.clone().ok_or_else(|| {
                    anyhow::anyhow!("`old_string` is required with `new_string`.")
                })?,
                new_string: self.new_string.clone().ok_or_else(|| {
                    anyhow::anyhow!("`new_string` is required with `old_string`.")
                })?,
                replace_all: self.replace_all,
            }]),
            (None, false) => Err(anyhow::anyhow!(
                "Provide `edits` (array of old_string/new_string) or `old_string` and `new_string`."
            )),
        }
    }
}

struct AppliedEdit {
    occurrences: usize,
    start_line: usize,
}

/// Apply every edit in order to an in-memory copy. Any failure aborts the whole
/// call so the file is never left half-edited.
fn apply_edits(
    original: &str,
    edits: &[EditOperation],
    file_path: &str,
) -> Result<(String, Vec<AppliedEdit>)> {
    let mut content = original.to_string();
    let mut applied = Vec::with_capacity(edits.len());
    let mut failures = Vec::new();
    let label = |index: usize| {
        if edits.len() == 1 {
            String::new()
        } else {
            format!("Edit {}: ", index + 1)
        }
    };

    for (index, edit) in edits.iter().enumerate() {
        if edit.old_string == edit.new_string {
            failures.push(format!(
                "{}old_string and new_string must be different",
                label(index)
            ));
            continue;
        }
        if edit.old_string.is_empty() {
            failures.push(format!("{}old_string must not be empty", label(index)));
            continue;
        }
        let occurrences = content.matches(&edit.old_string).count();
        if occurrences == 0 {
            let hint = flexible_match_hint(&content, &edit.old_string, file_path);
            failures.push(format!("{}{hint}", label(index)));
            continue;
        }
        if occurrences > 1 && !edit.replace_all {
            failures.push(format!(
                "{}old_string found {occurrences} times. Either:\n\
                 1. Provide more context to make it unique, or\n\
                 2. Set replace_all: true to replace all occurrences",
                label(index)
            ));
            continue;
        }
        let start_line = find_line_number(&content, &edit.old_string);
        content = if edit.replace_all {
            content.replace(&edit.old_string, &edit.new_string)
        } else {
            content.replacen(&edit.old_string, &edit.new_string, 1)
        };
        applied.push(AppliedEdit {
            occurrences,
            start_line,
        });
    }

    if failures.is_empty() {
        return Ok((content, applied));
    }
    if edits.len() == 1 {
        return Err(anyhow::anyhow!(failures.remove(0)));
    }
    Err(anyhow::anyhow!(
        "No changes written to {file_path}. {} of {} edits failed:\n{}\n\
         Edits apply in order, so later edits see the result of earlier ones.",
        failures.len(),
        edits.len(),
        failures
            .iter()
            .map(|failure| format!("  ✗ {failure}"))
            .collect::<Vec<_>>()
            .join("\n")
    ))
}

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &str {
        "edit"
    }

    fn description(&self) -> &str {
        "Edit a file by exact replacement. All edits apply or none do."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["file_path"],
            "properties": {
                "intent": super::intent_schema_property(),
                "file_path": {
                    "type": "string",
                    "description": "File path."
                },
                "edits": {
                    "type": "array",
                    "description": "Replacements applied in order. Each old_string must match exactly once unless replace_all is set.",
                    "minItems": 1,
                    "items": {
                        "type": "object",
                        "required": ["old_string", "new_string"],
                        "properties": {
                            "old_string": {"type": "string", "description": "Exact text to replace."},
                            "new_string": {"type": "string", "description": "Replacement text."},
                            "replace_all": {"type": "boolean", "description": "Replace every match."}
                        }
                    }
                },
                "old_string": {
                    "type": "string",
                    "description": "Single-edit shorthand: exact text to replace. Omit when using edits."
                },
                "new_string": {
                    "type": "string",
                    "description": "Single-edit shorthand: replacement text."
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "Single-edit shorthand: replace every match."
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: EditInput = serde_json::from_value(input)?;
        let edits = params.operations()?;
        let path = ctx.resolve_path(Path::new(&params.file_path));

        if !path.exists() {
            return Err(anyhow::anyhow!("File not found: {}", params.file_path));
        }

        let _lock = super::file_lock::lock(&path).await;
        let content = tokio::fs::read_to_string(&path).await?;
        let (new_content, applied) = apply_edits(&content, &edits, &params.file_path)?;

        tokio::fs::write(&path, &new_content).await?;
        super::edit_stats::record(&ctx, &content, &new_content, false).await;

        let intent = params
            .intent
            .clone()
            .filter(|value| !value.trim().is_empty());

        let mut body = if let [edit] = edits.as_slice() {
            let applied = &applied[0];
            let diff = generate_diff(&edit.old_string, &edit.new_string, applied.start_line);
            let end_line =
                applied.start_line + edit.new_string.lines().count().saturating_sub(1);
            Bus::global().publish(BusEvent::FileTouch(FileTouch {
                session_id: ctx.session_id.clone(),
                path: path.to_path_buf(),
                op: FileOp::Edit,
                intent,
                summary: Some(format!(
                    "edited lines {}-{} ({} occurrence{})",
                    applied.start_line,
                    end_line,
                    applied.occurrences,
                    if applied.occurrences == 1 { "" } else { "s" }
                )),
                detail: build_file_touch_preview(&diff),
            }));
            let context = extract_context(&new_content, applied.start_line, end_line, 3);
            format!(
                "Edited {}: replaced {} occurrence(s)\n{}\n\nContext after edit (lines {}-{}):\n{}",
                params.file_path, applied.occurrences, diff, context.0, context.1, context.2
            )
        } else {
            let diff = generate_diff_summary(&content, &new_content);
            let replaced: usize = applied.iter().map(|edit| edit.occurrences).sum();
            Bus::global().publish(BusEvent::FileTouch(FileTouch {
                session_id: ctx.session_id.clone(),
                path: path.to_path_buf(),
                op: FileOp::Edit,
                intent,
                summary: Some(format!(
                    "applied {} edits ({replaced} replacement{})",
                    edits.len(),
                    if replaced == 1 { "" } else { "s" }
                )),
                detail: build_file_touch_preview(&diff),
            }));
            let mut body = format!("Edited {}: applied {} edits\n", params.file_path, edits.len());
            for (index, edit) in applied.iter().enumerate() {
                body.push_str(&format!(
                    "  ✓ Edit {}: replaced {} occurrence{} at line {}\n",
                    index + 1,
                    edit.occurrences,
                    if edit.occurrences == 1 { "" } else { "s" },
                    edit.start_line
                ));
            }
            if !diff.is_empty() {
                body.push_str("\nDiff:\n");
                body.push_str(&diff);
            }
            body
        };
        super::config_edit_notice::append_config_edit_notice(
            &mut body,
            &path,
            &content,
            &new_content,
        );

        Ok(super::file_diff::attach(
            ToolOutput::new(body).with_title(params.file_path.clone()),
            super::file_diff::unified(&params.file_path, &params.file_path, &content, &new_content),
        ))
    }
}

/// Find the 1-based line number where a substring starts
fn find_line_number(content: &str, substring: &str) -> usize {
    if let Some(pos) = content.find(substring) {
        content[..pos].bytes().filter(|&byte| byte == b'\n').count() + 1
    } else {
        1
    }
}

/// Generate a compact diff: "42- old" / "42+ new"
fn generate_diff(old: &str, new: &str, start_line: usize) -> String {
    let diff = TextDiff::from_lines(old, new);
    let mut output = String::new();

    let mut old_line = start_line;
    let mut new_line = start_line;

    for change in diff.iter_all_changes() {
        let content = change.value().trim();
        let (prefix, line_num) = match change.tag() {
            ChangeTag::Delete => {
                let num = old_line;
                old_line += 1;
                if content.is_empty() {
                    continue;
                }
                ("-", num)
            }
            ChangeTag::Insert => {
                let num = new_line;
                new_line += 1;
                if content.is_empty() {
                    continue;
                }
                ("+", num)
            }
            ChangeTag::Equal => {
                old_line += 1;
                new_line += 1;
                continue;
            }
        };

        // Compact format: "42- content" (no spaces)
        output.push_str(&format!("{}{} {}\n", line_num, prefix, content));
    }

    if output.is_empty() {
        String::new()
    } else {
        output.trim_end().to_string()
    }
}

fn build_file_touch_preview(diff: &str) -> Option<String> {
    let trimmed = diff.trim();
    if trimmed.is_empty() {
        return None;
    }

    let mut lines = trimmed.lines();
    let mut preview = lines
        .by_ref()
        .take(FILE_TOUCH_PREVIEW_MAX_LINES)
        .collect::<Vec<_>>()
        .join("\n");
    let mut truncated = lines.next().is_some();

    if preview.len() > FILE_TOUCH_PREVIEW_MAX_BYTES {
        preview = crate::util::truncate_str(&preview, FILE_TOUCH_PREVIEW_MAX_BYTES)
            .trim_end()
            .to_string();
        truncated = true;
    }

    if truncated {
        preview.push_str("\n…");
    }

    Some(preview)
}

/// Extract lines around the edited region, returns (start_line, end_line, content)
fn extract_context(
    content: &str,
    edit_start: usize,
    edit_end: usize,
    padding: usize,
) -> (usize, usize, String) {
    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();

    // Calculate range with padding (1-indexed to 0-indexed)
    let start = edit_start.saturating_sub(padding + 1);
    let end = (edit_end + padding).min(total_lines);

    let context_lines: Vec<String> = lines[start..end]
        .iter()
        .enumerate()
        .map(|(i, line)| format!("{:>4}│ {}", start + i + 1, line))
        .collect();

    (start + 1, end, context_lines.join("\n"))
}

fn flexible_match_hint(content: &str, old_string: &str, file_path: &str) -> String {
    let trimmed = old_string.trim();
    if !trimmed.is_empty() && trimmed != old_string && content.contains(trimmed) {
        return "old_string not found exactly, but found after trimming whitespace. \
                Use the exact string from the file, including leading/trailing whitespace."
            .to_string();
    }

    let old_lines: Vec<&str> = old_string.lines().collect();
    let content_lines: Vec<&str> = content.lines().collect();
    if !old_lines.is_empty() {
        for (i, window) in content_lines.windows(old_lines.len()).enumerate() {
            if window
                .iter()
                .zip(old_lines.iter())
                .all(|(a, b)| a.trim() == b.trim())
            {
                return format!(
                    "old_string not found exactly, but found with different indentation around line {}. \
                     Preserve the exact whitespace from the file.",
                    i + 1
                );
            }
        }
    }

    format!("old_string not found in {file_path}. Use the read tool to see the current file contents.")
}

/// Generate a compact whole-file diff: "42- old" / "42+ new" (max 30 lines)
fn generate_diff_summary(old: &str, new: &str) -> String {
    const MAX_LINES: usize = 30;
    let diff = TextDiff::from_lines(old, new);
    let mut output = String::new();
    let mut lines_shown = 0;
    let mut old_line = 1usize;
    let mut new_line = 1usize;

    for change in diff.iter_all_changes() {
        let (prefix, number) = match change.tag() {
            ChangeTag::Equal => {
                old_line += 1;
                new_line += 1;
                continue;
            }
            ChangeTag::Delete => {
                old_line += 1;
                ("-", old_line - 1)
            }
            ChangeTag::Insert => {
                new_line += 1;
                ("+", new_line - 1)
            }
        };
        let content = change.value().trim();
        if content.is_empty() {
            continue;
        }
        if lines_shown >= MAX_LINES {
            output.push_str("...\n");
            break;
        }
        output.push_str(&format!("{number}{prefix} {content}\n"));
        lines_shown += 1;
    }

    output.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_diff_single_line_change() {
        let old = "hello world";
        let new = "hello rust";
        let diff = generate_diff(old, new, 10);

        // Compact format: "10- content" / "10+ content"
        assert!(diff.contains("10- hello world"), "Should show deleted line");
        assert!(diff.contains("10+ hello rust"), "Should show added line");
    }

    #[test]
    fn test_generate_diff_multi_line() {
        let old = "line one\nline two\nline three";
        let new = "line one\nmodified two\nline three";
        let diff = generate_diff(old, new, 5);

        // Line 6 should be the changed line (5 + 1 for "line two")
        assert!(diff.contains("6- line two"), "Should show deleted line");
        assert!(diff.contains("6+ modified two"), "Should show added line");
        // Equal lines should not appear
        assert!(
            !diff.contains("line one"),
            "Should not show unchanged lines"
        );
        assert!(
            !diff.contains("line three"),
            "Should not show unchanged lines"
        );
    }

    #[test]
    fn test_generate_diff_addition_only() {
        let old = "first\nthird";
        let new = "first\nsecond\nthird";
        let diff = generate_diff(old, new, 1);

        assert!(diff.contains("+ second"), "Should show added line");
    }

    #[test]
    fn test_generate_diff_deletion_only() {
        let old = "first\nsecond\nthird";
        let new = "first\nthird";
        let diff = generate_diff(old, new, 1);

        assert!(diff.contains("- second"), "Should show deleted line");
    }

    #[test]
    fn test_generate_diff_no_changes() {
        let old = "same content";
        let new = "same content";
        let diff = generate_diff(old, new, 1);

        assert!(diff.is_empty(), "No changes should produce empty diff");
    }

    #[test]
    fn test_generate_diff_line_number_format() {
        let old = "old";
        let new = "new";
        let diff = generate_diff(old, new, 42);

        // Compact format: no padding
        assert!(
            diff.contains("42- old"),
            "Should have line number directly before minus"
        );
        assert!(
            diff.contains("42+ new"),
            "Should have line number directly before plus"
        );
    }

    fn op(old: &str, new: &str) -> EditOperation {
        EditOperation {
            old_string: old.into(),
            new_string: new.into(),
            replace_all: false,
        }
    }

    #[test]
    fn apply_edits_is_all_or_nothing() {
        let error = apply_edits(
            "alpha\nbeta\ngamma\n",
            &[op("alpha", "a"), op("missing", "x"), op("gamma", "g")],
            "f.rs",
        )
        .err()
        .unwrap()
        .to_string();
        assert!(error.contains("No changes written to f.rs"), "{error}");
        assert!(error.contains("1 of 3 edits failed"), "{error}");
        assert!(error.contains("Edit 2: old_string not found"), "{error}");
    }

    #[test]
    fn apply_edits_applies_sequentially() {
        let (content, applied) = apply_edits(
            "alpha\nbeta\n",
            &[op("alpha", "temp"), op("temp", "done"), op("beta", "b")],
            "f.rs",
        )
        .unwrap();
        assert_eq!(content, "done\nb\n");
        assert_eq!(applied.len(), 3);
        assert_eq!(applied[2].start_line, 2);
    }

    #[test]
    fn apply_edits_rejects_ambiguous_match_without_replace_all() {
        assert!(apply_edits("x x", &[op("x", "y")], "f").is_err());
        let mut all = op("x", "y");
        all.replace_all = true;
        let (content, applied) = apply_edits("x x", &[all], "f").unwrap();
        assert_eq!(content, "y y");
        assert_eq!(applied[0].occurrences, 2);
    }

    #[test]
    fn input_accepts_single_or_array_but_not_both() {
        let parse = |value: Value| serde_json::from_value::<EditInput>(value).unwrap();
        assert_eq!(
            parse(json!({"file_path":"f","old_string":"a","new_string":"b"}))
                .operations()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            parse(json!({"file_path":"f","edits":[{"old_string":"a","new_string":"b"},{"old_string":"c","new_string":"d"}]}))
                .operations()
                .unwrap()
                .len(),
            2
        );
        assert!(parse(json!({"file_path":"f","edits":[],"old_string":"a","new_string":"b"})).operations().is_err());
        assert!(parse(json!({"file_path":"f"})).operations().is_err());
        assert!(parse(json!({"file_path":"f","old_string":"a"})).operations().is_err());
    }

    #[test]
    fn test_find_line_number() {
        let content = "line 1\nline 2\nline 3\nline 4";

        assert_eq!(find_line_number(content, "line 1"), 1);
        assert_eq!(find_line_number(content, "line 2"), 2);
        assert_eq!(find_line_number(content, "ine 2"), 2);
        assert_eq!(find_line_number(content, "line 3"), 3);
        assert_eq!(find_line_number(content, "line 4"), 4);
        assert_eq!(find_line_number(content, "not found"), 1);
    }

    #[test]
    fn test_extract_context() {
        let content =
            "line 1\nline 2\nline 3\nline 4\nline 5\nline 6\nline 7\nline 8\nline 9\nline 10";

        // Edit at line 5, with 2 lines padding
        let (start, end, ctx) = extract_context(content, 5, 5, 2);

        assert_eq!(start, 3, "Should start at line 3 (5 - 2)");
        assert_eq!(end, 7, "Should end at line 7 (5 + 2)");
        assert!(ctx.contains("line 3"), "Should include line 3");
        assert!(ctx.contains("line 5"), "Should include edited line 5");
        assert!(ctx.contains("line 7"), "Should include line 7");
        assert!(!ctx.contains("line 2"), "Should not include line 2");
        assert!(!ctx.contains("line 8"), "Should not include line 8");
    }

    #[test]
    fn test_extract_context_at_start() {
        let content = "line 1\nline 2\nline 3\nline 4\nline 5";

        // Edit at line 1, with 2 lines padding - shouldn't go negative
        let (start, _end, ctx) = extract_context(content, 1, 1, 2);

        assert_eq!(start, 1, "Should start at line 1 (can't go before)");
        assert!(ctx.contains("line 1"), "Should include line 1");
        assert!(ctx.contains("line 3"), "Should include line 3");
    }

    #[test]
    fn test_extract_context_at_end() {
        let content = "line 1\nline 2\nline 3\nline 4\nline 5";

        // Edit at line 5, with 2 lines padding - shouldn't go past end
        let (_start, end, ctx) = extract_context(content, 5, 5, 2);

        assert_eq!(end, 5, "Should end at line 5 (can't go past)");
        assert!(ctx.contains("line 5"), "Should include line 5");
        assert!(ctx.contains("line 3"), "Should include line 3");
    }

    #[test]
    fn test_extract_context_range_past_end() {
        let content = "line 1\nline 2\nline 3\nline 4\nline 5";

        // Edit range extends past the end of the file.
        let (start, end, ctx) = extract_context(content, 4, 10, 1);

        assert_eq!(start, 3, "Should start at line 3 (4 - 1)");
        assert_eq!(end, 5, "Should clamp to last line");
        assert!(ctx.contains("line 3"), "Should include line 3");
        assert!(ctx.contains("line 5"), "Should include line 5");
    }
}
