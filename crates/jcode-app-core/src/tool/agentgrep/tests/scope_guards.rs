use super::*;
#[test]
fn agentgrep_rejects_missing_session_cwd_instead_of_using_process_cwd() {
    let mut ctx = test_ctx(Path::new("/unused"));
    ctx.working_dir = None;

    let error = run_agentgrep_blocking(&grep_input("needle", None), &ctx)
        .expect_err("workspace search without a session cwd must fail");

    assert!(error.to_string().contains("session working directory"));
}

/// A file-scope search must resolve to the searched file's real parent, never
/// to "." (which the daemon would resolve against its own cwd, i.e. another
/// project's tree).
///
/// This test binds to the reachable behaviour. The `parent()` fallback that
/// used to sit above this is *unreachable*: every path whose `parent()` is
/// `None` is either a directory or does not exist, and this branch requires
/// `is_file()`. Probing every parentless shape confirms it
/// (`/`, `C:\\`, `C:/`, `C:`, and the empty path are all `is_dir` or
/// nonexistent). So the fallback could never produce a wrong root on its own,
/// and a test that pretended otherwise passed with the fallback restored.
///
/// What remains load-bearing, and what this asserts, is that a file scope
/// yields that file's directory. A regression to "." would show up here as a
/// path that is not the parent.
#[test]
fn file_scope_resolves_to_the_real_parent_never_the_cwd() {
    let temp = tempfile::tempdir().expect("tempdir");
    let nested = temp.path().join("src");
    fs::create_dir_all(&nested).expect("mkdir");
    fs::write(nested.join("app.rs"), "fn auth_status() {}\n").expect("write file");

    let ctx = test_ctx(temp.path());
    let params = AgentGrepInput {
        mode: "grep".to_string(),
        query: Some("auth_status".to_string()),
        file: Some("src/app.rs".to_string()),
        terms: None,
        regex: Some(false),
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

    let args = build_grep_args(&params, &ctx).expect("a real file must scope");
    let root = args.path.expect("a file scope always has a root");
    assert_eq!(
        root,
        nested.to_string_lossy().into_owned(),
        "file scope must use the file's own directory"
    );
    assert_ne!(
        root, ".",
        "the cwd is never the right answer for a file scope"
    );
    assert_eq!(
        args.glob.as_deref(),
        Some("app.rs"),
        "and the glob is the file name"
    );
}

/// `scope_root_for` is the seam that makes the parentless case testable.
///
/// `resolved_search_scope` only calls it after `is_file()`, and every path
/// whose `parent()` is `None` is a directory or does not exist, so the `None`
/// arm is unreachable through `build_grep_args`. Two earlier tests tried to
/// reach it that way and both passed with the old
/// `unwrap_or_else(|| Path::new("."))` restored, so neither proved anything.
///
/// The positive control matters as much as the negative case: with no working
/// directory the helper must still return the file's real parent, so a
/// regression to "." is caught by the assertion below and not only here.
#[test]
fn scope_root_for_parentless_path_errors_and_never_returns_the_cwd() {
    // Empty path: `Path::new("").parent()` is None. The old code turned this
    // into ".", which the daemon would resolve against its own cwd.
    let error = scope_root_for(Path::new(""))
        .expect_err("a path with no parent must not be rooted at the cwd");
    assert!(
        error.to_string().contains("no parent directory"),
        "the error should say the path has no parent, got {error:?}"
    );

    // Positive control: an ordinary file path returns its real directory.
    let nested = PathBuf::from("src").join("app.rs");
    assert_eq!(
        scope_root_for(&nested).expect("a normal file path has a parent"),
        PathBuf::from("src").to_string_lossy().into_owned()
    );
    // And it must never be "." for that same path, which is the regression the
    // old fallback would have introduced had it been reachable.
    assert_ne!(
        scope_root_for(&nested).expect("a normal file path has a parent"),
        "."
    );
}
