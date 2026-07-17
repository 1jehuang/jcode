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
        // Web/search tools: keep the URL/query so the transcript row still
        // shows what was fetched or searched after storage compaction.
        "webfetch" => obj(vec![(
            "url",
            input
                .get("url")
                .and_then(|v| v.as_str())
                .map(|s| serde_json::Value::String(crate::util::truncate_str(s, 200).to_string()))
                .unwrap_or(serde_json::Value::Null),
        )]),
        "websearch" | "codesearch" | "session_search" | "conversation_search" => obj(vec![(
            "query",
            input
                .get("query")
                .and_then(|v| v.as_str())
                .map(|s| serde_json::Value::String(crate::util::truncate_str(s, 200).to_string()))
                .unwrap_or(serde_json::Value::Null),
        )]),
        "open" => obj(vec![
            (
                "action",
                input
                    .get("action")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            ),
            (
                "target",
                input
                    .get("target")
                    .and_then(|v| v.as_str())
                    .map(|s| {
                        serde_json::Value::String(crate::util::truncate_str(s, 200).to_string())
                    })
                    .unwrap_or(serde_json::Value::Null),
            ),
        ]),
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
        // Swarm rows: keep the action and routing fields so the transcript
        // summary ("spawn '...'", "dm → worker-1", ...) survives compaction.
        "swarm" => obj(vec![
            (
                "action",
                input
                    .get("action")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            ),
            (
                "to_session",
                input
                    .get("to_session")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            ),
            (
                "target_session",
                input
                    .get("target_session")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            ),
            (
                "channel",
                input
                    .get("channel")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            ),
            (
                "working_dir",
                input
                    .get("working_dir")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            ),
            (
                "task_id",
                input
                    .get("task_id")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            ),
            (
                "prompt",
                input
                    .get("prompt")
                    .and_then(|v| v.as_str())
                    .map(|s| {
                        serde_json::Value::String(crate::util::truncate_str(s, 160).to_string())
                    })
                    .unwrap_or(serde_json::Value::Null),
            ),
            (
                "message",
                input
                    .get("message")
                    .and_then(|v| v.as_str())
                    .map(|s| {
                        serde_json::Value::String(crate::util::truncate_str(s, 160).to_string())
                    })
                    .unwrap_or(serde_json::Value::Null),
            ),
        ]),
        // Gmail/browser rows: keep the action plus the small routing fields
        // that get_tool_summary_with_budget renders, so the transcript summary
        // survives storage compaction and session reload.
        "gmail" => obj(vec![
            (
                "action",
                input
                    .get("action")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            ),
            (
                "query",
                input
                    .get("query")
                    .and_then(|v| v.as_str())
                    .map(|s| {
                        serde_json::Value::String(crate::util::truncate_str(s, 200).to_string())
                    })
                    .unwrap_or(serde_json::Value::Null),
            ),
            (
                "message_id",
                input
                    .get("message_id")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            ),
            (
                "thread_id",
                input
                    .get("thread_id")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            ),
            (
                "draft_id",
                input
                    .get("draft_id")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            ),
            (
                "to",
                input.get("to").cloned().unwrap_or(serde_json::Value::Null),
            ),
            (
                "subject",
                input
                    .get("subject")
                    .and_then(|v| v.as_str())
                    .map(|s| {
                        serde_json::Value::String(crate::util::truncate_str(s, 120).to_string())
                    })
                    .unwrap_or(serde_json::Value::Null),
            ),
            // Keep the draft/send body so the Gmail draft card still renders
            // its content after storage compaction (previously dropped, which
            // made every reloaded draft card show "(empty body)").
            (
                "body",
                input
                    .get("body")
                    .and_then(|v| v.as_str())
                    .map(|s| {
                        serde_json::Value::String(crate::util::truncate_str(s, 4000).to_string())
                    })
                    .unwrap_or(serde_json::Value::Null),
            ),
            (
                "attachments",
                input
                    .get("attachments")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            ),
        ]),
        "browser" => obj(vec![
            (
                "action",
                input
                    .get("action")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            ),
            (
                "url",
                input
                    .get("url")
                    .and_then(|v| v.as_str())
                    .map(|s| {
                        serde_json::Value::String(crate::util::truncate_str(s, 200).to_string())
                    })
                    .unwrap_or(serde_json::Value::Null),
            ),
            (
                "selector",
                input
                    .get("selector")
                    .and_then(|v| v.as_str())
                    .map(|s| {
                        serde_json::Value::String(crate::util::truncate_str(s, 120).to_string())
                    })
                    .unwrap_or(serde_json::Value::Null),
            ),
            (
                "format",
                input
                    .get("format")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            ),
            (
                "key",
                input.get("key").cloned().unwrap_or(serde_json::Value::Null),
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
                            let intent =
                                crate::tui::ui::tools_ui::batch_subcall_intent(call, &params);
                            let compacted = compact_tool_input_for_display(raw_name, &params);
                            let mut entry = serde_json::Map::new();
                            entry.insert(
                                "tool".to_string(),
                                serde_json::Value::String(raw_name.to_string()),
                            );
                            if let Some(intent) = intent {
                                entry.insert(
                                    "intent".to_string(),
                                    serde_json::Value::String(intent),
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
        // Generic fallback: keep the action field (when present) so
        // action-shaped tool rows still render a summary after reload.
        _ => obj(vec![(
            "action",
            input
                .get("action")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        )]),
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
            "🔍 {} session started for parent `{}`.\n\nThis session is analysis-only: it will inspect the recent work, send exactly one DM back telling the parent either to `CONTINUE:` with specific next steps or `STOP:` because the work is complete, and then stop. It should not continue the work or modify repo state.\n\nJudge sessions use a user-visible mirror of the parent conversation: user prompts, visible assistant replies, and shallow tool-call summaries - not the parent's full hidden tool context.",
            label, parent_session_id
        )
    } else {
        format!(
            "🔍 {} session started for parent `{}`.\n\nThis session is analysis-only: it will inspect the recent work, send exactly one DM back to the parent session, and stop. It should not continue the work or modify repo state.\n\nJudge sessions use a user-visible mirror of the parent conversation: user prompts, visible assistant replies, and shallow tool-call summaries - not the parent's full hidden tool context.",
            label, parent_session_id
        )
    };

    Some((format!("{} starting", label), (label.to_string(), body)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool_message(name: &str, input: serde_json::Value) -> DisplayMessage {
        DisplayMessage {
            role: "tool".to_string(),
            content: "output".to_string(),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: Some(crate::message::ToolCall {
                id: "call-1".to_string(),
                name: name.to_string(),
                input: input.clone(),
                intent: crate::message::ToolCall::intent_from_input(&input),
                thought_signature: None,
            }),
        }
    }

    #[test]
    fn compaction_keeps_webfetch_url_for_transcript_summary() {
        let mut message = tool_message(
            "webfetch",
            serde_json::json!({
                "url": "https://example.com/docs/api",
                "format": "markdown",
                "timeout": 30
            }),
        );
        compact_display_message_tool_data(&mut message);
        let tool = message.tool_data.expect("tool data");
        assert_eq!(
            tool.input.get("url").and_then(|v| v.as_str()),
            Some("https://example.com/docs/api")
        );
        let summary = crate::tui::ui::tools_ui::get_tool_summary(&tool);
        assert!(
            summary.contains("example.com"),
            "summary should surface the fetched URL: {summary:?}"
        );
    }

    #[test]
    fn compaction_keeps_websearch_query_for_transcript_summary() {
        let mut message = tool_message(
            "websearch",
            serde_json::json!({ "query": "rust async traits", "num_results": 5 }),
        );
        compact_display_message_tool_data(&mut message);
        let tool = message.tool_data.expect("tool data");
        assert_eq!(
            tool.input.get("query").and_then(|v| v.as_str()),
            Some("rust async traits")
        );
        let summary = crate::tui::ui::tools_ui::get_tool_summary(&tool);
        assert!(
            summary.contains("rust async traits"),
            "summary should surface the search query: {summary:?}"
        );
    }

    #[test]
    fn compaction_preserves_model_provided_intent() {
        let mut message = tool_message(
            "webfetch",
            serde_json::json!({
                "intent": "Check release notes",
                "url": "https://example.com/releases"
            }),
        );
        compact_display_message_tool_data(&mut message);
        let tool = message.tool_data.expect("tool data");
        assert_eq!(tool.intent.as_deref(), Some("Check release notes"));
    }

    #[test]
    fn compaction_preserves_flat_and_nested_batch_subcall_intents() {
        let mut message = tool_message(
            "batch",
            serde_json::json!({
                "tool_calls": [
                    {
                        "tool": "read",
                        "intent": "Inspect flat batch input",
                        "file_path": "src/flat.rs"
                    },
                    {
                        "tool": "read",
                        "parameters": {
                            "intent": "Inspect nested batch input",
                            "file_path": "src/nested.rs"
                        }
                    },
                    {
                        "tool": "read",
                        "file_path": "src/no-intent.rs"
                    }
                ]
            }),
        );

        compact_display_message_tool_data(&mut message);
        let calls = message
            .tool_data
            .expect("tool data")
            .input
            .get("tool_calls")
            .and_then(|value| value.as_array())
            .cloned()
            .expect("batch calls");

        assert_eq!(calls[0]["intent"], "Inspect flat batch input");
        assert_eq!(calls[1]["intent"], "Inspect nested batch input");
        assert!(calls[2].get("intent").is_none());
        assert_eq!(calls[0]["file_path"], "src/flat.rs");
        assert_eq!(calls[1]["file_path"], "src/nested.rs");
    }

    #[test]
    fn compaction_keeps_swarm_action_and_intent_for_transcript_summary() {
        let mut message = tool_message(
            "swarm",
            serde_json::json!({
                "intent": "Spin up a parser-fix worker",
                "action": "spawn",
                "prompt": "Fix the parser bug in crates/parser and add tests",
                "spawn_mode": "inline"
            }),
        );
        compact_display_message_tool_data(&mut message);
        let tool = message.tool_data.expect("tool data");
        assert_eq!(tool.intent.as_deref(), Some("Spin up a parser-fix worker"));
        assert_eq!(
            tool.input.get("action").and_then(|v| v.as_str()),
            Some("spawn")
        );
        let summary = crate::tui::ui::tools_ui::get_tool_summary(&tool);
        assert!(
            summary.contains("spawn"),
            "summary should surface the swarm action: {summary:?}"
        );
        assert!(
            summary.contains("Fix the parser bug"),
            "summary should keep the spawn prompt: {summary:?}"
        );
    }

    #[test]
    fn compaction_keeps_swarm_dm_target_for_transcript_summary() {
        let mut message = tool_message(
            "swarm",
            serde_json::json!({
                "action": "dm",
                "to_session": "worker-1",
                "message": "status update please",
                "delivery": "notify"
            }),
        );
        compact_display_message_tool_data(&mut message);
        let tool = message.tool_data.expect("tool data");
        let summary = crate::tui::ui::tools_ui::get_tool_summary(&tool);
        assert!(
            summary.contains("dm") && summary.contains("worker-1"),
            "summary should keep the dm target: {summary:?}"
        );
    }

    #[test]
    fn compaction_keeps_gmail_action_and_intent_for_transcript_summary() {
        let mut message = tool_message(
            "gmail",
            serde_json::json!({
                "intent": "Check unread mail",
                "action": "search",
                "query": "is:unread",
                "max_results": 10
            }),
        );
        compact_display_message_tool_data(&mut message);
        let tool = message.tool_data.expect("tool data");
        assert_eq!(tool.intent.as_deref(), Some("Check unread mail"));
        let summary = crate::tui::ui::tools_ui::get_tool_summary(&tool);
        assert!(
            summary.contains("search") && summary.contains("is:unread"),
            "summary should keep the gmail action and query: {summary:?}"
        );
    }

    #[test]
    fn compaction_keeps_gmail_draft_body_and_attachments_for_draft_card() {
        let mut message = tool_message(
            "gmail",
            serde_json::json!({
                "action": "draft",
                "to": "someone@example.com",
                "subject": "Hello",
                "body": "Dear someone,\n\nThis is the body.\n",
                "attachments": ["/tmp/lease.pdf"]
            }),
        );
        compact_display_message_tool_data(&mut message);
        let tool = message.tool_data.expect("tool data");
        assert_eq!(
            tool.input.get("body").and_then(|v| v.as_str()),
            Some("Dear someone,\n\nThis is the body.\n"),
            "draft body must survive compaction so the draft card doesn't render '(empty body)'"
        );
        assert_eq!(
            tool.input
                .get("attachments")
                .and_then(|v| v.as_array())
                .map(|a| a.len()),
            Some(1),
            "attachments must survive compaction"
        );
    }

    #[test]
    fn compaction_keeps_browser_action_and_intent_for_transcript_summary() {
        let mut message = tool_message(
            "browser",
            serde_json::json!({
                "intent": "Open docs page",
                "action": "open",
                "url": "https://example.com/docs",
                "new_tab": true
            }),
        );
        compact_display_message_tool_data(&mut message);
        let tool = message.tool_data.expect("tool data");
        assert_eq!(tool.intent.as_deref(), Some("Open docs page"));
        let summary = crate::tui::ui::tools_ui::get_tool_summary(&tool);
        assert!(
            summary.contains("open") && summary.contains("example.com"),
            "summary should keep the browser action and url: {summary:?}"
        );
    }
}
