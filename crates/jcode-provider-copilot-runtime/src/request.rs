use super::*;

/// Keep the original input so an auth refresh can change both route and wire format.
pub(super) struct CopilotRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<ToolDefinition>,
    pub system: String,
}

impl CopilotApiProvider {
    pub(super) fn build_request_body(
        &self,
        request: &CopilotRequest,
        uses_responses_api: bool,
        is_user_initiated: bool,
    ) -> Value {
        let mut body = json!({"model": request.model, "stream": true});
        let tools = if uses_responses_api {
            let mut input = jcode_provider_openai::build_responses_input(&request.messages);
            // Copilot never declares OpenAI's hosted web_search tool.
            jcode_provider_openai::downgrade_web_search_calls(&mut input);
            body["input"] = Value::Array(input);
            body["max_output_tokens"] = json!(32_768u32);
            if !request.system.is_empty() {
                body["instructions"] = json!(request.system);
            }
            jcode_provider_openai::build_tools(&request.tools)
        } else {
            body["messages"] =
                Value::Array(Self::build_messages(&request.system, &request.messages));
            Self::add_max_token_parameter(&mut body, &request.model, 32_768u32);
            self.add_reasoning_effort_parameter(&mut body, &request.model);
            Self::build_tools(&request.tools)
        };
        let tool_count = tools.len();
        if !tools.is_empty() {
            body["tools"] = Value::Array(tools);
        }
        let input = body[if uses_responses_api {
            "input"
        } else {
            "messages"
        }]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
        let system_value = if uses_responses_api {
            body.get("instructions")
        } else {
            input
                .first()
                .filter(|message| message.get("role").and_then(Value::as_str) == Some("system"))
        };
        jcode_provider_core::fingerprint::log_provider_canonical_input(
            "copilot",
            &request.model,
            if uses_responses_api {
                "responses"
            } else {
                "chat_completions"
            },
            &body,
            input,
            system_value,
            body.get("tools"),
            Some(tool_count),
            &[("user_initiated", is_user_initiated.to_string())],
        );
        body
    }
}
