use super::*;
use chrono::Duration;
use std::fs;

fn test_ctx(root: &Path) -> ToolContext {
    ToolContext {
        session_id: "test".to_string(),
        message_id: "test".to_string(),
        tool_call_id: "test".to_string(),
        working_dir: Some(root.to_path_buf()),
        sandbox_root: None,
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: super::super::ToolExecutionMode::Direct,
    }
}

fn test_exposure(message_index: usize, total_messages: usize) -> ExposureDescriptor {
    ExposureDescriptor {
        timestamp: Some(Utc::now()),
        message_index,
        total_messages,
        compaction_cutoff: None,
    }
}

fn grep_input(query: &str, max_regions: Option<usize>) -> AgentGrepInput {
    AgentGrepInput {
        mode: "grep".to_string(),
        query: Some(query.to_string()),
        file: None,
        terms: None,
        regex: Some(false),
        path: None,
        glob: None,
        file_type: None,
        hidden: None,
        no_ignore: None,
        max_files: None,
        max_regions,
        full_region: None,
        debug_plan: None,
        debug_score: None,
        paths_only: None,
    }
}

#[test]
fn render_compacts_huge_grep_match_lines() {
    let args = GrepArgs {
        query: "set_status_notice".to_string(),
        regex: false,
        file_type: None,
        json: false,
        paths_only: false,
        hidden: false,
        no_ignore: false,
        path: None,
        glob: None,
    };
    let line = format!(
        "{{\"output\":\"{}set_status_notice{}\"}}",
        "a".repeat(800),
        "b".repeat(800)
    );

    let compact = render::compact_rendered_match_line(&line, &args);

    assert!(compact.contains("set_status_notice"));
    assert!(compact.contains("[truncated:"), "{compact}");
    assert!(
        compact.chars().count() < 340,
        "compact output should be bounded, got {} chars: {compact}",
        compact.chars().count()
    );
}

#[test]
fn grep_max_regions_limits_rendered_match_excerpts() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::write(
        temp.path().join("a.rs"),
        "fn one() { status_notice(); }\nfn two() { status_notice(); }\nfn three() { status_notice(); }\n",
    )
    .expect("write file");

    let output = execute_linked_agentgrep(
        &grep_input("status_notice", Some(2)),
        &test_ctx(temp.path()),
        None,
    )
    .expect("agentgrep execute")
    .output;

    assert_eq!(output.matches("      - @ ").count(), 2, "{output}");
    assert!(
        output.contains("1 more matches omitted (max_regions=2)"),
        "{output}"
    );
}

#[test]
fn grep_caps_non_code_file_match_excerpts_by_default() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::write(
        temp.path().join("timeline.json"),
        (0..5)
            .map(|idx| format!("{{\"event\":\"status_notice {idx}\"}}\n"))
            .collect::<String>(),
    )
    .expect("write file");

    let output = execute_linked_agentgrep(
        &grep_input("status_notice", None),
        &test_ctx(temp.path()),
        None,
    )
    .expect("agentgrep execute")
    .output;

    assert_eq!(output.matches("      - @ ").count(), 3, "{output}");
    assert!(
        output.contains("2 more non-code matches omitted"),
        "{output}"
    );
}

#[test]
fn build_grep_args_includes_scope_flags() {
    let ctx = test_ctx(Path::new("/tmp/root"));
    let params = AgentGrepInput {
        mode: "grep".to_string(),
        query: Some("auth_status".to_string()),
        file: None,
        terms: None,
        regex: Some(true),
        path: Some("src".to_string()),
        glob: Some("src/**/*.rs".to_string()),
        file_type: Some("rs".to_string()),
        hidden: Some(true),
        no_ignore: Some(true),
        max_files: None,
        max_regions: None,
        full_region: None,
        debug_plan: None,
        debug_score: None,
        paths_only: Some(true),
    };

    let args = build_grep_args(&params, &ctx).unwrap();
    assert_eq!(args.query, "auth_status");
    assert!(args.regex);
    assert_eq!(args.file_type.as_deref(), Some("rs"));
    assert!(args.paths_only);
    assert!(args.hidden);
    assert!(args.no_ignore);
    assert_eq!(args.path.as_deref(), Some("/tmp/root/src"));
    assert_eq!(args.glob.as_deref(), Some("src/**/*.rs"));
}

