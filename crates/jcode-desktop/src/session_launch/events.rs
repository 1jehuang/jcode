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
        "status_detail" => value
            .get("detail")
            .and_then(Value::as_str)
            .map(|detail| DesktopSessionEvent::Status(DesktopSessionStatus::external(detail))),
        "tool_start" => {
            value
                .get("name")
                .and_then(Value::as_str)
                .map(|name| DesktopSessionEvent::ToolStarted {
                    name: name.to_string(),
                })
        }
        "tool_exec" => value.get("name").and_then(Value::as_str).map(|name| {
            DesktopSessionEvent::ToolExecuting {
                name: name.to_string(),
            }
        }),
        "tool_input" => {
            value
                .get("delta")
                .and_then(Value::as_str)
                .map(|delta| DesktopSessionEvent::ToolInput {
                    delta: delta.to_string(),
                })
        }
        "tool_done" => value.get("name").and_then(Value::as_str).map(|name| {
            DesktopSessionEvent::ToolFinished {
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

fn non_empty_server_str<'a>(value: &'a Value, field: &str) -> Option<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
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

pub(super) fn history_reasoning_effort_from_server_value(value: &Value) -> Option<String> {
    value
        .get("reasoning_effort")
        .and_then(Value::as_str)
        .or_else(|| value.get("openai_reasoning_effort").and_then(Value::as_str))
        .or_else(|| {
            value
                .get("provider_config")
                .and_then(|config| config.get("openai_reasoning_effort"))
                .and_then(Value::as_str)
        })
        .filter(|effort| !effort.trim().is_empty())
        .map(ToOwned::to_owned)
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
