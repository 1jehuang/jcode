use super::{Session, StoredDisplayRole};
use crate::message::{ContentBlock, Role, ToolCall};
use jcode_config_types::ReasoningDisplayMode;
pub use jcode_session_types::{
    RenderedCompactedHistoryInfo, RenderedImage, RenderedImageAnchor, RenderedImageSource,
    RenderedMessage,
};
use std::collections::HashMap;

/// Number of compacted historical messages shown by default in the UI.
///
/// Compaction still keeps older history out of the active model context, but
/// the transcript should retain recent continuity instead of replacing the
/// entire compacted prefix with a marker.
pub const DEFAULT_VISIBLE_COMPACTED_HISTORY_MESSAGES: usize = 64;

/// Format persisted reasoning/thinking text into the dim+italic markdown used
/// by the live streaming path. Each line is wrapped via the shared `reasoning_line_markup` so resumed
/// sessions render reasoning identically to how it streamed, terminated by a
/// blank line so following answer text renders as a normal paragraph.
///
/// Honors the active `reasoning_display` mode so re-rendered history (reload,
/// resume, remote sync, compaction-window expand) matches the live behavior:
/// - `Off`: persisted reasoning is hidden entirely.
/// - `Current`: only the *live* reasoning block is ever shown, so historical
///   reasoning is hidden on re-render (the live block already streamed and was
///   discarded once the model answered), matching the ephemeral live behavior.
/// - `Full`: every reasoning line is shown (classic behavior).
fn format_reasoning_markup(text: &str) -> String {
    if text.trim().is_empty() {
        return String::new();
    }
    let mode = crate::config::config().display.reasoning_display();
    match mode {
        // In both `Off` and `Current` modes persisted reasoning is not re-rendered:
        // `Current` only ever shows the live block, which is discarded once the
        // model answers, so reloaded history shows no past reasoning.
        ReasoningDisplayMode::Off | ReasoningDisplayMode::Current => return String::new(),
        ReasoningDisplayMode::Full => {}
    }
    let mut out = String::new();
    for line in text.split('\n') {
        out.push_str(&jcode_render_core::reasoning_line_markup(line));
    }
    // Blank line terminates the reasoning block.
    out.push('\n');
    out
}

fn is_internal_system_reminder(msg: &super::StoredMessage) -> bool {
    msg.content
        .iter()
        .find_map(|block| match block {
            ContentBlock::Text { text, .. } => Some(text.trim_start()),
            _ => None,
        })
        .is_some_and(|text| text.starts_with("<system-reminder>"))
}

