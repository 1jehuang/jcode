use super::*;

async fn listing(provider: &ClaudeCodeProvider, definitions: &[ToolDefinition]) -> Vec<String> {
    let mut stream = provider
        .complete(&[Message::user("file-tools")], definitions, SYSTEM, None)
        .await
        .unwrap();
    let mut events = Vec::new();
    loop {
        let event = tokio::time::timeout(Duration::from_secs(10), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let done = matches!(event, StreamEvent::MessageEnd { .. });
        events.push(event);
        if done {
            break;
        }
    }
    serde_json::from_str(&text(&events)).unwrap()
}

fn editors() -> Vec<ToolDefinition> {
    [
        "write",
        "edit",
        "apply_patch",
        "patch",
        "replace",
        "read",
        "bash",
    ]
    .into_iter()
    .map(|name| ToolDefinition::new(name, name, json!({"type":"object"})))
    .collect()
}

#[tokio::test]
async fn editors_require_ready_executor_and_restart_when_readiness_changes() {
    let fake = Fake::new();
    let provider = fake.provider();
    let definitions = editors();
    assert!(listing(&provider, &definitions).await.is_empty());
    let initial = fake.launches();
    assert!(
        !initial[0]["argv"]
            .as_array()
            .unwrap()
            .iter()
            .any(|arg| arg == "--disallowedTools")
    );
    provider.set_host_tool_executor_available(true);
    assert_eq!(
        listing(&provider, &definitions).await,
        vec!["write", "edit", "apply_patch", "patch", "replace"]
    );
    let launches = fake.launches();
    assert_eq!(launches.len(), 2);
    let argv = launches[1]["argv"].as_array().unwrap();
    for native in ["Write", "Edit", "MultiEdit", "NotebookEdit"] {
        assert!(
            argv.iter().any(|arg| arg == native),
            "missing deny for {native}"
        );
    }
    assert!(!argv.iter().any(|arg| arg == "Read" || arg == "Bash"));
    provider.set_host_tool_executor_available(false);
    assert!(listing(&provider, &definitions).await.is_empty());
    assert_eq!(fake.launches().len(), 3);
    provider.shutdown().await;
}

#[tokio::test]
async fn editor_only_registry_starts_mcp_but_disabled_exposure_stays_native() {
    let fake = Fake::new();
    let definitions: Vec<_> = editors()
        .into_iter()
        .filter(|t| t.name == "write" || t.name == "edit")
        .collect();
    let provider = fake.provider();
    provider.set_host_tool_executor_available(true);
    assert_eq!(
        listing(&provider, &definitions).await,
        vec!["write", "edit"]
    );
    provider.shutdown().await;
    let fake = Fake::new();
    let mut settings = fake.settings();
    settings.expose_jcode_tools = false;
    let provider = ClaudeCodeProvider::with_settings(settings);
    provider.set_host_tool_executor_available(true);
    assert!(listing(&provider, &definitions).await.is_empty());
    assert!(
        !fake.launches()[0]["argv"]
            .as_array()
            .unwrap()
            .iter()
            .any(|arg| arg == "--disallowedTools")
    );
    provider.shutdown().await;
}
