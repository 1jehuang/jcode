use super::*;
#[test]
fn trace_output_collects_symbols_regions_and_focus() {
    let ctx = test_ctx(Path::new("/repo"));
    let mut context = AgentGrepHarnessContext {
        version: 1,
        ..Default::default()
    };
    let mut focus = HashSet::new();
    let mut file_mtime_cache = HashMap::new();
    let content = r#"
query parameters:
  subject: auth_status
  relation: rendered

top results: 1 files, 1 regions
best answer likely in src/tui/app.rs

1. src/tui/app.rs
   role: ui
   structure:
     - function render_status_bar @ 9002-9017 (16 lines)
     - function draw_header @ 9035-9056 (22 lines)
   regions:
     - render_status_bar @ 9002-9017 (16 lines)
       kind: render-site
       full region:
         fn render_status_bar(&self, ui: &mut Ui) {
             let status = auth_status();
         }
       why:
         - exact subject match
"#;

    collect_trace_exposure(
        content,
        Path::new("/repo"),
        &ctx,
        &mut context,
        &mut focus,
        test_exposure(8, 10),
        &mut file_mtime_cache,
    );

    assert!(focus.contains("src/tui/app.rs"));
    assert!(
        context
            .known_files
            .iter()
            .any(|entry| entry.path == "src/tui/app.rs")
    );
    assert!(
        context
            .known_symbols
            .iter()
            .any(|entry| { entry.path == "src/tui/app.rs" && entry.symbol == "render_status_bar" })
    );
    assert!(context.known_regions.iter().any(|entry| {
        entry.path == "src/tui/app.rs" && entry.start_line == 9002 && entry.end_line == 9017
    }));
}

#[test]
fn bash_exposure_collects_file_and_line_hits() {
    let ctx = test_ctx(Path::new("/repo"));
    let mut context = AgentGrepHarnessContext {
        version: 1,
        ..Default::default()
    };
    let mut focus = HashSet::new();
    let mut file_mtime_cache = HashMap::new();
    let tool = ToolCall {
        id: "tool-1".to_string(),
        name: "bash".to_string(),
        input: json!({
            "command": "cat src/tool/lsp.rs && rg -n auth_status src/tool/lsp.rs"
        }),
        intent: None,
        thought_signature: None,
    };
    let content = "src/tool/lsp.rs:42:let status = auth_status();\n";

    collect_bash_exposure(
        &tool,
        content,
        Path::new("/repo"),
        &ctx,
        &mut context,
        &mut focus,
        test_exposure(9, 10),
        &mut file_mtime_cache,
    );

    assert!(focus.contains("src/tool/lsp.rs"));
    assert!(
        context
            .known_files
            .iter()
            .any(|entry| entry.path == "src/tool/lsp.rs")
    );
    assert!(context.known_regions.iter().any(|entry| {
        entry.path == "src/tool/lsp.rs" && entry.start_line == 42 && entry.end_line == 42
    }));
}

#[test]
fn tuning_penalizes_compacted_history() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ctx = test_ctx(temp.path());
    let file_path = temp.path().join("src/foo.rs");
    fs::create_dir_all(file_path.parent().expect("parent")).expect("mkdir");
    fs::write(&file_path, "fn foo() {}\n").expect("write file");

    let known = AgentGrepKnownFile {
        path: "src/foo.rs".to_string(),
        structure_confidence: 0.9,
        body_confidence: 0.8,
        current_version_confidence: 0.9,
        prune_confidence: 0.8,
        source_strength: "full_file",
        reasons: vec!["test"],
    };
    let mut cache = HashMap::new();
    let tuned = tune_known_file(
        known,
        ExposureDescriptor {
            timestamp: Some(Utc::now()),
            message_index: 1,
            total_messages: 10,
            compaction_cutoff: Some(8),
        },
        temp.path(),
        &ctx,
        &mut cache,
    );

    assert!(tuned.body_confidence < 0.5);
    assert!(tuned.prune_confidence < 0.5);
    assert!(tuned.reasons.contains(&"compacted_history"));
}

#[test]
fn tuning_detects_file_changed_since_seen() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ctx = test_ctx(temp.path());
    let file_path = temp.path().join("src/bar.rs");
    fs::create_dir_all(file_path.parent().expect("parent")).expect("mkdir");
    fs::write(&file_path, "fn bar() {}\n").expect("write file");

    let mut cache = HashMap::new();
    let tuned = tune_known_region(
        AgentGrepKnownRegion {
            path: "src/bar.rs".to_string(),
            start_line: 1,
            end_line: 1,
            body_confidence: 0.9,
            current_version_confidence: 0.9,
            prune_confidence: 0.8,
            source_strength: "full_region",
            reasons: vec!["test"],
        },
        ExposureDescriptor {
            timestamp: Some(Utc::now() - Duration::hours(1)),
            message_index: 9,
            total_messages: 10,
            compaction_cutoff: None,
        },
        temp.path(),
        &ctx,
        &mut cache,
    );

    assert!(tuned.current_version_confidence < 0.6);
    assert!(tuned.reasons.contains(&"file_changed_since_seen"));
}
