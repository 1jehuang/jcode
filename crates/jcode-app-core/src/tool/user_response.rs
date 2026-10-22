//! User-facing Markdown responses rendered inline by Desktop from tool input.
use super::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

pub(super) struct UserResponseTool;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    content: String,
    title: Option<String>,
    #[serde(rename = "intent")]
    _intent: Option<String>,
    #[serde(rename = "accept_large_output")]
    _accept_large_output: Option<bool>,
}

#[async_trait]
impl Tool for UserResponseTool {
    fn name(&self) -> &str {
        "user_response"
    }

    fn description(&self) -> &str {
        "Respond to the user with Markdown rendered inline in Jcode Desktop. Use this for user-visible answers, explanations, and completion summaries. Put the full response in content, with an optional title. Do not duplicate this response in a normal assistant message. This is a response, not a separate panel or an applet."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "content": {"type": "string", "description": "The complete user-visible response as Markdown."},
                "title": {"type": "string", "description": "Optional response heading."},
                "intent": super::intent_schema_property()
            },
            "required": ["content"]
        })
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        let input: Input = serde_json::from_value(input)?;
        let mut output = ToolOutput::new(input.content);
        output.title = input.title;
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> ToolContext {
        ToolContext {
            session_id: "user-response-test".into(),
            message_id: "msg".into(),
            tool_call_id: "call".into(),
            working_dir: None,
            stdin_request_tx: None,
            graceful_shutdown_signal: None,
            execution_mode: crate::tool::ToolExecutionMode::AgentTurn,
        }
    }

    #[tokio::test]
    async fn user_response_preserves_markdown_and_optional_title() {
        // Assert exact tool output, so take the shared env lock: sibling
        // hook tests set JCODE_HOOK_POST_TOOL_FEEDBACK process-wide while
        // holding it, and a concurrent run here would append the
        // [post_tool_feedback] block to this tool's output.
        let _env_lock = crate::storage::lock_test_env();
        for title in [None, Some("Summary")] {
            let content = "# Answer\n\n**Markdown** with Unicode: 日本語\n";
            let result = UserResponseTool
                .execute(
                    json!({
                        "content": content, "title": title,
                        "intent": "Respond", "accept_large_output": null
                    }),
                    context(),
                )
                .await
                .unwrap();
            assert_eq!(result.output, content);
            assert_eq!(result.title.as_deref(), title);
            assert!(result.metadata.is_none());
            assert!(result.images.is_empty());
        }
        let result = UserResponseTool
            .execute(json!({"content": ""}), context())
            .await
            .unwrap();
        assert_eq!(result.output, "");
        assert!(result.title.is_none());
    }

    #[tokio::test]
    async fn user_response_rejects_invalid_input() {
        for input in [
            json!({}),
            json!({"content": null}),
            json!({"content": 42}),
            json!({"content": "ok", "title": 42}),
            json!({"content": "ok", "unexpected": true}),
        ] {
            assert!(UserResponseTool.execute(input, context()).await.is_err());
        }
    }

    #[tokio::test]
    async fn user_response_desktop_registration_is_reversible_and_isolated() {
        // Same as above: exact-output assertions must not race the sibling
        // post_tool_feedback hook test's process-global env var.
        let _env_lock = crate::storage::lock_test_env();
        let desktop = crate::tool::Registry::empty();
        let tui = crate::tool::Registry::empty();
        assert!(!crate::tool::Registry::base_tools(&tui.skills).contains_key("user_response"));
        assert!(
            tui.execute("user_response", json!({"content": "no"}), context())
                .await
                .is_err()
        );
        desktop.set_desktop_ui_enabled(true).await;
        let defs = desktop.definitions(None).await;
        let def = defs.iter().find(|d| d.name == "user_response").unwrap();
        assert_eq!(def.input_schema["required"], json!(["content", "intent"]));
        assert_eq!(def.input_schema["additionalProperties"], false);
        assert!(def.description.contains("Do not duplicate"));
        assert!(tui.definitions(None).await.is_empty());
        let output = desktop
            .execute(
                "user_response",
                json!({"content": "# Hello", "title": "Hi"}),
                context(),
            )
            .await
            .unwrap();
        assert_eq!(output.output, "# Hello");
        assert_eq!(output.title.as_deref(), Some("Hi"));
        desktop.set_desktop_ui_enabled(false).await;
        assert!(desktop.definitions(None).await.is_empty());
        assert!(
            desktop
                .execute("user_response", json!({"content": "no"}), context())
                .await
                .is_err()
        );
    }
}
