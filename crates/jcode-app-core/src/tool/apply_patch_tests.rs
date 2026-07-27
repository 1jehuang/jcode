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
    let patch = "*** Begin Patch\n*** Update File: test.py\n@@\n foo\n-bar\n+baz\n*** End Patch";
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

// Issue #604: apply_patch can delete by absolute path, so it is a second route
// to the same damage the bash gate blocks. `ToolContext::resolve_path` passes
// absolute paths through unchanged, so nothing else bounds it.

#[tokio::test]
async fn apply_patch_refuses_to_delete_a_protected_path() {
    let temp = tempfile::tempdir().expect("temp home");
    let home = temp.path().to_path_buf();
    let previous = std::env::var("HOME").ok();
    // SAFETY: single-threaded test setup; restored below.
    unsafe { std::env::set_var("HOME", &home) };

    // A credential file inside the protected ~/.ssh directory.
    let ssh = home.join(".ssh");
    std::fs::create_dir_all(&ssh).expect("ssh dir");
    let key = ssh.join("id_ed25519");
    std::fs::write(&key, "PRIVATE KEY").expect("key");

    let patch = format!(
        "*** Begin Patch\n*** Delete File: {}\n*** End Patch",
        key.display()
    );
    let result = ApplyPatchTool
        .execute(
            serde_json::json!({ "patch_text": patch }),
            ToolContext {
                session_id: "patch-gate".to_string(),
                message_id: "m".to_string(),
                tool_call_id: "c".to_string(),
                working_dir: Some(std::path::PathBuf::from("/tmp")),
                stdin_request_tx: None,
                graceful_shutdown_signal: None,
                execution_mode: crate::tool::ToolExecutionMode::Direct,
            },
        )
        .await;

    match previous {
        Some(value) => unsafe { std::env::set_var("HOME", value) },
        None => unsafe { std::env::remove_var("HOME") },
    }

    let output = result.expect("the tool should report, not error out");
    assert!(
        format!("{output:?}").contains("refused"),
        "expected a refusal in the output: {output:?}"
    );
    assert!(
        key.exists(),
        "apply_patch must not delete a protected credential file"
    );
}

#[tokio::test]
async fn apply_patch_still_deletes_ordinary_files() {
    // The guard must not break the tool's normal job.
    let temp = tempfile::tempdir().expect("temp dir");
    let target = temp.path().join("obsolete.rs");
    std::fs::write(&target, "fn old() {}\n").expect("file");

    let patch = format!(
        "*** Begin Patch\n*** Delete File: {}\n*** End Patch",
        target.display()
    );
    ApplyPatchTool
        .execute(
            serde_json::json!({ "patch_text": patch }),
            ToolContext {
                session_id: "patch-ok".to_string(),
                message_id: "m".to_string(),
                tool_call_id: "c".to_string(),
                working_dir: Some(temp.path().to_path_buf()),
                stdin_request_tx: None,
                graceful_shutdown_signal: None,
                execution_mode: crate::tool::ToolExecutionMode::Direct,
            },
        )
        .await
        .expect("ordinary delete should succeed");

    assert!(!target.exists(), "an ordinary file should still be deleted");
}