#[test]
fn build_grep_args_drops_match_all_glob() {
    let ctx = test_ctx(Path::new("/tmp/root"));
    let params = AgentGrepInput {
        mode: "grep".to_string(),
        query: Some("agentgrep".to_string()),
        file: None,
        terms: None,
        regex: Some(false),
        path: Some(".".to_string()),
        glob: Some("**/*".to_string()),
        file_type: Some("rs".to_string()),
        hidden: None,
        no_ignore: None,
        max_files: None,
        max_regions: None,
        full_region: None,
        debug_plan: None,
        debug_score: None,
        paths_only: None,
    };

    let args = build_grep_args(&params, &ctx).unwrap();
    assert_eq!(args.query, "agentgrep");
    assert_eq!(args.file_type.as_deref(), Some("rs"));
    assert_eq!(args.path.as_deref(), Some("/tmp/root/."));
    assert_eq!(args.glob, None);
}

#[test]
fn build_grep_args_scopes_file_path_to_parent_and_exact_glob() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(temp.path().join("src")).expect("mkdir");
    fs::write(temp.path().join("src/app.rs"), "fn auth_status() {}\n").expect("write file");

    let ctx = test_ctx(temp.path());
    let params = AgentGrepInput {
        mode: "grep".to_string(),
        query: Some("auth_status".to_string()),
        file: None,
        terms: None,
        regex: Some(false),
        path: Some("src/app.rs".to_string()),
        glob: Some("**/*.rs".to_string()),
        file_type: Some("rs".to_string()),
        hidden: None,
        no_ignore: None,
        max_files: None,
        max_regions: None,
        full_region: None,
        debug_plan: None,
        debug_score: None,
        paths_only: None,
    };

    let args = build_grep_args(&params, &ctx).unwrap();
    assert_eq!(
        args.path.as_deref(),
        Some(temp.path().join("src").to_string_lossy().as_ref())
    );
    assert_eq!(args.glob.as_deref(), Some("app.rs"));
}

#[test]
fn build_find_args_allows_glob_only_search() {
    let ctx = test_ctx(Path::new("/tmp/root"));
    let params = AgentGrepInput {
        mode: "find".to_string(),
        query: None,
        file: None,
        terms: None,
        regex: None,
        path: Some(".".to_string()),
        glob: Some("**/*release*".to_string()),
        file_type: None,
        hidden: None,
        no_ignore: None,
        max_files: Some(25),
        max_regions: None,
        full_region: None,
        debug_plan: None,
        debug_score: None,
        paths_only: Some(true),
    };

    let args = build_find_args(&params, &ctx).expect("glob-only find should be valid");
    assert!(args.query_parts.is_empty());
    assert_eq!(args.path.as_deref(), Some("/tmp/root/."));
    assert_eq!(args.glob.as_deref(), Some("**/*release*"));
    assert_eq!(args.max_files, 25);
    assert!(args.paths_only);
}

#[test]
fn build_find_args_still_rejects_unscoped_empty_query() {
    let ctx = test_ctx(Path::new("/tmp/root"));
    let params = AgentGrepInput {
        mode: "find".to_string(),
        query: None,
        file: None,
        terms: None,
        regex: None,
        path: None,
        glob: None,
        file_type: None,
        hidden: None,
        no_ignore: None,
        max_files: None,
        max_regions: None,
        full_region: None,
        debug_plan: None,
        debug_score: None,
        paths_only: None,
    };

    let error = build_find_args(&params, &ctx).unwrap_err();
    assert_eq!(
        error.to_string(),
        "agentgrep find requires 'query' unless path, glob, or type narrows the search"
    );
}

#[test]
fn build_smart_args_uses_terms() {
    let ctx = test_ctx(Path::new("/workspace"));
    let params = AgentGrepInput {
        mode: "smart".to_string(),
        query: None,
        file: None,
        terms: Some(vec![
            "subject:auth_status".to_string(),
            "relation:rendered".to_string(),
            "path:src/tui".to_string(),
        ]),
        regex: None,
        path: Some("repo".to_string()),
        glob: None,
        file_type: Some("rs".to_string()),
        hidden: None,
        no_ignore: None,
        max_files: Some(3),
        max_regions: Some(4),
        full_region: Some("auto".to_string()),
        debug_plan: Some(true),
        debug_score: Some(true),
        paths_only: None,
    };

    let (args, query) = build_smart_args_and_query(&params, &ctx, None).unwrap();
    assert_eq!(
        args.terms,
        vec!["subject:auth_status", "relation:rendered", "path:src/tui"]
    );
    assert_eq!(args.max_files, 3);
    assert_eq!(args.max_regions, 4);
    assert!(matches!(args.full_region, FullRegionMode::Auto));
    assert!(args.debug_plan);
    assert!(args.debug_score);
    assert_eq!(args.file_type.as_deref(), Some("rs"));
    assert_eq!(args.path.as_deref(), Some("/workspace/repo"));
    assert_eq!(query.subject, "auth_status");
    assert_eq!(query.relation.as_str(), "rendered");
    assert_eq!(query.path_hint.as_deref(), Some("src/tui"));
}

