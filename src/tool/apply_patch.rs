use super::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use similar::{ChangeTag, TextDiff};
use std::path::Path;

pub struct ApplyPatchTool;

impl ApplyPatchTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct ApplyPatchInput {
    patch_text: String,
}

#[derive(Debug, Clone)]
struct UpdateFileChunk {
    change_context: Option<String>,
    old_lines: Vec<String>,
    new_lines: Vec<String>,
    is_end_of_file: bool,
}

#[derive(Debug)]
enum PatchHunk {
    AddFile {
        path: String,
        contents: String,
    },
    DeleteFile {
        path: String,
    },
    UpdateFile {
        path: String,
        move_to: Option<String>,
        chunks: Vec<UpdateFileChunk>,
    },
}

#[async_trait]
impl Tool for ApplyPatchTool {
    fn name(&self) -> &str {
        "apply_patch"
    }

    fn description(&self) -> &str {
        "Apply a patch in the Codex apply_patch format (*** Begin Patch). Supports add, update, and delete operations."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["patch_text"],
            "properties": {
                "patch_text": {
                    "type": "string",
                    "description": "Patch text in apply_patch format"
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: ApplyPatchInput = serde_json::from_value(input)?;
        let hunks = parse_apply_patch(&params.patch_text)?;

        let mut results = Vec::new();

        for hunk in &hunks {
            match hunk {
                PatchHunk::AddFile { path, contents } => {
                    let resolved = ctx.resolve_path(Path::new(path));
                    if let Some(parent) = resolved.parent() {
                        tokio::fs::create_dir_all(parent).await?;
                    }
                    tokio::fs::write(&resolved, contents).await?;
                    let diff = generate_diff_summary("", contents);
                    if diff.is_empty() {
                        results.push(format!("✓ {}: created", path));
                    } else {
                        results.push(format!("✓ {}: created\n{}", path, diff));
                    }
                }
                PatchHunk::DeleteFile { path } => {
                    let resolved = ctx.resolve_path(Path::new(path));
                    let old_contents = tokio::fs::read_to_string(&resolved)
                        .await
                        .unwrap_or_default();
                    if tokio::fs::remove_file(&resolved).await.is_ok() {
                        let diff = generate_diff_summary(&old_contents, "");
                        if diff.is_empty() {
                            results.push(format!("✓ {}: deleted", path));
                        } else {
                            results.push(format!("✓ {}: deleted\n{}", path, diff));
                        }
                    } else {
                        results.push(format!("✗ {}: failed to delete", path));
                    }
                }
                PatchHunk::UpdateFile {
                    path,
                    move_to,
                    chunks,
                } => {
                    let resolved = ctx.resolve_path(Path::new(path));
                    match apply_update_chunks(&resolved, chunks).await {
                        Ok((old_contents, new_contents)) => {
                            let diff = generate_diff_summary(&old_contents, &new_contents);
                            if let Some(dest) = move_to {
                                let dest_resolved = ctx.resolve_path(Path::new(dest));
                                if let Some(parent) = dest_resolved.parent() {
                                    tokio::fs::create_dir_all(parent).await?;
                                }
                                tokio::fs::write(&dest_resolved, &new_contents).await?;
                                let _ = tokio::fs::remove_file(&resolved).await;
                                if diff.is_empty() {
                                    results.push(format!(
                                        "✓ {}: modified ({} hunks), moved to {}",
                                        path,
                                        chunks.len(),
                                        dest
                                    ));
                                } else {
                                    results.push(format!(
                                        "✓ {}: modified ({} hunks), moved to {}\n{}",
                                        path,
                                        chunks.len(),
                                        dest,
                                        diff
                                    ));
                                }
                            } else {
                                tokio::fs::write(&resolved, &new_contents).await?;
                                if diff.is_empty() {
                                    results.push(format!(
                                        "✓ {}: modified ({} hunks)",
                                        path,
                                        chunks.len()
                                    ));
                                } else {
                                    results.push(format!(
                                        "✓ {}: modified ({} hunks)\n{}",
                                        path,
                                        chunks.len(),
                                        diff
                                    ));
                                }
                            }
                        }
                        Err(e) => {
                            results.push(format!("✗ {}: {}", path, e));
                        }
                    }
                }
            }
        }

        if results.is_empty() {
            Ok(ToolOutput::new("No changes applied"))
        } else {
            Ok(ToolOutput::new(results.join("\n")))
        }
    }
}

async fn apply_update_chunks(path: &Path, chunks: &[UpdateFileChunk]) -> Result<(String, String)> {
    let original_contents = tokio::fs::read_to_string(path).await?;
    let mut original_lines: Vec<String> = original_contents.split('\n').map(String::from).collect();

    if original_lines.last().is_some_and(String::is_empty) {
        original_lines.pop();
    }

    let replacements = compute_replacements(&original_lines, path, chunks)?;
    let mut new_lines = apply_replacements(original_lines, &replacements);

    if !new_lines.last().is_some_and(String::is_empty) {
        new_lines.push(String::new());
    }
    Ok((original_contents, new_lines.join("\n")))
}

/// Generate a compact diff with line numbers (max 30 lines).
fn generate_diff_summary(old: &str, new: &str) -> String {
    let diff = TextDiff::from_lines(old, new);
    let mut output = String::new();
    let mut line_count = 0;
    const MAX_LINES: usize = 30;

    let mut old_line = 1usize;
    let mut new_line = 1usize;

    for change in diff.iter_all_changes() {
        if line_count >= MAX_LINES {
            output.push_str("... (diff truncated)\n");
            break;
        }

        let content = change.value().trim_end_matches('\n');
        let (prefix, line_num) = match change.tag() {
            ChangeTag::Delete => {
                let num = old_line;
                old_line += 1;
                if content.trim().is_empty() {
                    continue;
                }
                ("-", num)
            }
            ChangeTag::Insert => {
                let num = new_line;
                new_line += 1;
                if content.trim().is_empty() {
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

        output.push_str(&format!("{}{} {}\n", line_num, prefix, content));
        line_count += 1;
    }

    output.trim_end().to_string()
}

fn compute_replacements(
    original_lines: &[String],
    path: &Path,
    chunks: &[UpdateFileChunk],
) -> Result<Vec<(usize, usize, Vec<String>)>> {
    let mut replacements: Vec<(usize, usize, Vec<String>)> = Vec::new();
    let mut line_index: usize = 0;

    for chunk in chunks {
        if let Some(ctx_line) = &chunk.change_context {
            if let Some(idx) = seek_sequence(
                original_lines,
                std::slice::from_ref(ctx_line),
                line_index,
                false,
            ) {
                line_index = idx + 1;
            } else {
                anyhow::bail!(
                    "Failed to find context '{}' in {}",
                    ctx_line,
                    path.display()
                );
            }
        }

        if chunk.old_lines.is_empty() {
            let insertion_idx = if original_lines.last().is_some_and(String::is_empty) {
                original_lines.len() - 1
            } else {
                original_lines.len()
            };
            replacements.push((insertion_idx, 0, chunk.new_lines.clone()));
            continue;
        }

        let mut pattern: &[String] = &chunk.old_lines;
        let mut found = seek_sequence(original_lines, pattern, line_index, chunk.is_end_of_file);

        let mut new_slice: &[String] = &chunk.new_lines;

        if found.is_none() && pattern.last().is_some_and(String::is_empty) {
            pattern = &pattern[..pattern.len() - 1];
            if new_slice.last().is_some_and(String::is_empty) {
                new_slice = &new_slice[..new_slice.len() - 1];
            }
            found = seek_sequence(original_lines, pattern, line_index, chunk.is_end_of_file);
        }

        if let Some(start_idx) = found {
            replacements.push((start_idx, pattern.len(), new_slice.to_vec()));
            line_index = start_idx + pattern.len();
        } else {
            anyhow::bail!(
                "Failed to find expected lines in {}:\n{}",
                path.display(),
                chunk.old_lines.join("\n"),
            );
        }
    }

    replacements.sort_by(|(a, _, _), (b, _, _)| a.cmp(b));
    Ok(replacements)
}

fn apply_replacements(
    mut lines: Vec<String>,
    replacements: &[(usize, usize, Vec<String>)],
) -> Vec<String> {
    for (start_idx, old_len, new_segment) in replacements.iter().rev() {
        let start_idx = *start_idx;
        let old_len = *old_len;

        for _ in 0..old_len {
            if start_idx < lines.len() {
                lines.remove(start_idx);
            }
        }

        for (offset, new_line) in new_segment.iter().enumerate() {
            lines.insert(start_idx + offset, new_line.clone());
        }
    }

    lines
}

fn seek_sequence(lines: &[String], pattern: &[String], start: usize, eof: bool) -> Option<usize> {
    if pattern.is_empty() {
        return Some(start);
    }

    if pattern.len() > lines.len() {
        return None;
    }

    let search_start = if eof && lines.len() >= pattern.len() {
        lines.len() - pattern.len()
    } else {
        start
    };

    for i in search_start..=lines.len().saturating_sub(pattern.len()) {
        if lines[i..i + pattern.len()] == *pattern {
            return Some(i);
        }
    }

    for i in search_start..=lines.len().saturating_sub(pattern.len()) {
        let mut ok = true;
        for (p_idx, pat) in pattern.iter().enumerate() {
            if lines[i + p_idx].trim_end() != pat.trim_end() {
                ok = false;
                break;
            }
        }
        if ok {
            return Some(i);
        }
    }

    for i in search_start..=lines.len().saturating_sub(pattern.len()) {
        let mut ok = true;
        for (p_idx, pat) in pattern.iter().enumerate() {
            if lines[i + p_idx].trim() != pat.trim() {
                ok = false;
                break;
            }
        }
        if ok {
            return Some(i);
        }
    }

    None
}

fn parse_apply_patch(input: &str) -> Result<Vec<PatchHunk>> {
    let lines: Vec<&str> = input.lines().collect();

    let start = lines
        .iter()
        .position(|l| l.trim() == "*** Begin Patch")
        .ok_or_else(|| anyhow::anyhow!("Patch must contain *** Begin Patch"))?;

    let mut hunks = Vec::new();
    let mut i = start + 1;

    while i < lines.len() {
        let line = lines[i].trim_end();
        if line.trim() == "*** End Patch" {
            break;
        }

        if let Some(path) = line.strip_prefix("*** Add File: ") {
            let path = path.trim().to_string();
            i += 1;
            let mut contents = String::new();
            while i < lines.len() {
                let current = lines[i];
                if current.starts_with("*** ") {
                    break;
                }
                if let Some(added) = current.strip_prefix('+') {
                    contents.push_str(added);
                    contents.push('\n');
                }
                i += 1;
            }
            hunks.push(PatchHunk::AddFile { path, contents });
            continue;
        }

        if let Some(path) = line.strip_prefix("*** Delete File: ") {
            hunks.push(PatchHunk::DeleteFile {
                path: path.trim().to_string(),
            });
            i += 1;
            continue;
        }

        if let Some(path) = line.strip_prefix("*** Update File: ") {
            let path = path.trim().to_string();
            i += 1;

            let mut move_to = None;
            if i < lines.len() {
                if let Some(target) = lines[i].trim_end().strip_prefix("*** Move to: ") {
                    move_to = Some(target.trim().to_string());
                    i += 1;
                }
            }

            let mut chunks = Vec::new();
            let mut is_first_chunk = true;

            while i < lines.len() {
                let current = lines[i].trim_end();

                if current.starts_with("*** ") && current != "*** End of File" {
                    break;
                }

                if current.trim().is_empty()
                    && !current.starts_with(' ')
                    && !current.starts_with('+')
                    && !current.starts_with('-')
                {
                    i += 1;
                    continue;
                }

                let change_context;
                if current == "@@" {
                    change_context = None;
                    i += 1;
                } else if let Some(ctx) = current.strip_prefix("@@ ") {
                    change_context = Some(ctx.to_string());
                    i += 1;
                } else if is_first_chunk {
                    change_context = None;
                } else {
                    break;
                }

                let mut old_lines = Vec::new();
                let mut new_lines = Vec::new();
                let mut is_end_of_file = false;
                let mut had_diff_lines = false;

                while i < lines.len() {
                    let cl = lines[i];

                    if cl == "*** End of File" {
                        is_end_of_file = true;
                        i += 1;
                        break;
                    }

                    if cl.starts_with("*** ") || cl.starts_with("@@") {
                        break;
                    }

                    if let Some(content) = cl.strip_prefix(' ') {
                        old_lines.push(content.to_string());
                        new_lines.push(content.to_string());
                        had_diff_lines = true;
                    } else if let Some(content) = cl.strip_prefix('+') {
                        new_lines.push(content.to_string());
                        had_diff_lines = true;
                    } else if let Some(content) = cl.strip_prefix('-') {
                        old_lines.push(content.to_string());
                        had_diff_lines = true;
                    } else if cl.is_empty() {
                        old_lines.push(String::new());
                        new_lines.push(String::new());
                        had_diff_lines = true;
                    } else {
                        if had_diff_lines {
                            break;
                        }
                        i += 1;
                        continue;
                    }

                    i += 1;
                }

                if had_diff_lines || change_context.is_some() {
                    chunks.push(UpdateFileChunk {
                        change_context,
                        old_lines,
                        new_lines,
                        is_end_of_file,
                    });
                }

                is_first_chunk = false;
            }

            if chunks.is_empty() {
                anyhow::bail!("Update file hunk for '{}' has no changes", path);
            }

            hunks.push(PatchHunk::UpdateFile {
                path,
                move_to,
                chunks,
            });
            continue;
        }

        i += 1;
    }

    if hunks.is_empty() {
        anyhow::bail!("No valid patch directives found");
    }

    Ok(hunks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn write_temp(content: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f
    }

    #[test]
    fn test_parse_add_file() {
        let patch =
            "*** Begin Patch\n*** Add File: hello.txt\n+Hello world\n+Second line\n*** End Patch";
        let hunks = parse_apply_patch(patch).unwrap();
        assert_eq!(hunks.len(), 1);
        match &hunks[0] {
            PatchHunk::AddFile { path, contents } => {
                assert_eq!(path, "hello.txt");
                assert_eq!(contents, "Hello world\nSecond line\n");
            }
            _ => panic!("Expected AddFile"),
        }
    }

    #[test]
    fn test_parse_delete_file() {
        let patch = "*** Begin Patch\n*** Delete File: old.txt\n*** End Patch";
        let hunks = parse_apply_patch(patch).unwrap();
        assert_eq!(hunks.len(), 1);
        match &hunks[0] {
            PatchHunk::DeleteFile { path } => {
                assert_eq!(path, "old.txt");
            }
            _ => panic!("Expected DeleteFile"),
        }
    }

    #[test]
    fn test_parse_update_file_simple() {
        let patch =
            "*** Begin Patch\n*** Update File: test.py\n@@\n foo\n-bar\n+baz\n*** End Patch";
        let hunks = parse_apply_patch(patch).unwrap();
        assert_eq!(hunks.len(), 1);
        match &hunks[0] {
            PatchHunk::UpdateFile { path, chunks, .. } => {
                assert_eq!(path, "test.py");
                assert_eq!(chunks.len(), 1);
                assert_eq!(chunks[0].old_lines, vec!["foo", "bar"]);
                assert_eq!(chunks[0].new_lines, vec!["foo", "baz"]);
            }
            _ => panic!("Expected UpdateFile"),
        }
    }

    #[test]
    fn test_parse_update_with_context() {
        let patch = "*** Begin Patch\n*** Update File: test.py\n@@ def my_func():\n-    pass\n+    return 42\n*** End Patch";
        let hunks = parse_apply_patch(patch).unwrap();
        match &hunks[0] {
            PatchHunk::UpdateFile { chunks, .. } => {
                assert_eq!(chunks[0].change_context, Some("def my_func():".to_string()));
                assert_eq!(chunks[0].old_lines, vec!["    pass"]);
                assert_eq!(chunks[0].new_lines, vec!["    return 42"]);
            }
            _ => panic!("Expected UpdateFile"),
        }
    }

    #[test]
    fn test_parse_update_with_move() {
        let patch = "*** Begin Patch\n*** Update File: old.py\n*** Move to: new.py\n@@\n-old_line\n+new_line\n*** End Patch";
        let hunks = parse_apply_patch(patch).unwrap();
        match &hunks[0] {
            PatchHunk::UpdateFile {
                path,
                move_to,
                chunks,
            } => {
                assert_eq!(path, "old.py");
                assert_eq!(move_to, &Some("new.py".to_string()));
                assert_eq!(chunks.len(), 1);
            }
            _ => panic!("Expected UpdateFile"),
        }
    }

    #[test]
    fn test_parse_multiple_chunks() {
        let patch = "*** Begin Patch\n*** Update File: test.py\n@@\n foo\n-bar\n+BAR\n@@\n baz\n-qux\n+QUX\n*** End Patch";
        let hunks = parse_apply_patch(patch).unwrap();
        match &hunks[0] {
            PatchHunk::UpdateFile { chunks, .. } => {
                assert_eq!(chunks.len(), 2);
                assert_eq!(chunks[0].old_lines, vec!["foo", "bar"]);
                assert_eq!(chunks[0].new_lines, vec!["foo", "BAR"]);
                assert_eq!(chunks[1].old_lines, vec!["baz", "qux"]);
                assert_eq!(chunks[1].new_lines, vec!["baz", "QUX"]);
            }
            _ => panic!("Expected UpdateFile"),
        }
    }

    #[test]
    fn test_parse_end_of_file() {
        let patch = "*** Begin Patch\n*** Update File: test.py\n@@\n last_line\n+new_last_line\n*** End of File\n*** End Patch";
        let hunks = parse_apply_patch(patch).unwrap();
        match &hunks[0] {
            PatchHunk::UpdateFile { chunks, .. } => {
                assert!(chunks[0].is_end_of_file);
            }
            _ => panic!("Expected UpdateFile"),
        }
    }

    #[tokio::test]
    async fn test_apply_update_simple() {
        let f = write_temp("foo\nbar\n");
        let chunks = vec![UpdateFileChunk {
            change_context: None,
            old_lines: vec!["foo".to_string(), "bar".to_string()],
            new_lines: vec!["foo".to_string(), "baz".to_string()],
            is_end_of_file: false,
        }];
        let (old_result, new_result) = apply_update_chunks(f.path(), &chunks).await.unwrap();
        assert_eq!(old_result, "foo\nbar\n");
        assert_eq!(new_result, "foo\nbaz\n");
    }

    #[tokio::test]
    async fn test_apply_update_multiple_chunks() {
        let f = write_temp("foo\nbar\nbaz\nqux\n");
        let chunks = vec![
            UpdateFileChunk {
                change_context: None,
                old_lines: vec!["foo".to_string(), "bar".to_string()],
                new_lines: vec!["foo".to_string(), "BAR".to_string()],
                is_end_of_file: false,
            },
            UpdateFileChunk {
                change_context: None,
                old_lines: vec!["baz".to_string(), "qux".to_string()],
                new_lines: vec!["baz".to_string(), "QUX".to_string()],
                is_end_of_file: false,
            },
        ];
        let (old_result, new_result) = apply_update_chunks(f.path(), &chunks).await.unwrap();
        assert_eq!(old_result, "foo\nbar\nbaz\nqux\n");
        assert_eq!(new_result, "foo\nBAR\nbaz\nQUX\n");
    }

    #[tokio::test]
    async fn test_apply_update_with_context_header() {
        let f = write_temp(
            "class Foo:\n    def bar(self):\n        pass\n    def baz(self):\n        pass\n",
        );
        let chunks = vec![UpdateFileChunk {
            change_context: Some("def baz(self):".to_string()),
            old_lines: vec!["        pass".to_string()],
            new_lines: vec!["        return 42".to_string()],
            is_end_of_file: false,
        }];
        let (_old_result, new_result) = apply_update_chunks(f.path(), &chunks).await.unwrap();
        assert_eq!(
            new_result,
            "class Foo:\n    def bar(self):\n        pass\n    def baz(self):\n        return 42\n"
        );
    }

    #[tokio::test]
    async fn test_apply_update_append_at_eof() {
        let f = write_temp("foo\nbar\nbaz\n");
        let chunks = vec![UpdateFileChunk {
            change_context: None,
            old_lines: vec![],
            new_lines: vec!["quux".to_string()],
            is_end_of_file: false,
        }];
        let (_old_result, new_result) = apply_update_chunks(f.path(), &chunks).await.unwrap();
        assert_eq!(new_result, "foo\nbar\nbaz\nquux\n");
    }

    #[test]
    fn test_generate_diff_summary_compact_format() {
        let old = "line one\nline two\nline three\n";
        let new = "line one\nchanged two\nline three\n";
        let diff = generate_diff_summary(old, new);

        assert!(diff.contains("2- line two"));
        assert!(diff.contains("2+ changed two"));
        assert!(!diff.contains("line one"));
    }

    #[test]
    fn test_seek_sequence_exact() {
        let lines: Vec<String> = vec!["foo", "bar", "baz"]
            .into_iter()
            .map(String::from)
            .collect();
        let pattern: Vec<String> = vec!["bar", "baz"].into_iter().map(String::from).collect();
        assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(1));
    }

    #[test]
    fn test_seek_sequence_whitespace_tolerant() {
        let lines: Vec<String> = vec!["foo   ", "bar\t"]
            .into_iter()
            .map(String::from)
            .collect();
        let pattern: Vec<String> = vec!["foo", "bar"].into_iter().map(String::from).collect();
        assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(0));
    }

    #[test]
    fn test_seek_sequence_eof() {
        let lines: Vec<String> = vec!["a", "b", "c", "d"]
            .into_iter()
            .map(String::from)
            .collect();
        let pattern: Vec<String> = vec!["c", "d"].into_iter().map(String::from).collect();
        assert_eq!(seek_sequence(&lines, &pattern, 0, true), Some(2));
    }

    #[test]
    fn test_parse_no_begin() {
        let result = parse_apply_patch("random text");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_heredoc_wrapper() {
        let patch = "<<'EOF'\n*** Begin Patch\n*** Add File: test.txt\n+hello\n*** End Patch\nEOF";
        let hunks = parse_apply_patch(patch).unwrap();
        assert_eq!(hunks.len(), 1);
    }

    #[test]
    fn test_parse_update_without_explicit_at() {
        let patch = "*** Begin Patch\n*** Update File: file.py\n import foo\n+bar\n*** End Patch";
        let hunks = parse_apply_patch(patch).unwrap();
        match &hunks[0] {
            PatchHunk::UpdateFile { chunks, .. } => {
                assert_eq!(chunks.len(), 1);
                assert!(chunks[0].change_context.is_none());
            }
            _ => panic!("Expected UpdateFile"),
        }
    }
}
