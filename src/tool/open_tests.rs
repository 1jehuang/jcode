use super::*;

fn make_ctx() -> ToolContext {
    ToolContext {
        session_id: "test-session".to_string(),
        message_id: "test-msg".to_string(),
        tool_call_id: "test-call".to_string(),
        working_dir: Some(std::env::temp_dir()),
        sandbox_root: None,
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
    let temp_file = std::env::temp_dir().join("jcode-open-tool-file-url.txt");
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
