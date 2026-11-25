use super::*;

fn config(mode: &str) -> McpServerConfig {
    serde_json::from_value(serde_json::json!({
        "command":"python3", "args":["-u","-c",include_str!("pagination_fixture.py"),mode],
        "shared":false,"timeout_secs":10
    }))
    .unwrap()
}

#[tokio::test]
async fn tools_list_follows_opaque_cursor_and_publishes_complete_catalog() {
    let client = McpClient::connect("pagination-normal".into(), &config("normal"))
        .await
        .unwrap();
    assert_eq!(
        client
            .tools()
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        vec!["first", "second"]
    );
}

#[tokio::test]
async fn incomplete_tools_list_keeps_the_last_good_catalog() {
    for (mode, reason) in [
        ("cycle", "cursor"),
        ("duplicate", "duplicate"),
        ("error", "later page failed"),
        ("invalid", "invalid type"),
        ("missing", "no result"),
        ("many", "100 pages"),
        ("count", "10000 tools"),
        ("oversize", "32 MiB"),
    ] {
        let client = McpClient::connect(format!("pagination-{mode}"), &config(mode))
            .await
            .unwrap();
        assert_eq!(client.tools()[0].name, "seed");
        let result =
            tokio::time::timeout(std::time::Duration::from_secs(15), client.refresh_tools())
                .await
                .expect("catalog refresh must terminate");
        let error = result.expect_err(mode);
        assert!(format!("{error:#}").contains(reason), "{mode}: {error:#}");
        assert_eq!(
            client
                .tools()
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            vec!["seed"],
            "{mode}"
        );
    }
}

#[tokio::test]
async fn successful_empty_catalog_replaces_previous_tools() {
    let client = McpClient::connect("pagination-empty".into(), &config("empty"))
        .await
        .unwrap();
    assert_eq!(client.tools()[0].name, "seed");
    client.refresh_tools().await.unwrap();
    assert!(client.tools().is_empty());
}

#[tokio::test]
async fn tools_list_uses_one_deadline_for_the_whole_catalog() {
    let client = McpClient::connect("pagination-deadline".into(), &config("deadline"))
        .await
        .unwrap();
    let mut handle = client.handle();
    handle.request_timeout = std::time::Duration::from_millis(350);
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), handle.refresh_tools())
        .await
        .expect("whole-catalog deadline was not enforced");
    let error = result.unwrap_err();
    assert!(
        format!("{error:#}").contains("catalog refresh timed out"),
        "{error:#}"
    );
    assert_eq!(handle.tools()[0].name, "seed");
}
