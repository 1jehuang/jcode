use super::*;

fn compact_tool_input_for_display(name: &str, input: &serde_json::Value) -> serde_json::Value {
    let obj = |pairs: Vec<(&str, serde_json::Value)>| {
        let mut map = serde_json::Map::new();
        for (key, value) in pairs {
            if !value.is_null() {
                map.insert(key.to_string(), value);
            }
        }
        serde_json::Value::Object(map)
    };

    match crate::tui::ui::tools_ui::canonical_tool_name(name) {
        "bash" => obj(vec![(
            "command",
            input
                .get("command")
                .and_then(|v| v.as_str())
                .map(|s| serde_json::Value::String(crate::util::truncate_str(s, 160).to_string()))
                .unwrap_or(serde_json::Value::Null),
        )]),
        "read" => obj(vec![
            (
                "file_path",
                input
                    .get("file_path")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            ),
            (
                "start_line",
                input
                    .get("start_line")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            ),
            (
                "end_line",
                input
                    .get("end_line")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            ),
            (
                "offset",
                input
                    .get("offset")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            ),
            (
                "limit",
                input
                    .get("limit")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            ),
        ]),
        "write" | "edit" | "multiedit" => obj(vec![(
            "file_path",
            input
                .get("file_path")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        )]),
        "patch" | "apply_patch" => {
            let file_path = input.get("file_path").cloned().or_else(|| {
                input
                    .get("patch_text")
                    .and_then(|v| v.as_str())
                    .and_then(|patch_text| {
                        match crate::tui::ui::tools_ui::canonical_tool_name(name) {
                            "apply_patch" => {
                                crate::tui::ui::tools_ui::extract_apply_patch_primary_file(
                                    patch_text,
                                )
                            }
                            "patch" => {
                                crate::tui::ui::tools_ui::extract_unified_patch_primary_file(
                                    patch_text,
                                )
                            }
                            _ => None,
                        }
                    })
                    .map(serde_json::Value::String)
            });
            obj(vec![(
                "file_path",
                file_path.unwrap_or(serde_json::Value::Null),
            )])
        }
        "glob" => obj(vec![(
            "pattern",
            input
                .get("pattern")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        )]),
        "grep" => obj(vec![
            (
                "pattern",
                input
                    .get("pattern")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            ),
            (
                "path",
                input
                    .get("path")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            ),
        ]),
        "agentgrep" => obj(vec![
            (
                "mode",
                input
                    .get("mode")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            ),
            (
                "query",
                input
                    .get("query")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            ),
            (
                "terms",
                input
                    .get("terms")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            ),
        ]),
        "memory" => obj(vec![
            (
                "action",
                input
                    .get("action")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            ),
            (
                "category",
                input
                    .get("category")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            ),
            (
                "tag",
                input.get("tag").cloned().unwrap_or(serde_json::Value::Null),
            ),
            (
                "query",
                input
                    .get("query")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            ),
            (
                "content",
                input
                    .get("content")
                    .and_then(|v| v.as_str())
                    .map(|s| {
                        serde_json::Value::String(crate::util::truncate_str(s, 240).to_string())
                    })
                    .unwrap_or(serde_json::Value::Null),
            ),
        ]),
        "batch" => {
            let tool_calls = input
                .get("tool_calls")
                .and_then(|v| v.as_array())
                .map(|calls| {
                    calls
                        .iter()
                        .map(|call| {
                            let raw_name = call
                                .get("tool")
                                .or_else(|| call.get("name"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("?");
                            let params = crate::tui::ui::tools_ui::batch_subcall_params(call);
                            let sub_tool = crate::message::ToolCall {
                                id: String::new(),
                                name: crate::tui::ui::tools_ui::resolve_display_tool_name(raw_name)
                                    .to_string(),
                                input: params.clone(),
                                intent: crate::message::ToolCall::intent_from_input(&params),
                            };
                            let summary = crate::tui::ui::tools_ui::get_tool_summary(&sub_tool);
                            let compacted = compact_tool_input_for_display(raw_name, &params);
                            let mut entry = serde_json::Map::new();
                            entry.insert(
                                "tool".to_string(),
                                serde_json::Value::String(raw_name.to_string()),
                            );
                            if !summary.is_empty() {
                                entry.insert(
                                    "intent".to_string(),
                                    serde_json::Value::String(summary),
                                );
                            }
                            if let Some(compacted_obj) = compacted.as_object() {
                                for (key, value) in compacted_obj {
                                    entry.insert(key.clone(), value.clone());
                                }
                            }
                            serde_json::Value::Object(entry)
                        })
                        .collect::<Vec<_>>()
                })
                .map(serde_json::Value::Array)
                .unwrap_or(serde_json::Value::Null);
            obj(vec![("tool_calls", tool_calls)])
        }
        _ => serde_json::Value::Object(serde_json::Map::new()),
    }
}

pub(crate) fn compact_display_message_tool_data(message: &mut DisplayMessage) {
    let Some(tool) = message.tool_data.as_mut() else {
        return;
    };
    tool.input = compact_tool_input_for_display(tool.name.as_str(), &tool.input);
}

pub(crate) fn compact_display_messages_for_storage(messages: &mut [DisplayMessage]) {
    for message in messages {
        compact_display_message_tool_data(message);
    }
}

pub(super) fn infer_spawned_session_startup_hints(
    message: &str,
) -> Option<(String, (String, String))> {
    let label = if message.starts_with("You are the automatic reviewer for parent session `") {
        "Autoreview"
    } else if message.starts_with("You are the automatic judge for parent session `") {
        "Autojudge"
    } else if message.starts_with("You are the one-shot reviewer for parent session `") {
        "Review"
    } else if message.starts_with("You are the one-shot judge for parent session `") {
        "Judge"
    } else {
        return None;
    };

    let parent_session_id = message.split('`').nth(1).unwrap_or("parent");
    let body = if label == "Autojudge" {
        format!(
            "🔍 {} session started for parent `{}`.\n\nThis session is analysis-only: it will inspect the recent work, send exactly one DM back telling the parent either to `CONTINUE:` with specific next steps or `STOP:` because the work is complete, and then stop. It should not continue the work or modify repo state.\n\nJudge sessions use a user-visible mirror of the parent conversation: user prompts, visible assistant replies, and shallow tool-call summaries — not the parent's full hidden tool context.",
            label, parent_session_id
        )
    } else {
        format!(
            "🔍 {} session started for parent `{}`.\n\nThis session is analysis-only: it will inspect the recent work, send exactly one DM back to the parent session, and stop. It should not continue the work or modify repo state.\n\nJudge sessions use a user-visible mirror of the parent conversation: user prompts, visible assistant replies, and shallow tool-call summaries — not the parent's full hidden tool context.",
            label, parent_session_id
        )
    };

    Some((format!("{} starting", label), (label.to_string(), body)))
}
