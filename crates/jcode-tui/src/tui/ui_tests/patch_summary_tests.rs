#[test]
fn test_summarize_apply_patch_input_ignores_begin_marker() {
    let patch = "*** Begin Patch\n*** Update File: src/lib.rs\n@@\n-old\n+new\n*** End Patch\n";
    let summary = tools_ui::summarize_apply_patch_input(patch);
    assert_eq!(summary, "src/lib.rs (6 lines)");
}

#[test]
fn test_summarize_apply_patch_input_multiple_files() {
    let patch = "*** Begin Patch\n*** Update File: a.txt\n@@\n-a\n+b\n*** Update File: b.txt\n@@\n-c\n+d\n*** End Patch\n";
    let summary = tools_ui::summarize_apply_patch_input(patch);
    assert_eq!(summary, "2 files (10 lines)");
}

#[test]
fn test_extract_apply_patch_primary_file() {
    let patch = "*** Begin Patch\n*** Add File: new/file.rs\n+fn main() {}\n*** End Patch\n";
    let file = tools_ui::extract_apply_patch_primary_file(patch);
    assert_eq!(file.as_deref(), Some("new/file.rs"));
}

#[test]
fn test_patch_summaries_preserve_line_counts() {
    let single = "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-old\n+new\n";
    assert_eq!(
        tools_ui::summarize_unified_patch_input(single),
        "src/lib.rs (5 lines)"
    );
    let multiple = format!("{single}--- /dev/null\n+++ b/new.rs\n@@ -0,0 +1 @@\n+new\n");
    assert_eq!(
        tools_ui::summarize_unified_patch_input(&multiple),
        "2 files (9 lines)"
    );
    for patch in ["", "@@\n-old\n+new\n"] {
        let expected = format!("({} lines)", patch.lines().count());
        assert_eq!(tools_ui::summarize_unified_patch_input(patch), expected);
        assert_eq!(tools_ui::summarize_apply_patch_input(patch), expected);
    }
}
