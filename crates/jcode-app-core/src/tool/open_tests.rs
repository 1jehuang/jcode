use super::*;

fn make_ctx() -> ToolContext {
    ToolContext {
        session_id: "test-session".to_string(),
        message_id: "test-msg".to_string(),
        tool_call_id: "test-call".to_string(),
        working_dir: Some(std::env::temp_dir()),
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: crate::tool::ToolExecutionMode::Direct,
    }
}

#[test]
fn parse_target_accepts_supported_schemes() {
    let parsed = parse_target("https://example.com/docs").unwrap();
    assert!(matches!(parsed, Some(ParsedTarget::Url(url)) if url == "https://example.com/docs"));

    let parsed_mailto = parse_target("mailto:test@example.com").unwrap();
    assert!(
        matches!(parsed_mailto, Some(ParsedTarget::Url(url)) if url == "mailto:test@example.com")
    );
}

#[test]
fn parse_target_rejects_custom_scheme() {
    let err = parse_target("javascript:alert(1)").unwrap_err();
    assert!(
        err.to_string()
            .contains("Unsupported URL scheme: javascript")
    );
}

#[test]
fn resolve_target_treats_file_url_as_local_path() {
    let ctx = make_ctx();
    let temp_file = std::env::temp_dir().join("kcode-open-tool-file-url.txt");
    std::fs::write(&temp_file, "test").unwrap();

    let file_url = url::Url::from_file_path(&temp_file).unwrap().to_string();
    let resolved = resolve_target(&file_url, &ctx).unwrap();

    assert!(matches!(
        resolved,
        ResolvedTarget::Local { path, kind: LocalTargetKind::File }
        if path == temp_file
    ));

    let _ = std::fs::remove_file(&temp_file);
}

#[test]
fn resolve_target_rejects_missing_local_path() {
    let ctx = make_ctx();
    let err = resolve_target("./definitely-missing-jcode-open-target", &ctx).unwrap_err();
    assert!(err.to_string().contains("Target path does not exist"));
}

#[tokio::test]
async fn execute_rejects_reveal_for_url() {
    let tool = OpenTool::new();
    let err = tool
        .execute(
            json!({"action": "reveal", "target": "https://example.com"}),
            make_ctx(),
        )
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("The reveal action only supports local filesystem paths")
    );
}

#[tokio::test]
async fn execute_rejects_removed_mode_parameter() {
    let tool = OpenTool::new();
    let err = tool
        .execute(
            json!({"mode": "reveal", "target": "https://example.com"}),
            make_ctx(),
        )
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("open.mode was removed"),
        "err={err}"
    );
}

#[test]
fn expand_home_handles_plain_non_tilde_paths() {
    let path = expand_home("docs/spec.pdf").unwrap();
    assert_eq!(path, PathBuf::from("docs/spec.pdf"));
}

fn window(id: u64, app_id: &str, ts: Option<(u64, u32)>) -> NiriWindow {
    NiriWindow {
        id,
        app_id: Some(app_id.to_string()),
        focus_timestamp: ts.map(|(secs, nanos)| NiriTimestamp { secs, nanos }),
    }
}

#[test]
fn normalize_desktop_entry_strips_suffix_and_adds_stem() {
    let stems = normalize_desktop_entry_to_stems("firefox.desktop");
    assert_eq!(stems, vec!["firefox".to_string()]);

    let stems = normalize_desktop_entry_to_stems("org.mozilla.firefox.desktop");
    assert_eq!(
        stems,
        vec!["org.mozilla.firefox".to_string(), "firefox".to_string()]
    );
}

#[test]
fn app_id_matches_is_case_insensitive_and_guards_short_ids() {
    let stems = vec!["firefox".to_string()];
    assert!(app_id_matches(Some("firefox"), &stems));
    assert!(app_id_matches(Some("Firefox"), &stems));
    assert!(!app_id_matches(Some("kitty"), &stems));
    assert!(!app_id_matches(None, &stems));
    // Very short ids should never match to avoid accidental substring hits.
    assert!(!app_id_matches(Some("fi"), &["fi".to_string()]));
}

#[test]
fn select_window_prefers_newly_created_browser_window() {
    let windows = vec![
        window(10, "kitty", Some((100, 0))),
        window(11, "firefox", Some((50, 0))),
        window(20, "firefox", Some((40, 0))),
    ];
    let stems = vec!["firefox".to_string()];
    let pre_ids: std::collections::HashSet<u64> = [11].into_iter().collect();
    // Window 20 is the new firefox window (not in pre_ids), so it wins even
    // though window 11 was focused more recently.
    assert_eq!(select_window_to_focus(&windows, &stems, &pre_ids), Some(20));
}

#[test]
fn select_window_falls_back_to_most_recently_focused() {
    let windows = vec![
        window(11, "firefox", Some((50, 0))),
        window(20, "firefox", Some((90, 0))),
    ];
    let stems = vec!["firefox".to_string()];
    // Both windows already existed: raise the most recently focused one (20),
    // which is where browsers add a new tab.
    let pre_ids: std::collections::HashSet<u64> = [11, 20].into_iter().collect();
    assert_eq!(select_window_to_focus(&windows, &stems, &pre_ids), Some(20));
}

#[test]
fn select_window_returns_none_without_browser_windows() {
    let windows = vec![window(10, "kitty", Some((100, 0)))];
    let stems = vec!["firefox".to_string()];
    let pre_ids = std::collections::HashSet::new();
    assert_eq!(select_window_to_focus(&windows, &stems, &pre_ids), None);
}
