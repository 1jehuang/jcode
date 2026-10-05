use super::*;
#[tokio::test]
async fn execute_runs_linked_grep() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(temp.path().join("src")).expect("mkdir");
    fs::write(
        temp.path().join("src/app.rs"),
        "pub fn auth_status() {}\nfn render_status_bar() {}\n",
    )
    .expect("write file");

    let tool = AgentGrepTool::new();
    let ctx = test_ctx(temp.path());
    let output = tool
        .execute(
            json!({"mode": "grep", "query": "auth_status", "path": ".", "type": "rs"}),
            ctx,
        )
        .await
        .expect("tool output");
    assert!(output.output.contains("query: auth_status"));
    assert!(
        output_mentions(&output.output, "src/app.rs"),
        "expected a match under src/app.rs, got:\n{}",
        output.output
    );
    assert!(output.output.contains("@ 1 pub fn auth_status() {}"));
}

#[tokio::test]
async fn execute_runs_linked_grep_when_mode_is_omitted() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(temp.path().join("src")).expect("mkdir");
    fs::write(temp.path().join("src/app.rs"), "pub fn auth_status() {}\n").expect("write file");

    let tool = AgentGrepTool::new();
    let ctx = test_ctx(temp.path());
    let output = tool
        .execute(json!({"query": "auth_status", "path": "src"}), ctx)
        .await
        .expect("tool output");

    assert!(output.output.contains("query: auth_status"));
    assert!(output.output.contains("app.rs"));
}

#[tokio::test]
async fn execute_grep_file_field_does_not_scan_sibling_files() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(temp.path().join("src")).expect("mkdir");
    fs::write(temp.path().join("src/app.rs"), "fn target() {}\n").expect("write target");
    fs::write(
        temp.path().join("src/sibling.rs"),
        "fn target() { panic!(\"sibling marker\") }\n",
    )
    .expect("write sibling");

    let output = AgentGrepTool::new()
        .execute(
            json!({"mode": "grep", "query": "target", "file": "src/app.rs"}),
            test_ctx(temp.path()),
        )
        .await
        .expect("file-scoped grep");

    assert!(output_mentions(&output.output, "app.rs"));
    assert!(!output_mentions(&output.output, "sibling.rs"));
    assert!(!output.output.contains("sibling marker"));
}

#[tokio::test]
async fn execute_runs_linked_grep_when_path_points_to_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(temp.path().join("src")).expect("mkdir");
    fs::write(
        temp.path().join("src/app.rs"),
        "pub fn auth_status() {}\nfn render_status_bar() {}\n",
    )
    .expect("write target file");
    fs::write(
        temp.path().join("src/other.rs"),
        "pub fn auth_status() {}\nfn render_other() {}\n",
    )
    .expect("write sibling file");

    let tool = AgentGrepTool::new();
    let ctx = test_ctx(temp.path());
    let output = tool
        .execute(
            json!({
                "mode": "grep",
                "query": "auth_status",
                "path": "src/app.rs",
                "glob": "**/*.rs",
                "type": "rs"
            }),
            ctx,
        )
        .await
        .expect("tool output for exact-file path");
    assert!(
        output_mentions(&output.output, "app.rs"),
        "expected app.rs, got:\n{}",
        output.output
    );
    assert!(!output_mentions(&output.output, "src/other.rs"));
    assert!(!output_mentions(&output.output, "other.rs"));
}

#[tokio::test]
async fn execute_smart_accepts_query_fallback() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(temp.path().join("src/tool")).expect("mkdir");
    fs::write(
        temp.path().join("src/tool/lsp.rs"),
        r#"pub struct LspTool;
impl LspTool {}
fn execute() { println!("implementation"); }
"#,
    )
    .expect("write file");

    let tool = AgentGrepTool::new();
    let ctx = test_ctx(temp.path());
    let output = tool
        .execute(
            json!({
                "mode": "smart",
                "query": "subject:lsp relation:implementation path:src/tool",
                "path": ".",
                "max_files": 2,
                "max_regions": 3,
                "debug_plan": true
            }),
            ctx,
        )
        .await
        .expect("agentgrep execution");
    assert!(output.output.contains("debug plan:"));
    assert!(output.output.contains("subject: lsp"));
    assert!(output.output.contains("relation: implementation"));
}
