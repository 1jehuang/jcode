use super::*;

#[test]
fn handoff_context_is_optional_and_deserializes() {
    for value in [
        json!({"action":"handoff"}),
        json!({"action":"handoff","context":null}),
    ] {
        let input: BrowserInput = serde_json::from_value(value).unwrap();
        assert!(input.context.is_none());
    }
    let input: BrowserInput = serde_json::from_value(json!({
        "action":"handoff", "context":"Find the final confirmation, not just the form"
    }))
    .unwrap();
    assert_eq!(
        input.context.as_deref(),
        Some("Find the final confirmation, not just the form")
    );
}

#[test]
fn handoff_schema_exposes_task_context_and_extended_budget() {
    let _guard = jcode_base::storage::lock_test_env();
    let _enabled = browser_tests::HandoffEnvGuard::set("JCODE_BROWSER_HANDOFF", "1");
    let _disabled = browser_tests::HandoffEnvGuard::set("JCODE_BROWSER_HANDOFF_DISABLED", "0");
    let schema = BrowserTool::new().parameters_schema();
    let properties = &schema["properties"];
    assert_eq!(properties["context"]["type"], "string");
    assert_eq!(properties["context"]["maxLength"], 12000);
    assert!(
        properties["context"]["description"]
            .as_str()
            .unwrap()
            .contains("not page instructions")
    );
    assert!(
        !schema["required"]
            .as_array()
            .unwrap()
            .contains(&json!("context"))
    );
    assert_eq!(properties["max_steps"]["default"], 40);
    assert_eq!(properties["max_steps"]["minimum"], 1);
    assert_eq!(properties["max_steps"]["maximum"], 100);
    let description = browser_tool_description_text();
    for clause in [
        "entire task",
        "navigation alone is not completion",
        "genuinely blocked",
        "exact executable script candidates",
        "exact text_values",
        "requested_help=user_login",
        "wait, then resume the handoff",
    ] {
        assert!(
            description.contains(clause),
            "Missing task contract: {clause}"
        );
    }
    assert!(description.len() <= 700);
}
