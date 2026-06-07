use serde_json::Value;

use super::{DesktopModelChoice, DesktopSessionEvent, DesktopSessionStatus};

pub(super) fn desktop_event_from_server_value(value: &Value) -> Option<DesktopSessionEvent> {
    match value.get("type").and_then(Value::as_str)? {
        "session" => value
            .get("session_id")
            .and_then(Value::as_str)
            .map(|session_id| DesktopSessionEvent::SessionStarted {
                session_id: session_id.to_string(),
            }),
        "session_renamed" => Some(DesktopSessionEvent::SessionRenamed {
            title: value
                .get("title")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            display_title: value
                .get("display_title")
                .and_then(Value::as_str)
                .unwrap_or("session")
                .to_string(),
        }),
        "text_delta" => value
            .get("text")
            .and_then(Value::as_str)
            .map(|text| DesktopSessionEvent::TextDelta(text.to_string())),
        "text_replace" => value
            .get("text")
            .and_then(Value::as_str)
            .map(|text| DesktopSessionEvent::TextReplace(text.to_string())),
        "connection_phase" => value
            .get("phase")
            .and_then(Value::as_str)
            .map(|phase| DesktopSessionEvent::Status(DesktopSessionStatus::external(phase))),
        "connection_type" => optional_server_str(value, "connection_type")
            .or_else(|| optional_server_str(value, "connection"))
            .map(|connection_type| DesktopSessionEvent::RuntimeMetadata {
                connection_type: Some(connection_type.to_string()),
                status_detail: None,
                upstream_provider: None,
            }),
        "status_detail" => optional_server_str(value, "detail").map(|detail| {
            DesktopSessionEvent::RuntimeMetadata {
                connection_type: None,
                status_detail: Some(detail.to_string()),
                upstream_provider: None,
            }
        }),
        "upstream_provider" => optional_server_str(value, "provider")
            .or_else(|| optional_server_str(value, "provider_name"))
            .map(|upstream_provider| DesktopSessionEvent::RuntimeMetadata {
                connection_type: None,
                status_detail: None,
                upstream_provider: Some(upstream_provider.to_string()),
            }),
        "tool_start" => {
            value
                .get("name")
                .and_then(Value::as_str)
                .map(|name| DesktopSessionEvent::ToolStarted {
                    id: optional_server_str(value, "id").map(ToOwned::to_owned),
                    name: name.to_string(),
                })
        }
        "tool_exec" => value.get("name").and_then(Value::as_str).map(|name| {
            DesktopSessionEvent::ToolExecuting {
                id: optional_server_str(value, "id").map(ToOwned::to_owned),
                name: name.to_string(),
            }
        }),
        "tool_input" => {
            value
                .get("delta")
                .and_then(Value::as_str)
                .map(|delta| DesktopSessionEvent::ToolInput {
                    id: optional_server_str(value, "id").map(ToOwned::to_owned),
                    delta: delta.to_string(),
                })
        }
        "tool_done" => value.get("name").and_then(Value::as_str).map(|name| {
            DesktopSessionEvent::ToolFinished {
                id: optional_server_str(value, "id").map(ToOwned::to_owned),
                name: name.to_string(),
                summary: value
                    .get("output")
                    .and_then(Value::as_str)
                    .map(compact_tool_output)
                    .unwrap_or_else(|| "done".to_string()),
                is_error: value.get("error").is_some_and(|error| !error.is_null()),
            }
        }),
        "interrupted" => Some(DesktopSessionEvent::Status(
            DesktopSessionStatus::Interrupted,
        )),
        "model_changed" => value.get("model").and_then(Value::as_str).map(|model| {
            DesktopSessionEvent::ModelChanged {
                model: model.to_string(),
                provider_name: value
                    .get("provider_name")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                error: value
                    .get("error")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
            }
        }),
        "reasoning_effort_changed" => {
            let effort = value
                .get("effort")
                .and_then(Value::as_str)
                .unwrap_or("unchanged");
            let status = if let Some(error) = value.get("error").and_then(Value::as_str) {
                DesktopSessionStatus::ReasoningEffortFailed(error.to_string())
            } else {
                DesktopSessionStatus::ReasoningEffort(effort.to_string())
            };
            Some(DesktopSessionEvent::Status(status))
        }
        "service_tier_changed" => Some(DesktopSessionEvent::Status(
            if let Some(error) = value.get("error").and_then(Value::as_str) {
                DesktopSessionStatus::ServiceTierFailed(error.to_string())
            } else {
                DesktopSessionStatus::ServiceTier(
                    value
                        .get("service_tier")
                        .and_then(Value::as_str)
                        .unwrap_or("standard")
                        .to_string(),
                )
            },
        )),
        "transport_changed" => Some(DesktopSessionEvent::Status(
            if let Some(error) = value.get("error").and_then(Value::as_str) {
                DesktopSessionStatus::TransportFailed(error.to_string())
            } else {
                DesktopSessionStatus::Transport(
                    value
                        .get("transport")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                        .to_string(),
                )
            },
        )),
        "compaction_mode_changed" => Some(DesktopSessionEvent::Status(
            if let Some(error) = value.get("error").and_then(Value::as_str) {
                DesktopSessionStatus::CompactionModeFailed(error.to_string())
            } else {
                DesktopSessionStatus::CompactionMode(
                    value
                        .get("mode")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                        .to_string(),
                )
            },
        )),
        "compact_result" => Some(DesktopSessionEvent::Status(
            DesktopSessionStatus::CompactResult {
                message: value
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("compaction request finished")
                    .to_string(),
                success: value
                    .get("success")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            },
        )),
        "history" => model_catalog_event_from_server_value(value),
        "available_models_updated" => Some(DesktopSessionEvent::ModelCatalog {
            current_model: value
                .get("provider_model")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            provider_name: value
                .get("provider_name")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            models: model_choices_from_server_value(value),
            reasoning_effort: None,
            service_tier: None,
            compaction_mode: None,
        }),
        "stdin_request" => Some(DesktopSessionEvent::StdinRequest {
            request_id: non_empty_server_str(value, "request_id")?.to_string(),
            prompt: value
                .get("prompt")
                .and_then(Value::as_str)
                .unwrap_or("interactive input requested")
                .to_string(),
            is_password: value
                .get("is_password")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            tool_call_id: non_empty_server_str(value, "tool_call_id")?.to_string(),
        }),
        "reload_progress" => Some(DesktopSessionEvent::ReloadProgress {
            step: optional_server_str(value, "step")
                .unwrap_or("reload")
                .to_string(),
            message: optional_server_str(value, "message")
                .or_else(|| optional_server_str(value, "detail"))
                .unwrap_or("server reload progress")
                .to_string(),
            success: value.get("success").and_then(Value::as_bool),
            output: optional_server_str(value, "output").map(ToOwned::to_owned),
        }),
        "tokens" => Some(DesktopSessionEvent::TokenUsage {
            input: server_u64(value, "input").unwrap_or(0),
            output: server_u64(value, "output").unwrap_or(0),
            cache_read_input: server_u64(value, "cache_read_input"),
            cache_creation_input: server_u64(value, "cache_creation_input"),
        }),
        "session_close_requested" => Some(DesktopSessionEvent::SessionCloseRequested {
            reason: optional_server_str(value, "reason")
                .unwrap_or("server requested the session be closed")
                .to_string(),
        }),
        "message_end" | "kv_cache_request" => None,
        "generated_image" => Some(DesktopSessionEvent::SystemNotice {
            title: "generated image".to_string(),
            message: optional_server_str(value, "path")
                .or_else(|| optional_server_str(value, "file"))
                .or_else(|| optional_server_str(value, "output_format"))
                .map(ToOwned::to_owned),
        }),
        // These are internal/protocol-side events used by the TUI, swarm,
        // memory, side-panel, communication, or background-task systems. They
        // can arrive interleaved with assistant text while the model streams,
        // but they are not assistant-visible transcript content and should not
        // become desktop chat rows.
        "batch_progress"
        | "mcp_status"
        | "memory_injected"
        | "memory_activity"
        | "notification"
        | "compaction"
        | "soft_interrupt_injected"
        | "side_panel_state"
        | "swarm_status"
        | "swarm_plan"
        | "swarm_plan_proposal"
        | "transcript"
        | "input_shell_result"
        | "split_response"
        | "compacted_history"
        | "comm_request"
        | "comm_response"
        | "comm_status"
        | "comm_presence" => None,
        "reloading" => Some(DesktopSessionEvent::Reloading {
            new_socket: value
                .get("new_socket")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
        }),
        "done" => Some(DesktopSessionEvent::Done),
        "error" => Some(DesktopSessionEvent::Error(
            value
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown server error")
                .to_string(),
        )),
        _ => None,
    }
}

