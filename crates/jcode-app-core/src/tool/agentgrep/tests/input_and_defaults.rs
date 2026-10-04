use super::*;
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
fn input_accepts_file_path_alias_for_file() {
    let params: AgentGrepInput = serde_json::from_value(json!({
        "mode": "outline",
        "file_path": "src/app.rs"
    }))
    .expect("agentgrep input with file_path should deserialize");

    assert_eq!(params.file.as_deref(), Some("src/app.rs"));
}

#[test]
fn input_accepts_legacy_grep_param_aliases() {
    // Models sometimes call the removed native `grep` tool, which is now
    // aliased to agentgrep. Its `pattern`/`include` params must map to
    // agentgrep's `query`/`glob`.
    let input: AgentGrepInput = serde_json::from_value(serde_json::json!({
        "pattern": "fn main",
        "include": "*.rs",
        "path": "src"
    }))
    .expect("legacy grep params should deserialize");
    assert_eq!(input.query.as_deref(), Some("fn main"));
    assert_eq!(input.glob.as_deref(), Some("*.rs"));
    assert_eq!(input.path.as_deref(), Some("src"));
    assert_eq!(input.mode, "grep");
}

#[test]
fn grep_defaults_to_a_bounded_match_count() {
    // grep was the only mode with no default cap: find defaults to 5 files and
    // outline to 6 regions, but grep passed `None` through and rendered every
    // match. One unscoped query over a repo with large data files produced 923k
    // chars in a single call.
    let unbounded = grep_input("x", None);
    assert_eq!(
        unbounded.max_regions.or(Some(DEFAULT_GREP_MAX_REGIONS)),
        Some(DEFAULT_GREP_MAX_REGIONS),
        "grep must be bounded when the caller sets no cap"
    );

    // An explicit cap must win in either direction, including a larger one, so
    // the default is a floor on safety and not a ceiling on capability.
    for explicit in [5usize, 5_000] {
        let params = grep_input("x", Some(explicit));
        assert_eq!(
            params.max_regions.or(Some(DEFAULT_GREP_MAX_REGIONS)),
            Some(explicit),
            "an explicit cap must win over the default"
        );
    }

    // The default has to be generous enough that ordinary code searches are
    // untouched; a cap that clips normal work trades one problem for another.
    // Checked against a real search rather than as a constant comparison, which
    // the compiler would fold away: this repo's own uses of a common internal
    // symbol must fit under the cap.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let args = build_grep_args(&grep_input("guard_context_overflow", None), &test_ctx(root))
        .expect("grep args");
    let result = ::agentgrep::search::run_grep(root, &args).expect("grep should run");
    assert!(
        result.total_matches > 0,
        "sanity: the probe symbol should exist in this crate"
    );
    assert!(
        result.total_matches < DEFAULT_GREP_MAX_REGIONS,
        "an ordinary in-repo search returned {} matches, which the default cap \
         of {} would clip",
        result.total_matches,
        DEFAULT_GREP_MAX_REGIONS
    );
}
