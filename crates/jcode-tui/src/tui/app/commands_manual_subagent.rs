//! Manual subagent launch command.

use super::*;

pub(super) fn launch_manual_subagent(app: &mut App, spec: ManualSubagentSpec) {
    let description = derive_subagent_description(&spec.prompt);
    let tool_call = crate::message::ToolCall {
        id: id::new_id("call"),
        name: "subagent".to_string(),
        input: serde_json::json!({
            "description": description,
            "prompt": spec.prompt,
            "subagent_type": spec.subagent_type,
            "model": spec.model,
            "session_id": spec.session_id,
            "command": "/subagent",
        }),
        intent: None,
        thought_signature: None,
    };

    app.push_display_message(DisplayMessage {
        role: "tool".to_string(),
        content: tool_call.name.clone(),
        tool_calls: vec![],
        duration_secs: None,
        title: None,
        tool_data: Some(tool_call.clone()),
    });

    let content_blocks = vec![ContentBlock::ToolUse {
        id: tool_call.id.clone(),
        name: tool_call.name.clone(),
        input: tool_call.input.clone(),
        thought_signature: None,
    }];
    app.add_provider_message(Message {
        role: Role::Assistant,
        content: content_blocks.clone().into(),
        timestamp: Some(chrono::Utc::now()),
        tool_duration_ms: None,
    });
    let message_id = app.session.add_message(Role::Assistant, content_blocks);
    let _ = app.session.save();
    app.subagent_status = Some("starting subagent".to_string());
    app.set_status_notice("Running subagent");

    let registry = app.registry.clone();
    let session_id = app.session.id.clone();
    let working_dir = app.session.working_dir.clone();
    let tool_call_for_task = tool_call.clone();
    tokio::spawn(async move {
        Bus::global().publish(BusEvent::ToolUpdated(ToolEvent {
            session_id: session_id.clone(),
            message_id: message_id.clone(),
            tool_call_id: tool_call_for_task.id.clone(),
            tool_name: tool_call_for_task.name.clone(),
            status: ToolStatus::Running,
            intent: tool_call_for_task.intent.clone(),
            title: None,
        }));

        let ctx = crate::tool::ToolContext {
            session_id: session_id.clone(),
            message_id: message_id.clone(),
            tool_call_id: tool_call_for_task.id.clone(),
            working_dir: working_dir.as_deref().map(PathBuf::from),
            stdin_request_tx: None,
            graceful_shutdown_signal: None,
            execution_mode: crate::tool::ToolExecutionMode::Direct,
        };

        let start = Instant::now();
        let result = registry
            .execute(
                &tool_call_for_task.name,
                tool_call_for_task.input.clone(),
                ctx,
            )
            .await;
        let duration_ms = start.elapsed().as_millis() as u64;

        let (output, is_error, title, status) = match result {
            Ok(output) => {
                crate::telemetry::record_tool_call();
                (output.output, false, output.title, ToolStatus::Completed)
            }
            Err(error) => {
                crate::telemetry::record_tool_failure();
                (format!("Error: {}", error), true, None, ToolStatus::Error)
            }
        };

        Bus::global().publish(BusEvent::ToolUpdated(ToolEvent {
            session_id: session_id.clone(),
            message_id,
            tool_call_id: tool_call_for_task.id.clone(),
            tool_name: tool_call_for_task.name.clone(),
            status,
            intent: tool_call_for_task.intent.clone(),
            title: title.clone(),
        }));

        Bus::global().publish(BusEvent::ManualToolCompleted(ManualToolCompleted {
            session_id,
            tool_call: tool_call_for_task,
            output,
            is_error,
            title,
            duration_ms,
        }));
    });
}