fn server_u64(value: &Value, field: &str) -> Option<u64> {
    value.get(field).and_then(Value::as_u64)
}

fn non_empty_server_str<'a>(value: &'a Value, field: &str) -> Option<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
}

fn optional_server_str<'a>(value: &'a Value, field: &str) -> Option<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

pub(super) fn model_catalog_event_from_server_value(value: &Value) -> Option<DesktopSessionEvent> {
    Some(DesktopSessionEvent::ModelCatalog {
        current_model: value
            .get("provider_model")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        provider_name: value
            .get("provider_name")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        models: model_choices_from_server_value(value),
        reasoning_effort: value
            .get("reasoning_effort")
            .and_then(Value::as_str)
            .filter(|effort| !effort.trim().is_empty())
            .map(ToOwned::to_owned),
        service_tier: value
            .get("service_tier")
            .and_then(Value::as_str)
            .filter(|tier| !tier.trim().is_empty())
            .map(ToOwned::to_owned),
        compaction_mode: value
            .get("compaction_mode")
            .and_then(Value::as_str)
            .filter(|mode| !mode.trim().is_empty())
            .map(ToOwned::to_owned),
    })
}

pub(super) fn model_choices_from_server_value(value: &Value) -> Vec<DesktopModelChoice> {
    let mut choices = Vec::new();
    if let Some(routes) = value
        .get("available_model_routes")
        .and_then(Value::as_array)
    {
        for route in routes {
            let Some(model) = route.get("model").and_then(Value::as_str) else {
                continue;
            };
            choices.push(DesktopModelChoice {
                model: model.to_string(),
                provider: route
                    .get("provider")
                    .and_then(Value::as_str)
                    .filter(|provider| !provider.is_empty())
                    .map(ToOwned::to_owned),
                api_method: route
                    .get("api_method")
                    .and_then(Value::as_str)
                    .filter(|method| !method.is_empty())
                    .map(ToOwned::to_owned),
                detail: route
                    .get("detail")
                    .and_then(Value::as_str)
                    .filter(|detail| !detail.is_empty())
                    .map(ToOwned::to_owned),
                available: route
                    .get("available")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
            });
        }
    }

    if choices.is_empty()
        && let Some(models) = value.get("available_models").and_then(Value::as_array)
    {
        for model in models.iter().filter_map(Value::as_str) {
            choices.push(DesktopModelChoice {
                model: model.to_string(),
                provider: None,
                api_method: None,
                detail: None,
                available: true,
            });
        }
    }

    choices
}

pub(super) fn compact_tool_output(output: &str) -> String {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return "done".to_string();
    }
    let single_line = trimmed.lines().next().unwrap_or(trimmed).trim();
    if single_line.chars().count() > 120 {
        format!("{}…", single_line.chars().take(120).collect::<String>())
    } else {
        single_line.to_string()
    }
}