/// True when a stored user message is a synthetic auto-poke continuation
/// (incomplete-todos poke or todo confidence summary). These are persisted as
/// `Role::User` so the model treats them as a normal continuation turn, but
/// the live UI never shows them as user prompts (it shows an "Auto-poking..."
/// notice instead). Re-rendered history must not resurrect them as the user's
/// "last prompt" after a reload/resume/remote attach, so they render with the
/// system role.
fn is_auto_poke_user_message(msg: &super::StoredMessage) -> bool {
    matches!(msg.role, Role::User)
        && msg.display_role.is_none()
        && msg
            .content
            .iter()
            .find_map(|block| match block {
                ContentBlock::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .is_some_and(crate::todo::is_auto_poke_message)
}

/// Short user-facing stand-in for a synthetic gate continuation, when one
/// exists. `None` means render the stored text as-is.
fn auto_poke_user_message_display_summary(msg: &super::StoredMessage) -> Option<&'static str> {
    if !is_auto_poke_user_message(msg) {
        return None;
    }
    msg.content
        .iter()
        .find_map(|block| match block {
            ContentBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .and_then(crate::todo::auto_poke_display_summary)
}

fn stored_message_renders_visible_message(msg: &super::StoredMessage) -> bool {
    if is_internal_system_reminder(msg) {
        return false;
    }

    msg.content.iter().any(|block| match block {
        ContentBlock::Text { text, .. } => !text.is_empty(),
        ContentBlock::ToolResult { .. } => true,
        _ => false,
    })
}

/// A compacted prefix is only truncated when it is genuinely large. Below this
/// many renderable messages we always show the whole prefix, even if the
/// `requested_visible` window is smaller, so short histories stay intact.
const COMPACTED_HISTORY_MIN_RENDERABLE_TO_TRUNCATE: usize = 80;

/// We never truncate a compacted prefix that contains this many user turns or
/// fewer. This guarantees a single very long turn (1 turn, possibly hundreds of
/// tool messages) is never cut off, and short multi-turn histories stay whole.
const COMPACTED_HISTORY_MIN_TURNS_TO_TRUNCATE: usize = 5;

fn stored_message_is_user_turn(msg: &super::StoredMessage) -> bool {
    matches!(msg.role, Role::User)
        && msg.display_role.is_none()
        && !is_auto_poke_user_message(msg)
        && stored_message_renders_visible_message(msg)
}

fn compacted_history_render_window(
    messages: &[super::StoredMessage],
    compacted_count: usize,
    requested_visible: usize,
) -> (usize, RenderedCompactedHistoryInfo) {
    let compacted_count = compacted_count.min(messages.len());
    let compacted_prefix = &messages[..compacted_count];
    let total_renderable = compacted_prefix
        .iter()
        .filter(|msg| stored_message_renders_visible_message(msg))
        .count();
    let total_turns = compacted_prefix
        .iter()
        .filter(|msg| stored_message_is_user_turn(msg))
        .count();

    // Guardrails: only truncate when the prefix is BOTH very long AND has more
    // than a handful of turns. Otherwise show everything. This avoids cutting a
    // single long turn or a short multi-turn history.
    let must_show_all = total_renderable < COMPACTED_HISTORY_MIN_RENDERABLE_TO_TRUNCATE
        || total_turns <= COMPACTED_HISTORY_MIN_TURNS_TO_TRUNCATE;

    let visible_renderable = if must_show_all {
        total_renderable
    } else {
        requested_visible.min(total_renderable)
    };
    let remaining_renderable = total_renderable.saturating_sub(visible_renderable);

    let mut render_start_idx = if visible_renderable == 0 {
        compacted_count
    } else if remaining_renderable == 0 {
        0
    } else {
        let mut seen = 0usize;
        let mut start_idx = compacted_count;
        for (idx, msg) in compacted_prefix.iter().enumerate().rev() {
            if stored_message_renders_visible_message(msg) {
                seen += 1;
                if seen >= visible_renderable {
                    start_idx = idx;
                    break;
                }
            }
        }
        start_idx
    };

    // Snap the start back to a user-turn boundary so the visible window begins
    // at the start of a prompt. This keeps prompt numbering and turn grouping
    // coherent (we never render a half turn at the top).
    if render_start_idx > 0 && render_start_idx < compacted_count {
        let mut boundary = render_start_idx;
        while boundary > 0 && !stored_message_is_user_turn(&compacted_prefix[boundary]) {
            boundary -= 1;
        }
        if stored_message_is_user_turn(&compacted_prefix[boundary]) {
            render_start_idx = boundary;
        }
    }

    // Recompute visible/remaining/hidden after snapping so the reported counts
    // match what is actually rendered.
    let visible_renderable = compacted_prefix[render_start_idx..]
        .iter()
        .filter(|msg| stored_message_renders_visible_message(msg))
        .count();
    let remaining_renderable = total_renderable.saturating_sub(visible_renderable);
    let hidden_user_prompts = compacted_prefix[..render_start_idx]
        .iter()
        .filter(|msg| stored_message_is_user_turn(msg))
        .count();

    (
        render_start_idx,
        RenderedCompactedHistoryInfo {
            total_messages: total_renderable,
            visible_messages: visible_renderable,
            remaining_messages: remaining_renderable,
            hidden_user_prompts,
        },
    )
}

fn image_source_for_message(role: Role, tool: Option<&ToolCall>) -> RenderedImageSource {
    if let Some(tool) = tool {
        return RenderedImageSource::ToolResult {
            tool_name: tool.name.clone(),
        };
    }

    match role {
        Role::User => RenderedImageSource::UserInput,
        Role::Assistant => RenderedImageSource::Other {
            role: "assistant".to_string(),
        },
    }
}

fn image_anchor_for_message(
    rendered_role: &str,
    tool: Option<&ToolCall>,
    user_prompt_ordinal: usize,
) -> Option<RenderedImageAnchor> {
    if let Some(tool) = tool {
        return Some(RenderedImageAnchor::ToolCall {
            id: tool.id.clone(),
        });
    }
    if rendered_role == "user" {
        return Some(RenderedImageAnchor::UserPrompt {
            ordinal: user_prompt_ordinal,
        });
    }
    None
}

fn fallback_image_label_for_tool(tool: &ToolCall) -> Option<String> {
    tool.input
        .get("file_path")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn parse_attached_image_label(text: &str) -> Option<String> {
    let prefix = "[Attached image associated with the preceding tool result: ";
    let suffix = "]";
    text.trim()
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_suffix(suffix))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// True when `text` is exactly an attached-image label message (the synthetic
/// "[Attached image associated with the preceding tool result: ...]" text that
/// follows tool-result images). UIs use this to keep user-prompt ordinals
/// consistent between live transcripts (which never show these) and rendered
/// history (which does).
pub fn is_attached_image_label_text(text: &str) -> bool {
    parse_attached_image_label(text).is_some()
}

pub fn render_images(session: &Session) -> Vec<RenderedImage> {
    render_messages_and_images(session).1
}

pub fn has_rendered_images(session: &Session) -> bool {
    session.messages.iter().any(|msg| {
        msg.content
            .iter()
            .any(|block| matches!(block, ContentBlock::Image { .. }))
    })
}

pub fn summarize_tool_calls(
    session: &Session,
    limit: usize,
) -> Vec<crate::protocol::ToolCallSummary> {
    let mut calls: Vec<crate::protocol::ToolCallSummary> = Vec::new();

    for msg in session.messages.iter().rev() {
        if calls.len() >= limit {
            break;
        }

        let text_summary = msg
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text, .. } => Some(text.as_str()),
                ContentBlock::OpenAICompaction { .. } => Some("[OpenAI native compaction]"),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        for block in &msg.content {
            if calls.len() >= limit {
                break;
            }

            if let ContentBlock::ToolUse { name, input, .. } = block {
                let fallback = input.to_string();
                let brief = if text_summary.trim().is_empty() {
                    crate::util::truncate_str(&fallback, 200).to_string()
                } else {
                    crate::util::truncate_str(&text_summary, 200).to_string()
                };
                calls.push(crate::protocol::ToolCallSummary {
                    tool_name: name.clone(),
                    brief_output: brief,
                    timestamp_secs: msg.timestamp.map(|ts| ts.timestamp().max(0) as u64),
                });
            }
        }
    }

    calls.reverse();
    calls
}

/// Convert stored session messages into renderable messages (including tool output).
pub fn render_messages(session: &Session) -> Vec<RenderedMessage> {
    render_messages_and_images(session).0
}

pub fn render_messages_and_images(session: &Session) -> (Vec<RenderedMessage>, Vec<RenderedImage>) {
    let (messages, images, _) = render_messages_and_images_with_compacted_history(
        session,
        DEFAULT_VISIBLE_COMPACTED_HISTORY_MESSAGES,
    );
    (messages, images)
}

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
        let content = if remaining_compacted == 0 {
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
        rendered.push(RenderedMessage {
            role: "system".to_string(),
            content,
            tool_calls: Vec::new(),
            tool_data: None,
            stored_index: None,
        });
    }

    for (stored_index, msg) in session.messages.iter().enumerate().skip(render_start_idx) {
        if is_internal_system_reminder(msg) {
            continue;
        }

        let role = match msg.display_role {
            Some(StoredDisplayRole::System) => "system",
            Some(StoredDisplayRole::BackgroundTask) => "background_task",
            None if is_auto_poke_user_message(msg) => "system",
            None if super::is_scheduled_task_message(msg) => "system",
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
                ContentBlock::OpenAICompaction { .. } => {}
            }
        }

        let combined = format!("{}{}", reasoning, text);
        if !combined.is_empty() {
            if role == "user" && !is_attached_image_label_text(&text) {
                user_prompt_count += 1;
            }
            rendered.push(RenderedMessage {
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

    (rendered, images, compacted_info)
}
