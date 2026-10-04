use super::*;
#[test]
fn output_mentions_accepts_both_spellings_and_rejects_other_files() {
    // Exactly what rg emits on Windows, including the leading `.\`.
    let win = ".\\src\\app.rs";
    // Exactly what rg emits on POSIX.
    let posix = "./src/app.rs";
    for haystack in [win, posix] {
        assert!(output_mentions(haystack, "src/app.rs"), "{haystack}");
        assert!(output_mentions(haystack, "app.rs"), "{haystack}");
        // A different basename must not match, in either spelling.
        assert!(!output_mentions(haystack, "src/other.rs"), "{haystack}");
        assert!(!output_mentions(haystack, "other.rs"), "{haystack}");
        assert!(!output_mentions(haystack, "sibling.rs"), "{haystack}");
    }
    // Regression guard for the mistake this helper was written to avoid:
    // `Path::new("src/app.rs").to_string_lossy()` keeps the `/` that was typed
    // into the literal, so deriving a Windows spelling from it yields a string
    // that cannot occur in rg's output on Windows.
    assert_ne!(
        rendered_rel_spellings("src/app.rs")[1],
        Path::new("src/app.rs").to_string_lossy().to_string(),
        "the Windows spelling must not be derived from to_string_lossy"
    );
}

#[test]
fn exact_file_filter_matches_the_forms_rg_actually_emits() {
    // `exact_search_file_path` hands the filter a bare file name, while rg
    // prints a path relative to the search root. These are the spellings rg
    // produced for a search rooted at the parent of `src/app.rs`.
    for emitted in ["app.rs", "./src/app.rs", ".\\src\\app.rs", "src/app.rs"] {
        assert!(
            is_exact_search_file(emitted, "app.rs"),
            "{emitted} should match a bare exact_file"
        );
    }
    // The same holds when the caller spells the file out in full.
    for emitted in ["./src/app.rs", ".\\src\\app.rs", "src/app.rs"] {
        assert!(
            is_exact_search_file(emitted, "src/app.rs"),
            "{emitted} should match a full exact_file"
        );
    }
    // Siblings must still be excluded, at a component boundary, so a
    // substring collision cannot slip through.
    for emitted in [
        "./src/other.rs",
        ".\\src\\other.rs",
        "src/other.rs",
        "./src/other_app.rs",
        ".\\src\\other_app.rs",
        "src/my_app.rs",
    ] {
        assert!(
            !is_exact_search_file(emitted, "app.rs"),
            "{emitted} must not match app.rs"
        );
    }
    // Scoping to one file does not mean "any file with this basename". That is
    // guaranteed upstream rather than by this comparison: `scope_root_for`
    // roots a file-valued search at the exact file's *parent*, so rg cannot
    // report a same-named file from another directory. The component-boundary
    // check above is what keeps `other_app.rs` and `my_app.rs` out.
    assert!(!is_exact_search_file("./src/my_app.rs", "app.rs"));
    assert!(!is_exact_search_file("./src/other_app.rs", "app.rs"));
    // Spelling the file out in full still works, including with rg's prefix.
    assert!(is_exact_search_file("./src/app.rs", "./src/app.rs"));
    assert!(is_exact_search_file(".\\src\\app.rs", "./src/app.rs"));
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
        no_follow: false,
        path: None,
        glob: None,
    };
    let line = format!(
        "{{\"output\":\"{}set_status_notice{}\"}}",
        "a".repeat(800),
        "b".repeat(800)
    );

    let compact = ::agentgrep::render::compact_rendered_match_line(&line, &args);

    assert!(compact.contains("set_status_notice"));
    assert!(compact.contains("[truncated:"), "{compact}");
    assert!(
        compact.chars().count() < 340,
        "compact output should be bounded, got {} chars: {compact}",
        compact.chars().count()
    );
}

#[test]
fn render_compacts_huge_trace_region_body_lines() {
    let line = format!("function handleAuth(){{{}}}", "var x=1;".repeat(2000));

    let compact = ::agentgrep::render::compact_region_body_line(&line);

    assert!(compact.contains("[truncated:"), "{compact}");
    assert!(
        compact.chars().count() < 340,
        "compact region body line should be bounded, got {} chars",
        compact.chars().count()
    );

    let short = "fn small() {}";
    assert_eq!(::agentgrep::render::compact_region_body_line(short), short);
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
