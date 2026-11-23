use super::*;
use jcode_message_types::{ContentBlock, Message, Role, ToolDefinition};
use serde_json::json;

fn def(name: &str, deferred: bool) -> ToolDefinition {
    ToolDefinition {
        name: name.to_string(),
        description: format!("{name} description"),
        input_schema: json!({"type": "object", "properties": {"q": {"type": "string"}}}),
        defer_loading: deferred,
    }
}

fn msg(role: Role, content: Vec<ContentBlock>) -> Message {
    Message {
        role,
        content: content.into(),
        ..Message::user("")
    }
}

#[test]
fn supported_models_are_3_6_and_newer() {
    for model in [
        "gemini-3.6-flash",
        "gemini-3.8-flash",
        "gemini-4-hydrogen",
        "models/gemini-4-argon",
        "gemini-5.0-pro-preview",
    ] {
        assert!(model_supports_tool_retrieval(model), "{model}");
    }
    for model in [
        "gemini-3.1-pro-preview",
        "gemini-2.5-pro",
        "gemini-pro-latest",
        "claude-opus",
    ] {
        assert!(!model_supports_tool_retrieval(model), "{model}");
    }
}

#[test]
fn deferred_tools_add_tool_search_and_defer_loading() {
    let tools = vec![def("bash", false), def("mcp__x__future_tool", true)];
    let built = build_interaction_tools(&tools, &[]);
    assert_eq!(built[0], json!({"type": "tool_search"}));
    assert_eq!(built[1]["name"], "bash");
    assert!(built[1].get("defer_loading").is_none());
    assert_eq!(built[2]["name"], "mcp__x__future_tool");
    assert_eq!(built[2]["defer_loading"], true);
    assert_eq!(built[2]["type"], "function");
}

#[test]
fn eager_only_tools_have_no_tool_search() {
    let built = build_interaction_tools(&[def("bash", false)], &[]);
    assert_eq!(built.len(), 1);
    assert_eq!(built[0]["name"], "bash");
}

#[test]
fn tool_reference_loads_deferred_tool_eagerly() {
    let messages = vec![msg(
        Role::User,
        vec![ContentBlock::ToolReference {
            tool_use_id: "c1".into(),
            tool_name: "gmail".into(),
        }],
    )];
    let built = build_interaction_tools(&[def("bash", false), def("gmail", true)], &messages);
    assert_eq!(built.len(), 2, "no tool_search once nothing is deferred");
    assert!(built.iter().all(|tool| tool.get("defer_loading").is_none()));
}

#[test]
fn history_replays_search_steps_unchanged_before_function_call() {
    let search_call = json!({
        "type": "tool_search_call", "id": "tsc_1",
        "arguments": {"function_names": ["mcp__x__t"]}, "signature": "SIG1"
    });
    let search_result = json!({
        "type": "tool_search_result", "call_id": "tsc_1", "signature": "SIG2",
        "result": [{"type": "function", "name": "mcp__x__t"}]
    });
    let messages = vec![
        Message::user("hi"),
        msg(
            Role::Assistant,
            vec![
                ContentBlock::ProviderNative {
                    provider: PROVIDER_NATIVE_GEMINI.into(),
                    item: search_call.clone(),
                },
                ContentBlock::ProviderNative {
                    provider: PROVIDER_NATIVE_GEMINI.into(),
                    item: search_result.clone(),
                },
                ContentBlock::ProviderNative {
                    provider: "openai".into(),
                    item: json!({"type": "web_search_call"}),
                },
                ContentBlock::ToolUse {
                    id: "fc_1".into(),
                    name: "mcp__x__t".into(),
                    input: json!({"a": 1}),
                    thought_signature: Some("SIG3".into()),
                },
            ],
        ),
        msg(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: "fc_1".into(),
                content: "done".into(),
                is_error: Some(true),
            }],
        ),
    ];
    let steps = build_interaction_steps(&messages);
    assert_eq!(steps[0]["type"], "user_input");
    assert_eq!(steps[0]["content"][0]["text"], "hi");
    assert_eq!(steps[1], search_call);
    assert_eq!(steps[2], search_result);
    assert_eq!(
        steps[3],
        json!({"type": "function_call", "id": "fc_1", "name": "mcp__x__t",
               "arguments": {"a": 1}, "signature": "SIG3"})
    );
    assert_eq!(
        steps[4],
        json!({"type": "function_result", "name": "mcp__x__t", "call_id": "fc_1",
               "result": [{"type": "text", "text": "done"}], "is_error": true})
    );
    assert_eq!(steps.len(), 5);
}

#[test]
fn request_is_stateless_with_system_instruction() {
    let body = build_interaction_request(
        "gemini-4-hydrogen",
        &[Message::user("hi")],
        &[def("mcp__x__t", true)],
        Some("be brief".into()),
    );
    assert_eq!(body["store"], false);
    assert_eq!(body["model"], "gemini-4-hydrogen");
    assert_eq!(body["system_instruction"], "be brief");
    assert_eq!(body["tools"][0]["type"], "tool_search");
}

#[test]
fn response_decodes_search_thought_text_and_call_in_order() {
    let response: InteractionResponse = serde_json::from_value(json!({
        "status": "requires_action",
        "steps": [
            {"type": "user_input", "content": [{"type": "text", "text": "echo"}]},
            {"type": "thought", "signature": "T", "summary": [{"type": "text", "text": "plan"}]},
            {"type": "tool_search_call", "id": "tsc", "arguments": {}, "signature": "S"},
            {"type": "tool_search_result", "call_id": "tsc", "signature": "R", "result": []},
            {"type": "model_output", "content": [{"type": "text", "text": "ok"}]},
            {"type": "function_call", "id": "fc", "name": "mcp__x__t", "arguments": {"a": 1}}
        ],
        "usage": {"total_input_tokens": 7, "total_output_tokens": 3}
    }))
    .unwrap();
    let events = interaction_events(&response);
    assert_eq!(events.len(), 6);
    assert_eq!(events[0], InteractionEvent::Thinking("plan".into()));
    assert!(matches!(&events[1], InteractionEvent::Native(v) if v["type"] == "thought"));
    assert!(matches!(&events[2], InteractionEvent::Native(v) if v["signature"] == "S"));
    assert!(matches!(&events[3], InteractionEvent::Native(v) if v["signature"] == "R"));
    assert_eq!(events[4], InteractionEvent::Text("ok".into()));
    assert_eq!(
        events[5],
        InteractionEvent::FunctionCall {
            id: "fc".into(),
            name: "mcp__x__t".into(),
            arguments: json!({"a": 1}),
            signature: None,
        }
    );
    assert_eq!(response.usage.unwrap().total_input_tokens, Some(7));
}
