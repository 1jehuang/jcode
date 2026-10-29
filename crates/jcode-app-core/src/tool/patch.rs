use super::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use similar::{ChangeTag, TextDiff};
use std::path::{Path, PathBuf};

pub struct PatchTool;

impl PatchTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct PatchInput {
    patch_text: String,
}

#[derive(Debug)]
struct FilePatch {
    path: String,
    hunks: Vec<Hunk>,
    is_new: bool,
    is_delete: bool,
}

#[derive(Debug)]
struct Hunk {
    old_start: usize,
    old_count: usize,
    new_start: usize,
    new_count: usize,
    old_lines: Vec<String>,
    new_lines: Vec<String>,
    old_no_newline: bool,
    new_no_newline: bool,
}

#[async_trait]
impl Tool for PatchTool {
    fn name(&self) -> &str {
        "patch"
    }

    fn description(&self) -> &str {
        "Apply a unified diff (---/+++ headers). Prefer apply_patch for Codex patches."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["patch_text"],
            "properties": {
                "intent": super::intent_schema_property(),
                "patch_text": {
                    "type": "string",
                    "description": "Patch text."
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: PatchInput = serde_json::from_value(input)?;

        let patches = parse_patch(&params.patch_text)?;

        if patches.is_empty() {
            return Err(anyhow::anyhow!("No valid patches found in input"));
        }

        // Watch config.toml across the whole invocation so an edit that lands
        // on it is reported regardless of which patch produced it.
        let _locks = super::file_lock::lock_all(
            patches
                .iter()
                .map(|patch| ctx.resolve_path(Path::new(&patch.path))),
        )
        .await;
        let config_watch = super::config_edit_notice::ConfigEditWatch::begin();
        let mut results = Vec::new();

        // Validate every file and every hunk before any filesystem mutation.
        // Never apply a later failure after silently overwriting earlier files.
        let mut prepared = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for patch in &patches {
            let path = ctx.resolve_path(Path::new(&patch.path));
            let key = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            anyhow::ensure!(seen.insert(key), "duplicate patch path: {}", path.display());
            prepared.push(prepare_patch(patch, path).await?);
        }
        // The locks coordinate tools in this process. Recheck preimages too,
        // so edits from an external writer during preflight reject the batch.
        for file in &prepared {
            file.verify_unchanged().await?;
        }
        for (patch, file) in patches.iter().zip(prepared) {
            let (msg, diff) = file.apply(&ctx).await?;
            if diff.is_empty() {
                results.push(format!("✓ {}: {}", patch.path, msg));
            } else {
                results.push(format!("✓ {}: {}\n{}", patch.path, msg, diff));
            }
        }

        let mut body = results.join("\n\n");
        config_watch.finish(&mut body);
        Ok(ToolOutput::new(body))
    }
}

fn parse_patch(text: &str) -> Result<Vec<FilePatch>> {
    let mut patches = Vec::new();
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        // Look for --- line
        if lines[i].starts_with("---") {
            let old_file = lines[i]
                .strip_prefix("--- ")
                .unwrap_or("")
                .split('\t')
                .next()
                .unwrap_or("");

            i += 1;
            anyhow::ensure!(
                i < lines.len() && lines[i].starts_with("+++ "),
                "missing +++ file header"
            );

            let new_file = lines[i]
                .strip_prefix("+++ ")
                .unwrap_or("")
                .split('\t')
                .next()
                .unwrap_or("");

            // Determine the actual file path
            let path = if new_file == "/dev/null" {
                old_file.strip_prefix("a/").unwrap_or(old_file).to_string()
            } else {
                new_file.strip_prefix("b/").unwrap_or(new_file).to_string()
            };

            let is_new = old_file == "/dev/null";
            let is_delete = new_file == "/dev/null";
            anyhow::ensure!(
                !old_file.is_empty() && !new_file.is_empty() && !(is_new && is_delete),
                "invalid unified file headers"
            );
            anyhow::ensure!(
                is_new
                    || is_delete
                    || old_file.strip_prefix("a/").unwrap_or(old_file)
                        == new_file.strip_prefix("b/").unwrap_or(new_file),
                "unified rename headers are not supported; no files changed"
            );

            i += 1;

            // Parse hunks
            let mut hunks = Vec::new();
            while i < lines.len() && !lines[i].starts_with("---") {
                if lines[i].starts_with("@@") {
                    hunks.push(parse_hunk(&lines, &mut i)?);
                } else {
                    anyhow::ensure!(
                        !lines[i].starts_with(['+', '-', ' ']),
                        "unexpected line outside hunk: {}",
                        lines[i]
                    );
                    i += 1;
                }
            }

            if !hunks.is_empty() || is_new || is_delete {
                patches.push(FilePatch {
                    path,
                    hunks,
                    is_new,
                    is_delete,
                });
            }
        } else {
            i += 1;
        }
    }

    Ok(patches)
}

fn parse_hunk(lines: &[&str], i: &mut usize) -> Result<Hunk> {
    let header = lines[*i];
    let parts: Vec<&str> = header.split_whitespace().collect();
    anyhow::ensure!(
        parts.len() >= 4 && parts[0] == "@@" && parts[3] == "@@",
        "invalid hunk header: {header}"
    );
    let range = |text: &str, prefix: char| -> Result<(usize, usize)> {
        let text = text
            .strip_prefix(prefix)
            .ok_or_else(|| anyhow::anyhow!("invalid hunk range: {header}"))?;
        let mut numbers = text.split(',');
        let start: usize = numbers.next().unwrap_or("").parse()?;
        let count: usize = numbers.next().unwrap_or("1").parse()?;
        anyhow::ensure!(
            numbers.next().is_none() && (start > 0 || count == 0),
            "invalid hunk range: {header}"
        );
        Ok((start, count))
    };
    let (old_start, old_count) = range(parts[1], '-')?;
    let (new_start, new_count) = range(parts[2], '+')?;
    let mut hunk = Hunk {
        old_start,
        old_count,
        new_start,
        new_count,
        old_lines: Vec::new(),
        new_lines: Vec::new(),
        old_no_newline: false,
        new_no_newline: false,
    };
    *i += 1;
    let mut previous = None;
    while *i < lines.len() {
        let line = lines[*i];
        if line == "\\ No newline at end of file" {
            match previous {
                Some('-') => hunk.old_no_newline = true,
                Some('+') => hunk.new_no_newline = true,
                Some(' ') => {
                    hunk.old_no_newline = true;
                    hunk.new_no_newline = true;
                }
                _ => anyhow::bail!("newline marker without a preceding hunk line"),
            }
            *i += 1;
            previous = None;
            continue;
        }
        if hunk.old_lines.len() == old_count && hunk.new_lines.len() == new_count {
            break;
        }
        let prefix = line
            .chars()
            .next()
            .ok_or_else(|| anyhow::anyhow!("unprefixed empty hunk line: {header}"))?;
        match prefix {
            '-' => hunk.old_lines.push(line[1..].to_owned()),
            '+' => hunk.new_lines.push(line[1..].to_owned()),
            ' ' => {
                hunk.old_lines.push(line[1..].to_owned());
                hunk.new_lines.push(line[1..].to_owned());
            }
            _ => anyhow::bail!("incomplete or invalid hunk: {header}"),
        }
        anyhow::ensure!(
            hunk.old_lines.len() <= old_count && hunk.new_lines.len() <= new_count,
            "hunk line count exceeds header: {header}"
        );
        previous = Some(prefix);
        *i += 1;
    }
    anyhow::ensure!(
        hunk.old_lines.len() == old_count && hunk.new_lines.len() == new_count,
        "hunk line count does not match header: {header}"
    );
    Ok(hunk)
}

/// Strict coordinates, not fuzzy matching: an offset accepted by git apply
/// is not permission to splice some other text in the fallback applier.
fn render_patch(patch: &FilePatch, old: &str) -> Result<String> {
    // Hunk lines are split with `str::lines`, which drops a trailing `\r`.
    // Compare a CRLF file without its `\r` too, and write CRLF back out.
    let crlf = old
        .split_once('\n')
        .is_some_and(|(first, _)| first.ends_with('\r'));
    let mut lines: Vec<String> = old
        .split_terminator('\n')
        .map(|line| match line.strip_suffix('\r') {
            Some(stripped) if crlf => stripped.to_owned(),
            _ => line.to_owned(),
        })
        .collect();
    let mut previous_end = 0;
    let mut delta: i128 = 0;
    let mut trailing_newline = old.ends_with('\n');
    for hunk in &patch.hunks {
        let start = if hunk.old_count == 0 {
            hunk.old_start
        } else {
            hunk.old_start - 1
        };
        let end = start
            .checked_add(hunk.old_count)
            .ok_or_else(|| anyhow::anyhow!("hunk range overflow"))?;
        anyhow::ensure!(
            start >= previous_end && end <= lines.len(),
            "{}: hunk at old line {} is out of bounds or overlaps; no files changed",
            patch.path,
            hunk.old_start
        );
        anyhow::ensure!(
            !hunk.old_lines.is_empty() || old.is_empty(),
            "{}: context-free insertion cannot be verified; regenerate patch with context",
            patch.path
        );
        anyhow::ensure!(
            lines[start..end] == hunk.old_lines,
            "{}: context mismatch at old line {}; stale coordinates, regenerate patch against current contents; no files changed",
            patch.path,
            hunk.old_start
        );
        let new_index = if hunk.new_count == 0 {
            hunk.new_start
        } else {
            hunk.new_start - 1
        };
        anyhow::ensure!(
            new_index as i128 == start as i128 + delta,
            "{}: inconsistent new hunk coordinates",
            patch.path
        );
        if end == lines.len() && hunk.old_count > 0 {
            anyhow::ensure!(
                hunk.old_no_newline != old.ends_with('\n'),
                "{}: old EOF newline does not match contents",
                patch.path
            );
        }
        if hunk.old_no_newline {
            anyhow::ensure!(
                end == lines.len() && !old.ends_with('\n'),
                "{}: old EOF marker does not match contents",
                patch.path
            );
        }
        if hunk.new_no_newline {
            anyhow::ensure!(
                end == lines.len(),
                "{}: new EOF marker is not at end of file",
                patch.path
            );
        }
        if end == lines.len() {
            trailing_newline = !hunk.new_no_newline;
        }
        previous_end = end;
        delta += hunk.new_count as i128 - hunk.old_count as i128;
    }
    for hunk in patch.hunks.iter().rev() {
        let start = if hunk.old_count == 0 {
            hunk.old_start
        } else {
            hunk.old_start - 1
        };
        lines.splice(
            start..start + hunk.old_count,
            hunk.new_lines.iter().cloned(),
        );
    }
    let eol = if crlf { "\r\n" } else { "\n" };
    let mut content = lines.join(eol);
    if trailing_newline && !lines.is_empty() {
        content.push_str(eol);
    }
    Ok(content)
}

struct PreparedPatch {
    path: PathBuf,
    old: Option<String>,
    new: Option<String>,
    first_line: usize,
    message: String,
}

async fn prepare_patch(patch: &FilePatch, path: PathBuf) -> Result<PreparedPatch> {
    let old = match tokio::fs::read_to_string(&path).await {
        Ok(content) => Some(content),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    anyhow::ensure!(
        old.is_none() == patch.is_new,
        "{}: {}",
        path.display(),
        if patch.is_new {
            "file already exists"
        } else {
            "file does not exist"
        }
    );
    anyhow::ensure!(
        !patch.hunks.is_empty(),
        "{}: missing hunks; cannot verify contents",
        path.display()
    );
    let content = render_patch(patch, old.as_deref().unwrap_or(""))?;
    anyhow::ensure!(
        !patch.is_delete || content.is_empty(),
        "{}: deletion patch does not cover entire file",
        path.display()
    );
    Ok(PreparedPatch {
        path,
        old,
        new: if patch.is_delete { None } else { Some(content) },
        first_line: patch.hunks.first().map_or(1, |h| h.old_start.max(1)),
        message: if patch.is_delete {
            "deleted".into()
        } else if patch.is_new {
            "created".into()
        } else {
            format!("modified ({} hunks)", patch.hunks.len())
        },
    })
}

impl PreparedPatch {
    async fn verify_unchanged(&self) -> Result<()> {
        let current = match tokio::fs::read_to_string(&self.path).await {
            Ok(content) => Some(content),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        anyhow::ensure!(
            current == self.old,
            "{} changed during patch preflight; no files changed",
            self.path.display()
        );
        Ok(())
    }

    async fn apply(self, ctx: &ToolContext) -> Result<(String, String)> {
        if let Some(new) = &self.new {
            if let Some(parent) = self.path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            tokio::fs::write(&self.path, new).await?;
        } else {
            tokio::fs::remove_file(&self.path).await?;
        }
        let old = self.old.as_deref().unwrap_or("");
        let new = self.new.as_deref().unwrap_or("");
        super::edit_stats::record(ctx, old, new, false).await;
        let diff = generate_diff(old, new, self.first_line);
        Ok((self.message, diff))
    }
}

/// Generate a compact diff with line numbers (max 30 lines)
fn generate_diff(old: &str, new: &str, start_line: usize) -> String {
    let diff = TextDiff::from_lines(old, new);
    let mut output = String::new();
    let mut line_count = 0;
    const MAX_LINES: usize = 30;

    let mut old_line = start_line;
    let mut new_line = start_line;

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

#[cfg(test)]
mod tests {
    use super::*;

    fn context(dir: &Path) -> ToolContext {
        ToolContext {
            session_id: "unified-patch-safety-test".into(),
            message_id: "m".into(),
            tool_call_id: "t".into(),
            working_dir: Some(dir.to_path_buf()),
            stdin_request_tx: None,
            graceful_shutdown_signal: None,
            execution_mode: super::super::ToolExecutionMode::Direct,
        }
    }

    fn render(diff: &str, old: &str) -> Result<String> {
        let patches = parse_patch(diff)?;
        render_patch(&patches[0], old)
    }

    #[test]
    fn unified_hunks_reject_offsets_even_when_context_exists_elsewhere() {
        let diff = "--- a/f\n+++ b/f\n@@ -1,2 +1,2 @@\n anchor\n-old\n+new\n";
        assert_eq!(render(diff, "anchor\nold\n").unwrap(), "anchor\nnew\n");
        let error = render(diff, "concurrent edit\nanchor\nold\n").unwrap_err();
        assert!(error.to_string().contains("stale coordinates"), "{error}");
        assert!(render(diff, "entirely\nunrelated\n").is_err());
    }

    #[test]
    fn unified_hunks_apply_to_crlf_files_and_keep_crlf() {
        let diff = "--- a/f\n+++ b/f\n@@ -1,2 +1,3 @@\n anchor\n-old\n+new\n+added\n";
        assert_eq!(
            render(diff, "anchor\r\nold\r\n").unwrap(),
            "anchor\r\nnew\r\nadded\r\n"
        );
        // A diff taken from a CRLF file carries the `\r` on its own lines.
        let crlf_diff = diff.replace('\n', "\r\n");
        assert_eq!(
            render(&crlf_diff, "anchor\r\nold\r\n").unwrap(),
            "anchor\r\nnew\r\nadded\r\n"
        );
        let eof = "--- a/f\n+++ b/f\n@@ -1,2 +1,2 @@\n a\n-old\n\\ No newline at end of file\n+new\n\\ No newline at end of file\n";
        assert_eq!(render(eof, "a\r\nold").unwrap(), "a\r\nnew");
        let error = render(diff, "anchor\r\nother\r\n").unwrap_err();
        assert!(error.to_string().contains("stale coordinates"), "{error}");
    }

    #[tokio::test]
    async fn unified_patch_tool_edits_crlf_file_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f");
        std::fs::write(&file, "anchor\r\nold\r\n").unwrap();
        let diff = "--- a/f\n+++ b/f\n@@ -1,2 +1,2 @@\n anchor\n-old\n+new\n";
        PatchTool::new()
            .execute(json!({"patch_text": diff}), context(dir.path()))
            .await
            .unwrap();
        assert_eq!(std::fs::read_to_string(file).unwrap(), "anchor\r\nnew\r\n");
    }

    #[test]
    fn unified_hunks_reject_malformed_ranges_counts_overlap_and_unanchored_insertions() {
        for diff in [
            "--- a/f\n+++ b/f\n@@ -99 +99 @@\n-a\n+b\n",
            "--- a/f\n+++ b/f\n@@ -1,2 +1 @@\n-a\n+b\n",
            "--- a/f\n+++ b/f\n@@ -1 +1 @@\n-a\n+b\n-extra\n",
            "--- a/f\n+++ b/f\n@@ -0,1 +1 @@\n-a\n+b\n",
            "--- a/f\n+++ b/f\n@@ -1 +1 @@\n-a\n+b\n@@ -1 +1 @@\n-a\n+c\n",
            "--- a/f\n+++ b/f\n@@ -1,0 +2 @@\n+unanchored\n",
        ] {
            assert!(
                render(diff, "a\n").is_err(),
                "accepted malformed or unverifiable patch: {diff}"
            );
        }
    }

    #[test]
    fn unified_hunks_use_original_coordinates_and_preserve_eof() {
        let diff = "--- a/f\n+++ b/f\n@@ -1,2 +1,3 @@\n a\n-b\n+B\n+extra\n@@ -4 +5 @@\n-d\n+D\n";
        assert_eq!(render(diff, "a\nb\nc\nd\n").unwrap(), "a\nB\nextra\nc\nD\n");
        let eof = "--- a/f\n+++ b/f\n@@ -1 +1 @@\n-old\n\\ No newline at end of file\n+new\n\\ No newline at end of file\n";
        assert_eq!(render(eof, "old").unwrap(), "new");
        assert!(render(eof, "old\n").is_err());
        // A source line beginning with -- is not another file header.
        assert_eq!(
            render("--- a/f\n+++ b/f\n@@ -1 +1 @@\n--- old\n+new\n", "-- old\n").unwrap(),
            "new\n"
        );
    }

    #[tokio::test]
    async fn unified_apply_patch_fallback_rejects_stale_context_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f");
        let old = "prefix\nconcurrent edit\nanchor\nold\nsuffix\n";
        std::fs::write(&file, old).unwrap();
        let diff = "--- a/f\n+++ b/f\n@@ -2,3 +2,3 @@\n anchor\n-old\n+new\n suffix\n";
        // Reproduce the incident: git accepts the context at an offset, but
        // this applier must not interpret that as a valid absolute coordinate.
        let mut git = std::process::Command::new("git")
            .args(["apply", "--check", "-"])
            .current_dir(dir.path())
            .stdin(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        std::io::Write::write_all(git.stdin.as_mut().unwrap(), diff.as_bytes()).unwrap();
        drop(git.stdin.take());
        let checked = git.wait_with_output().unwrap();
        assert!(
            checked.status.success(),
            "{}",
            String::from_utf8_lossy(&checked.stderr)
        );
        let error = super::super::apply_patch::ApplyPatchTool::new()
            .execute(json!({"patch_text":diff}), context(dir.path()))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("context mismatch"));
        assert_eq!(std::fs::read_to_string(file).unwrap(), old);
    }

    #[tokio::test]
    async fn unified_batch_preflights_all_files_before_creating_modifying_or_deleting() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("first"), "first\n").unwrap();
        std::fs::write(dir.path().join("delete"), "keep me\n").unwrap();
        std::fs::write(dir.path().join("last"), "concurrent last\n").unwrap();
        let diff = "--- a/first\n+++ b/first\n@@ -1 +1 @@\n-first\n+changed\n--- /dev/null\n+++ b/new/created\n@@ -0,0 +1 @@\n+created\n--- a/delete\n+++ /dev/null\n@@ -1 +0,0 @@\n-keep me\n--- a/last\n+++ b/last\n@@ -1 +1 @@\n-wrong\n+bad\n";
        assert!(
            PatchTool::new()
                .execute(json!({"patch_text":diff}), context(dir.path()))
                .await
                .is_err()
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("first")).unwrap(),
            "first\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("delete")).unwrap(),
            "keep me\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("last")).unwrap(),
            "concurrent last\n"
        );
        assert!(
            !dir.path().join("new").exists(),
            "preflight must not even create parent directories"
        );
    }

    #[tokio::test]
    async fn unified_later_hunk_failure_and_delete_mismatch_leave_original_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f");
        std::fs::write(&file, "a\nb\n").unwrap();
        for diff in [
            "--- a/f\n+++ b/f\n@@ -1 +1 @@\n-a\n+A\n@@ -2 +2 @@\n-missing\n+B\n",
            "--- a/f\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-a\n-missing\n",
        ] {
            assert!(
                PatchTool::new()
                    .execute(json!({"patch_text":diff}), context(dir.path()))
                    .await
                    .is_err()
            );
            assert_eq!(std::fs::read_to_string(&file).unwrap(), "a\nb\n");
        }
    }

    #[tokio::test]
    async fn unified_verified_batch_applies_creation_modification_and_deletion() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f"), "old\n").unwrap();
        std::fs::write(dir.path().join("delete"), "gone\n").unwrap();
        let diff = "--- a/f\n+++ b/f\n@@ -1 +1 @@\n-old\n+new\n--- /dev/null\n+++ b/created\n@@ -0,0 +1 @@\n+created\n--- a/delete\n+++ /dev/null\n@@ -1 +0,0 @@\n-gone\n";
        PatchTool::new()
            .execute(json!({"patch_text":diff}), context(dir.path()))
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("f")).unwrap(),
            "new\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("created")).unwrap(),
            "created\n"
        );
        assert!(!dir.path().join("delete").exists());
    }
}
