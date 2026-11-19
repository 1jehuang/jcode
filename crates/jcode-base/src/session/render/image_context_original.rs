// Frozen original render loop from 7e8ca3a2f, session/render.rs:365-724.
// Intentionally independent of the optimized renderer, sharing only unchanged helpers.
use super::*;

pub fn render_messages_and_images_with_compacted_history(
    session: &Session,
    compacted_history_visible: usize,
) -> (
    Vec<RenderedMessage>,
    Vec<RenderedImage>,
    Option<RenderedCompactedHistoryInfo>,
) {
    let mut rendered: Vec<RenderedMessage> = Vec::new();
    let mut images: Vec<RenderedImage> = Vec::new();
    let mut tool_map: HashMap<String, ToolCall> = HashMap::new();
    // 0-based ordinal of the next rendered user prompt, used to anchor pasted
    // user images to their prompt in the transcript.
    let mut user_prompt_count = 0usize;
    let compacted_count = session
        .compaction
        .as_ref()
        .map(|state| state.compacted_count.min(session.messages.len()))
        .unwrap_or(0);
    let (render_start_idx, compacted_info) = compacted_history_render_window(
        &session.messages,
        compacted_count,
        compacted_history_visible,
    );
    let compacted_info = (compacted_count > 0).then_some(compacted_info);

    if compacted_count > 0 {
        let visible_compacted = compacted_info
            .as_ref()
            .map(|info| info.visible_messages)
            .unwrap_or(0);
        let remaining_compacted = compacted_info
            .as_ref()
            .map(|info| info.remaining_messages)
            .unwrap_or(0);
        let total_compacted = compacted_info
            .as_ref()
            .map(|info| info.total_messages)
            .unwrap_or(0);
        let mut content = if remaining_compacted == 0 {
            format!(
                "Earlier conversation compacted - showing all {} compacted historical messages. Redraw may be slower while this view is open.",
                total_compacted
            )
        } else if visible_compacted == 0 {
            format!(
                "Earlier conversation compacted - {} historical messages hidden from the UI. Scroll to the top to load older history.",
                remaining_compacted
            )
        } else {
            format!(
                "Earlier conversation compacted - {} older historical messages hidden. Showing {} of {} compacted messages. Scroll to the top to load more.",
                remaining_compacted, visible_compacted, total_compacted
            )
        };
        // History can hydrate a remote client with only a metadata stub. Carry
        // the authoritative prompt offset alongside the existing lazy-window
        // marker rather than asking that client to count its stale local copy.
        if let Some(info) = &compacted_info {
            content.push_str(&format!(
                " Hidden user prompts: {}.",
                info.hidden_user_prompts
            ));
        }
        rendered.push(RenderedMessage {
            response_stats: None,
            tool_duration_ms: None,
            role: "system".to_string(),
            content,
            tool_calls: Vec::new(),
            tool_data: None,
            stored_index: None,
        });
    }

    // Error replay records are display-only: never turn provider diagnostics
    // into assistant/user context. Merge them at their original turn boundary
    // so a reconnect shows the failure beneath its wake, not at the latest turn.
    let mut errors: Vec<_> = session
        .replay_events
        .iter()
        .filter_map(|event| match &event.kind {
            crate::session::StoredReplayEventKind::DisplayMessage { role, content, .. }
                if role == "error" =>
            {
                Some((event.timestamp, content))
            }
            _ => None,
        })
        .filter(|(timestamp, _)| {
            render_start_idx == 0
                || session
                    .messages
                    .get(render_start_idx)
                    .and_then(|msg| msg.timestamp)
                    .is_none_or(|start| *timestamp >= start)
        })
        .collect();
    errors.sort_by_key(|(timestamp, _)| *timestamp);
    let mut errors = errors.into_iter().peekable();
    let render_error = |content: &String| RenderedMessage {
        response_stats: None,
        tool_duration_ms: None,
        role: "error".to_string(),
        content: content.clone(),
        tool_calls: Vec::new(),
        tool_data: None,
        stored_index: None,
    };
    for (stored_index, msg) in session.messages.iter().enumerate().skip(render_start_idx) {
        while errors
            .peek()
            .is_some_and(|(timestamp, _)| msg.timestamp.is_some_and(|next| *timestamp < next))
        {
            if let Some((_, content)) = errors.next() {
                rendered.push(render_error(content));
            }
        }
        if is_internal_system_reminder(msg) {
            continue;
        }

        let role = match msg.display_role {
            Some(StoredDisplayRole::JarvisUser) => "jarvis_user",
            Some(StoredDisplayRole::System) => "system",
            Some(StoredDisplayRole::BackgroundTask) => "background_task",
            Some(StoredDisplayRole::FileActivity) => "file_activity",
            None if is_auto_poke_user_message(msg) => "system",
            None if crate::session::is_scheduled_task_message(msg) => "system",
            None => match msg.role {
                Role::User => "user",
                Role::Assistant => "assistant",
            },
        };
        // Gate continuations are model-facing instructions naming specific
        // todos and fields. In the transcript the user only wants to know a
        // check happened, so replace the body with a one-liner.
        if let Some(summary) = auto_poke_user_message_display_summary(msg) {
            rendered.push(RenderedMessage {
                response_stats: None,
                tool_duration_ms: None,
                role: "system".to_string(),
                content: summary.to_string(),
                tool_calls: Vec::new(),
                tool_data: None,
                stored_index: Some(stored_index),
            });
            continue;
        }
        let message_role = msg.role.clone();
        let mut text = String::new();
        // Reasoning is accumulated separately so it can be rendered *before* the
        // answer text, matching the live streaming order. Providers persist the
        // assistant turn as `[Text, ReasoningTrace, ToolUse]`, so appending
        // reasoning into `text` in block order would otherwise show the thinking
        // *after* the answer on resume/re-render.
        let mut reasoning = String::new();
        let mut tool_calls: Vec<String> = Vec::new();
        let mut current_tool: Option<ToolCall> = None;
        let mut last_image_idx: Option<usize> = None;
        // Images from blocks with no owning tool result (e.g. a pasted user
        // screenshot). Their user-prompt ordinal is only known once we know the
        // message actually renders a user prompt, so patch them afterwards.
        let mut pending_prompt_image_indices: Vec<usize> = Vec::new();

        for block in &msg.content {
            match block {
                ContentBlock::Text { text: t, .. } => {
                    // The `[Attached image associated with the preceding tool
                    // result: ...]` block is synthetic metadata jcode injects so
                    // the model can associate a label with the image. It lives in
                    // the same (user) turn as the tool result, so if we rendered
                    // it as message text it would surface as a bogus user prompt
                    // (showing up as the "last prompt" instead of the user's real
                    // message). Consume it into the image label and never display
                    // it.
                    if let Some(label) = parse_attached_image_label(t) {
                        if let Some(last_idx) = last_image_idx
                            && let Some(image) = images.get_mut(last_idx)
                        {
                            image.label = Some(label);
                        }
                        continue;
                    }
                    text.push_str(t);
                }
                ContentBlock::ToolUse {
                    id,
                    name,
                    input,
                    thought_signature,
                } => {
                    let tool_call = ToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                        intent: ToolCall::intent_from_input(input),
                        thought_signature: thought_signature.clone(),
                    };
                    tool_map.insert(id.clone(), tool_call);
                    tool_calls.push(name.clone());
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } => {
                    let combined = format!("{}{}", reasoning, text);
                    if !combined.is_empty() {
                        if role == "user" && !is_attached_image_label_text(&text) {
                            user_prompt_count += 1;
                        }
                        text.clear();
                        reasoning.clear();
                        rendered.push(RenderedMessage {
                            response_stats: None,
                            tool_duration_ms: None,
                            role: role.to_string(),
                            content: combined,
                            tool_calls: tool_calls.clone(),
                            tool_data: None,
                            stored_index: Some(stored_index),
                        });
                    }

                    let tool_data = tool_map.get(tool_use_id).cloned().or_else(|| {
                        Some(ToolCall {
                            id: tool_use_id.clone(),
                            name: "tool".to_string(),
                            input: serde_json::Value::Null,
                            intent: None,
                            thought_signature: None,
                        })
                    });
                    current_tool = tool_data.clone();

                    rendered.push(RenderedMessage {
                        response_stats: None,
                        tool_duration_ms: msg.tool_duration_ms,
                        role: "tool".to_string(),
                        content: content.clone(),
                        tool_calls: Vec::new(),
                        tool_data,
                        stored_index: Some(stored_index),
                    });
                }
                ContentBlock::Reasoning { text: t } | ContentBlock::ReasoningTrace { text: t } => {
                    reasoning.push_str(&format_reasoning_markup(t));
                }
                ContentBlock::AnthropicThinking { .. } | ContentBlock::OpenAIReasoning { .. } => {}
                ContentBlock::Image { media_type, data } => {
                    let anchor =
                        image_anchor_for_message(role, current_tool.as_ref(), user_prompt_count);
                    let is_pending_prompt_anchor = current_tool.is_none() && role == "user";
                    images.push(RenderedImage {
                        history_message_index: current_tool.as_ref().map(|_| rendered.len()),
                        media_type: media_type.clone(),
                        data: data.clone(),
                        label: current_tool
                            .as_ref()
                            .and_then(fallback_image_label_for_tool),
                        source: image_source_for_message(
                            message_role.clone(),
                            current_tool.as_ref(),
                        ),
                        anchor,
                    });
                    last_image_idx = Some(images.len().saturating_sub(1));
                    if is_pending_prompt_anchor {
                        pending_prompt_image_indices.push(images.len() - 1);
                    }
                }
                ContentBlock::ProviderNative { provider, item } => {
                    use jcode_message_types::provider_native::provider_native_display;
                    let Some(display) = provider_native_display(provider, item) else {
                        continue;
                    };
                    let Some(output) = display.output else {
                        // Call start: remember the input for the result row.
                        let input = display.input.unwrap_or(serde_json::Value::Null);
                        tool_map.insert(
                            display.id.clone(),
                            ToolCall {
                                intent: ToolCall::intent_from_input(&input),
                                id: display.id,
                                name: display.name,
                                input,
                                thought_signature: None,
                            },
                        );
                        continue;
                    };
                    // Flush text streamed before the search so order matches live.
                    let combined = format!("{}{}", reasoning, text);
                    if !combined.is_empty() {
                        text.clear();
                        reasoning.clear();
                        rendered.push(RenderedMessage {
                            response_stats: None,
                            tool_duration_ms: None,
                            role: role.to_string(),
                            content: combined,
                            tool_calls: std::mem::take(&mut tool_calls),
                            tool_data: None,
                            stored_index: Some(stored_index),
                        });
                    }
                    let tool_data = tool_map.get(&display.id).cloned().unwrap_or_else(|| {
                        let input = display.input.clone().unwrap_or(serde_json::Value::Null);
                        ToolCall {
                            intent: ToolCall::intent_from_input(&input),
                            id: display.id.clone(),
                            name: display.name.clone(),
                            input,
                            thought_signature: None,
                        }
                    });
                    rendered.push(RenderedMessage {
                        response_stats: None,
                        tool_duration_ms: None,
                        role: "tool".to_string(),
                        content: output,
                        tool_calls: Vec::new(),
                        tool_data: Some(tool_data),
                        stored_index: Some(stored_index),
                    });
                }
                ContentBlock::OpenAICompaction { .. } | ContentBlock::ToolReference { .. } => {}
            }
        }

        let combined = format!("{}{}", reasoning, text);
        if !combined.is_empty() {
            if role == "user" && !is_attached_image_label_text(&text) {
                user_prompt_count += 1;
            }
            rendered.push(RenderedMessage {
                response_stats: None,
                tool_duration_ms: None,
                role: role.to_string(),
                content: combined,
                tool_calls,
                tool_data: None,
                stored_index: Some(stored_index),
            });
        } else if !pending_prompt_image_indices.is_empty() {
            // The message carried images but produced no rendered user prompt;
            // drop the anchor so these images fall back to the transcript tail
            // instead of pointing at the wrong prompt.
            for idx in pending_prompt_image_indices {
                if let Some(image) = images.get_mut(idx) {
                    image.anchor = None;
                }
            }
        }
    }

    rendered.extend(errors.map(|(_, content)| render_error(content)));
    response_stats::attach(&session.messages, &mut rendered);
    (rendered, images, compacted_info)
}