#[test]
fn build_smart_args_falls_back_to_query_terms() {
    let ctx = test_ctx(Path::new("/workspace"));
    let params = AgentGrepInput {
        mode: "smart".to_string(),
        query: Some(
            "subject:auth_status relation:rendered path:src/tui support:current".to_string(),
        ),
        file: None,
        terms: None,
        regex: None,
        path: Some("repo".to_string()),
        glob: None,
        file_type: Some("rs".to_string()),
        hidden: None,
        no_ignore: None,
        max_files: Some(3),
        max_regions: Some(4),
        full_region: Some("auto".to_string()),
        debug_plan: Some(true),
        debug_score: Some(true),
        paths_only: None,
    };

    let (args, _query) = build_smart_args_and_query(&params, &ctx, None).unwrap();
    assert_eq!(
        args.terms,
        vec![
            "subject:auth_status",
            "relation:rendered",
            "path:src/tui",
            "support:current"
        ]
    );
}

#[test]
fn build_args_for_trace_still_requires_terms() {
    let params = AgentGrepInput {
        mode: "trace".to_string(),
        query: Some("subject:auth_status relation:rendered".to_string()),
        file: None,
        terms: None,
        regex: None,
        path: None,
        glob: None,
        file_type: None,
        hidden: None,
        no_ignore: None,
        max_files: None,
        max_regions: None,
        full_region: None,
        debug_plan: None,
        debug_score: None,
        paths_only: None,
    };

    let error = trace_or_smart_terms_owned(&params).unwrap_err();
    assert_eq!(
        error.to_string(),
        "agentgrep trace requires non-empty 'terms'"
    );
}

#[test]
fn schema_only_advertises_common_public_fields() {
    let schema = AgentGrepTool::new().parameters_schema();
    let props = schema["properties"]
        .as_object()
        .expect("agentgrep schema should have properties");
    let required = schema["required"].as_array().cloned().unwrap_or_default();
    let mode_enum = props["mode"]["enum"]
        .as_array()
        .expect("agentgrep mode should expose enum values");

    assert!(
        !required.contains(&json!("mode")),
        "agentgrep mode should be optional because omitted mode defaults to grep"
    );
    assert!(props.contains_key("mode"));
    assert!(props.contains_key("query"));
    assert!(props.contains_key("file"));
    assert!(props.contains_key("terms"));
    assert!(props.contains_key("regex"));
    assert!(props.contains_key("path"));
    assert!(props.contains_key("glob"));
    assert!(props.contains_key("type"));
    assert!(props.contains_key("max_files"));
    assert!(props.contains_key("max_regions"));
    assert!(props.contains_key("paths_only"));
    assert_eq!(
        mode_enum,
        &vec![
            json!("grep"),
            json!("find"),
            json!("outline"),
            json!("trace")
        ]
    );
    assert!(!props.contains_key("hidden"));
    assert!(!props.contains_key("no_ignore"));
    assert!(!props.contains_key("full_region"));
    assert!(!props.contains_key("debug_plan"));
    assert!(!props.contains_key("debug_score"));
}

#[test]
fn input_defaults_missing_mode_to_grep() {
    let params: AgentGrepInput = serde_json::from_value(json!({
        "query": "auth_status",
        "path": "src"
    }))
    .expect("agentgrep input without mode should deserialize");

    assert_eq!(params.mode, "grep");
    assert_eq!(params.query.as_deref(), Some("auth_status"));
}

#[test]
fn build_outline_args_accepts_file_field() {
    let ctx = test_ctx(Path::new("/workspace"));
    let params = AgentGrepInput {
        mode: "outline".to_string(),
        query: None,
        file: Some("src/tool/agentgrep.rs".to_string()),
        terms: None,
        regex: None,
        path: Some("repo".to_string()),
        glob: None,
        file_type: None,
        hidden: None,
        no_ignore: None,
        max_files: None,
        max_regions: None,
        full_region: None,
        debug_plan: None,
        debug_score: None,
        paths_only: None,
    };

    let args = build_outline_args(&params, &ctx, None).unwrap();
    assert_eq!(args.file, "src/tool/agentgrep.rs");
    assert_eq!(args.path.as_deref(), Some("/workspace/repo"));
}

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
    assert!(output.output.contains("src/app.rs"));
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
    assert!(output.output.contains("app.rs"));
    assert!(!output.output.contains("src/other.rs"));
    assert!(!output.output.contains("other.rs"));
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
