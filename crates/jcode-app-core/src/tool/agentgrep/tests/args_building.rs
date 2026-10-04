use super::*;
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
    assert_eq!(
        args.path.as_deref(),
        Some(
            Path::new("/tmp/root")
                .join("src")
                .to_string_lossy()
                .as_ref()
        )
    );
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
    assert_eq!(
        args.path.as_deref(),
        Some(Path::new("/tmp/root").join(".").to_string_lossy().as_ref())
    );
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
fn build_grep_and_find_args_scope_file_field_to_exact_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(temp.path().join("src")).expect("mkdir");
    fs::write(temp.path().join("src/app.rs"), "fn auth_status() {}\n").expect("write file");

    let ctx = test_ctx(temp.path());
    let params = AgentGrepInput {
        mode: "grep".to_string(),
        query: Some("auth_status".to_string()),
        file: Some("src/app.rs".to_string()),
        terms: None,
        regex: Some(false),
        path: None,
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

    let grep = build_grep_args(&params, &ctx).unwrap();
    let find = build_find_args(&params, &ctx).unwrap();
    let expected_parent = temp.path().join("src").to_string_lossy().into_owned();
    assert_eq!(grep.path.as_deref(), Some(expected_parent.as_str()));
    assert_eq!(grep.glob.as_deref(), Some("app.rs"));
    assert_eq!(find.path.as_deref(), Some(expected_parent.as_str()));
    assert_eq!(find.glob.as_deref(), Some("app.rs"));
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
    assert_eq!(
        args.path.as_deref(),
        Some(Path::new("/tmp/root").join(".").to_string_lossy().as_ref())
    );
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
    assert_eq!(
        args.path.as_deref(),
        Some(
            Path::new("/workspace")
                .join("repo")
                .to_string_lossy()
                .as_ref()
        )
    );
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
    assert_eq!(
        args.path.as_deref(),
        Some(
            Path::new("/workspace")
                .join("repo")
                .to_string_lossy()
                .as_ref()
        )
    );
}

#[test]
fn build_outline_args_treats_file_valued_path_as_outline_target() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::write(temp.path().join("app.rs"), "fn main() {}\n").expect("write file");
    let ctx = test_ctx(temp.path());

    let params = AgentGrepInput {
        mode: "outline".to_string(),
        query: Some("fn".to_string()),
        file: None,
        terms: None,
        regex: None,
        path: Some("app.rs".to_string()),
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
    assert_eq!(
        args.file,
        temp.path().join("app.rs").display().to_string(),
        "file-valued path should become the outline target instead of joining query onto it"
    );
    assert_eq!(args.path, None);
}

#[test]
fn build_outline_args_does_not_duplicate_file_valued_path_when_file_is_also_set() {
    let temp = tempfile::tempdir().expect("tempdir");
    let relative_file = "src/tool/todo.rs";
    let absolute_file = temp.path().join(relative_file);
    fs::create_dir_all(absolute_file.parent().expect("file parent")).expect("mkdir");
    fs::write(&absolute_file, "pub fn save_todos() {}\n").expect("write file");
    let ctx = test_ctx(temp.path());

    let params = AgentGrepInput {
        mode: "outline".to_string(),
        query: None,
        file: Some(relative_file.to_string()),
        terms: None,
        regex: None,
        path: Some(relative_file.to_string()),
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
    assert_eq!(args.file, absolute_file.display().to_string());
    assert_eq!(args.path, None);
}
