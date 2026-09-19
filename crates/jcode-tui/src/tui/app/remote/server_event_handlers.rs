use super::*;

pub(super) fn handle_tool_done(
    app: &mut App,
    remote: &mut impl RemoteEventState,
    id: String,
    name: String,
    output: String,
    error: Option<String>,
) -> bool {
    let parsed_input = remote.tool_input_for(&id);
    let display_output = remote.handle_tool_done(&id, &name, &output);
    let display_output = if error.is_some()
        && !display_output.starts_with("Error:")
        && !display_output.starts_with("error:")
        && !display_output.starts_with("Failed:")
    {
        format!("Error: {}", display_output)
    } else {
        display_output
    };
    let existing_tool_call = app
        .streaming_tool_calls
        .iter()
        .find(|tc| tc.id == id)
        .cloned();
    let mut tool_call = existing_tool_call.unwrap_or_else(|| ToolCall {
        id: id.clone(),
        name: name.clone(),
        input: serde_json::Value::Null,
        intent: None,
        thought_signature: None,
    });
    if tool_input_has_edit_payload(&parsed_input) && !tool_input_has_edit_payload(&tool_call.input)
    {
        tool_call.input = parsed_input;
        tool_call.refresh_intent_from_input();
    } else if tool_call.input.is_null() && parsed_input.is_object() {
        tool_call.input = parsed_input;
        tool_call.refresh_intent_from_input();
    }
    app.commit_pending_streaming_assistant_message();
    crate::tui::mermaid::clear_streaming_preview_diagram();
    let is_batch = tool_call.name == "batch";
    app.observe_tool_result(&tool_call, &output, error.is_some(), None);
    app.note_tool_completed(&tool_call, error.is_some());
    app.push_display_message(DisplayMessage {
        role: "tool".to_string(),
        content: display_output,
        tool_calls: vec![],
        duration_secs: None,
        title: None,
        tool_data: Some(tool_call.clone()),
    });
    app.note_todo_gate_result(&tool_call, &output, error.is_some());
    if is_batch {
        app.batch_progress = None;
    }
    // Only remove the completed call. When the model emits several tool calls
    // in one assistant message, siblings that already streamed their parsed
    // input/intent are still waiting for their own ToolDone; clearing the
    // whole list here made their rows render with no intent or summary.
    app.streaming_tool_calls.retain(|tc| tc.id != id);
    app.status = ProcessingStatus::Streaming;
    true
}

fn tool_input_has_edit_payload(input: &serde_json::Value) -> bool {
    input
        .get("old_string")
        .and_then(|value| value.as_str())
        .is_some_and(|value| !value.is_empty())
        || input
            .get("new_string")
            .and_then(|value| value.as_str())
            .is_some_and(|value| !value.is_empty())
        || input
            .get("content")
            .and_then(|value| value.as_str())
            .is_some_and(|value| !value.is_empty())
}

pub(super) fn handle_generated_image(
    app: &mut App,
    id: String,
    path: String,
    metadata_path: Option<String>,
    output_format: String,
    revised_prompt: Option<String>,
) -> bool {
    app.pause_streaming_tps(false);
    app.commit_pending_streaming_assistant_message();
    let input = crate::message::generated_image_tool_input(
        &path,
        metadata_path.as_deref(),
        &output_format,
        revised_prompt.as_deref(),
    );
    let tool_call = ToolCall {
        id: id.clone(),
        name: crate::message::GENERATED_IMAGE_TOOL_NAME.to_string(),
        input,
        intent: Some("OpenAI native image generation".to_string()),
        thought_signature: None,
    };
    let summary = crate::message::generated_image_summary(
        &path,
        metadata_path.as_deref(),
        &output_format,
        revised_prompt.as_deref(),
    );
    app.push_display_message(DisplayMessage {
        role: "tool".to_string(),
        content: summary,
        tool_calls: vec![],
        duration_secs: None,
        title: Some("Generated image".to_string()),
        tool_data: Some(tool_call),
    });
    app.status = ProcessingStatus::Streaming;
    true
}
