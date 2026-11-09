use super::*;

#[tokio::test]
async fn tool_images_cross_sdk_mcp_with_text_labels_and_error_status() {
    const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNgYGD4DwABBAEAX+XDSwAAAABJRU5ErkJggg==";
    const GIF: &str = "R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7";
    for is_error in [false, true] {
        let fake = Fake::new();
        let provider = fake.provider();
        let answer = |_: &str, _: &Value| {
            let output = jcode_tool_types::ToolOutput::new("Screenshot captured; file removed")
                .with_labeled_image("image/png", PNG, "Original screenshot")
                .with_image("image/gif", GIF);
            let mut result = NativeToolResult::from_tool_output(String::new(), output);
            if is_error {
                result.result.error = result.result.output.take();
                result.is_error = true;
            }
            result
        };
        let events = run_turn(&provider, &[Message::user("mcp echo")], Some(&answer)).await;
        assert!(errors(&events).is_empty(), "{events:?}");
        let inputs = fake.inputs();
        let reply = inputs
            .iter()
            .map(|row| &row["message"]["response"]["response"]["mcp_response"])
            .find(|reply| reply["id"] == 2)
            .expect("tool callback must reach Claude Code");
        assert_eq!(
            reply,
            &json!({"jsonrpc":"2.0", "id":2, "result": {
                "content": [
                    {"type":"text", "text":"Screenshot captured; file removed"},
                    {"type":"text", "text":"Original screenshot"},
                    {"type":"image", "data":PNG, "mimeType":"image/png"},
                    {"type":"image", "data":GIF, "mimeType":"image/gif"}
                ],
                "isError":is_error
            }})
        );
        provider.shutdown().await;
    }
}

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
