//! History repairs applied before replaying a conversation to Anthropic.
//!
//! Each pass fixes one way a persisted transcript can violate the API's
//! tool_use/tool_result pairing rules, which otherwise 400s every request.

use super::*;

/// Move each `tool_result` that is not in the message directly after its
/// `tool_use` up into a user message placed directly after the assistant
/// message that made the call. Results already in the right place are left
/// alone. Runs after dedupe, so each id has at most one result.
pub(crate) fn hoist_late_tool_results(messages: &[Message]) -> Vec<Message> {
    use std::collections::{HashMap, HashSet};

    // Assistant message index that made each call.
    let mut call_at: HashMap<&str, usize> = HashMap::new();
    for (mi, msg) in messages.iter().enumerate() {
        if matches!(msg.role, Role::Assistant) {
            for block in &msg.content {
                if let ContentBlock::ToolUse { id, .. } = block {
                    call_at.insert(id, mi);
                }
            }
        }
    }

    // Results that are not in the message right after their call. Consecutive
    // messages of one role are merged into one later, so compare "turn"
    // positions: parallel calls stored as one user message per result, and
    // back-to-back assistant messages, each collapse into a single turn.
    let turn: Vec<usize> = {
        let mut t = 0usize;
        let mut prev: Option<bool> = None;
        messages
            .iter()
            .map(|m| {
                let is_user = matches!(m.role, Role::User);
                if prev.is_some_and(|p| p != is_user) {
                    t += 1;
                }
                prev = Some(is_user);
                t
            })
            .collect()
    };
    let mut late: HashSet<(usize, usize)> = HashSet::new();
    let mut moved: HashMap<usize, Vec<ContentBlock>> = HashMap::new();
    for (mi, msg) in messages.iter().enumerate() {
        for (bi, block) in msg.content.iter().enumerate() {
            let ContentBlock::ToolResult { tool_use_id, .. } = block else {
                continue;
            };
            let Some(&call) = call_at.get(tool_use_id.as_str()) else {
                continue; // no call anywhere: nothing to pair with
            };
            let in_place = turn[mi] == turn[call] + 1;
            if !in_place {
                // Insert after the last message of the call's turn, so a run
                // of assistant messages stays one turn.
                let anchor = (call..messages.len())
                    .take_while(|&i| turn[i] == turn[call])
                    .last()
                    .unwrap_or(call);
                let dest = moved.entry(anchor).or_default();
                for j in std::iter::once(bi).chain(attached_to_result(&msg.content, bi)) {
                    late.insert((mi, j));
                    dest.push(msg.content[j].clone());
                }
            }
        }
    }
    if late.is_empty() {
        return messages.to_vec();
    }
    jcode_logging::warn(&format!(
        "[anthropic] Moved {} late tool_result block(s) up to directly follow their tool_use",
        late.len()
    ));

    let mut out: Vec<Message> = Vec::with_capacity(messages.len() + moved.len());
    for (mi, msg) in messages.iter().enumerate() {
        let content: Vec<ContentBlock> = msg
            .content
            .iter()
            .enumerate()
            .filter(|(bi, _)| !late.contains(&(mi, *bi)))
            .map(|(_, b)| b.clone())
            .collect();
        if !content.is_empty() {
            out.push(Message {
                content,
                ..msg.clone()
            });
        }
        if let Some(results) = moved.remove(&mi) {
            // Same-role merging later folds this into the next user message
            // when there is one, keeping the results first.
            out.push(Message {
                role: Role::User,
                content: results,
                timestamp: msg.timestamp,
                tool_duration_ms: None,
            });
        }
    }
    out
}

/// Indices of the blocks that belong to the tool_result at `result_idx`, in
/// order. A tool output is stored as the result followed by its images (each
/// optionally followed by an "[Attached image ...]" label) and then its
/// `ToolReference` blocks, so those travel with the result when it moves.
/// References for the same call elsewhere in the message are included too,
/// since they are matched to the result by id within one message.
pub(crate) fn attached_to_result(blocks: &[ContentBlock], result_idx: usize) -> Vec<usize> {
    const IMAGE_LABEL_PREFIX: &str = "[Attached image associated with the preceding tool result:";
    let ContentBlock::ToolResult { tool_use_id, .. } = &blocks[result_idx] else {
        return Vec::new();
    };
    let is_own_reference = |block: &ContentBlock| matches!(block, ContentBlock::ToolReference { tool_use_id: id, .. } if id == tool_use_id);
    let mut attached = Vec::new();
    let mut after_image = false;
    let mut end = result_idx + 1;
    while let Some(block) = blocks.get(end) {
        let belongs = match block {
            ContentBlock::Image { .. } => true,
            ContentBlock::Text { text, .. } => after_image && text.starts_with(IMAGE_LABEL_PREFIX),
            other => is_own_reference(other),
        };
        if !belongs {
            break;
        }
        after_image = matches!(block, ContentBlock::Image { .. });
        attached.push(end);
        end += 1;
    }
    attached.extend(
        blocks
            .iter()
            .enumerate()
            .skip(end)
            .filter(|(_, block)| is_own_reference(block))
            .map(|(i, _)| i),
    );
    attached
}

/// Fold `ContentBlock::ToolReference` blocks into their tool_result.
///
/// The API rejects a tool_result that mixes `tool_reference` blocks with any
/// other content, so the referencing tool_result carries only references; its
/// original text moves to a sibling text block right after the tool_results,
/// keeping them contiguous. Only references to tools in this request's
/// catalog are emitted.
pub(crate) fn apply_tool_references(
    content: &mut Vec<ApiContentBlock>,
    blocks: &[ContentBlock],
    is_oauth: bool,
    available: &std::collections::HashSet<&str>,
) {
    use std::collections::HashMap;
    let mut refs: HashMap<String, Vec<String>> = HashMap::new();
    for block in blocks {
        if let ContentBlock::ToolReference {
            tool_use_id,
            tool_name,
        } = block
        {
            let name = if is_oauth {
                map_tool_name_for_oauth(tool_name)
            } else {
                tool_name.clone()
            };
            if !available.contains(name.as_str()) {
                continue;
            }
            let entry = refs.entry(sanitize_tool_id(tool_use_id)).or_default();
            if !entry.contains(&name) {
                entry.push(name);
            }
        }
    }
    if refs.is_empty() {
        return;
    }
    let mut moved_text: Vec<ApiContentBlock> = Vec::new();
    for block in content.iter_mut() {
        let ApiContentBlock::ToolResult {
            tool_use_id,
            content: result_content,
            ..
        } = block
        else {
            continue;
        };
        let Some(names) = refs.remove(tool_use_id.as_str()) else {
            continue;
        };
        let previous = std::mem::replace(
            result_content,
            ToolResultContent::Blocks(
                names
                    .into_iter()
                    .map(|tool_name| ToolResultContentBlock::ToolReference { tool_name })
                    .collect(),
            ),
        );
        let texts: Vec<String> = match previous {
            ToolResultContent::Text(text) => vec![text],
            ToolResultContent::Blocks(blocks) => blocks
                .into_iter()
                .filter_map(|b| match b {
                    ToolResultContentBlock::Text { text } => Some(text),
                    _ => None,
                })
                .collect(),
        };
        for text in texts.into_iter().filter(|t| !t.trim().is_empty()) {
            moved_text.push(ApiContentBlock::Text {
                text,
                cache_control: None,
            });
        }
    }
    if moved_text.is_empty() {
        return;
    }
    let insert_at = content
        .iter()
        .rposition(|b| matches!(b, ApiContentBlock::ToolResult { .. }))
        .map_or(0, |i| i + 1);
    content.splice(insert_at..insert_at, moved_text);
}

/// Rewrite any `tool_result` whose `tool_use_id` is not answered by a `tool_use`
/// in the immediately preceding assistant message into a plain text block.
///
/// Anthropic rejects the whole request (400 "unexpected `tool_use_id` found in
/// `tool_result` blocks") when a result is not paired with the previous
/// message. This happens when a late tool result is persisted after the
/// interrupt repair already answered the call and a new assistant turn was
/// written, leaving the result stranded after an unrelated message. Keeping the
/// output as text preserves the information while making the history sendable.
pub(crate) fn rewrite_orphaned_tool_results(messages: &mut [ApiMessage]) {
    use std::collections::HashSet;

    let mut rewritten = 0usize;
    for i in 0..messages.len() {
        if messages[i].role != "user" {
            continue;
        }
        let expected: HashSet<String> = if i > 0 && messages[i - 1].role == "assistant" {
            messages[i - 1]
                .content
                .iter()
                .filter_map(|b| match b {
                    ApiContentBlock::ToolUse { id, .. } => Some(id.clone()),
                    _ => None,
                })
                .collect()
        } else {
            HashSet::new()
        };

        let has_orphan = messages[i].content.iter().any(|b| {
            matches!(b, ApiContentBlock::ToolResult { tool_use_id, .. } if !expected.contains(tool_use_id))
        });
        if !has_orphan {
            continue;
        }

        let mut paired: Vec<ApiContentBlock> = Vec::new();
        let mut other: Vec<ApiContentBlock> = Vec::new();
        for block in std::mem::take(&mut messages[i].content) {
            match block {
                ApiContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } if !expected.contains(&tool_use_id) => {
                    rewritten += 1;
                    let label = if is_error {
                        "Recovered orphaned tool error"
                    } else {
                        "Recovered orphaned tool output"
                    };
                    match content {
                        ToolResultContent::Text(text) => other.push(ApiContentBlock::Text {
                            text: format!("[{label}: {tool_use_id}]\n{text}"),
                            cache_control: None,
                        }),
                        ToolResultContent::Blocks(blocks) => {
                            other.push(ApiContentBlock::Text {
                                text: format!("[{label}: {tool_use_id}]"),
                                cache_control: None,
                            });
                            for b in blocks {
                                other.push(match b {
                                    ToolResultContentBlock::Text { text } => {
                                        ApiContentBlock::Text {
                                            text,
                                            cache_control: None,
                                        }
                                    }
                                    ToolResultContentBlock::Image { source } => {
                                        ApiContentBlock::Image { source }
                                    }
                                    // A tool_reference is only valid inside a
                                    // paired tool_result, so keep just the name.
                                    ToolResultContentBlock::ToolReference { tool_name } => {
                                        ApiContentBlock::Text {
                                            text: format!("[tool reference: {tool_name}]"),
                                            cache_control: None,
                                        }
                                    }
                                });
                            }
                        }
                    }
                }
                b @ ApiContentBlock::ToolResult { .. } => paired.push(b),
                b => other.push(b),
            }
        }
        // Paired tool_results must lead the user turn.
        paired.extend(other);
        messages[i].content = paired;
    }

    if rewritten > 0 {
        jcode_logging::warn(&format!(
            "[anthropic] Rewrote {rewritten} orphaned tool_result(s) as text to prevent a 400"
        ));
    }
}

/// Text of the synthetic tool_result injected for a tool_use whose real result
/// exists but is not in the message right after the call.
const DISPLACED_TOOL_RESULT_TEXT: &str =
    "[Tool output was recorded later in the conversation and is included there as text]";

/// Guarantee every `tool_use` is answered by a `tool_result` in the
/// immediately following user message.
///
/// The dangling repair only covers tool_uses with no result anywhere in the
/// transcript. A result that exists but arrived after another assistant turn is
/// rewritten to text by [`rewrite_orphaned_tool_results`], which would leave the
/// call unanswered and the request rejected with a 400. Inject an error
/// tool_result for each such call, at the front of the next user message, or
/// in a new user message when the next message is not a user turn.
pub(crate) fn answer_unpaired_tool_uses(messages: &mut Vec<ApiMessage>) {
    use std::collections::HashSet;

    let mut injected = 0usize;
    let mut i = 0;
    while i < messages.len() {
        if messages[i].role != "assistant" {
            i += 1;
            continue;
        }
        let tool_use_ids: Vec<String> = messages[i]
            .content
            .iter()
            .filter_map(|b| match b {
                ApiContentBlock::ToolUse { id, .. } => Some(id.clone()),
                _ => None,
            })
            .collect();
        if tool_use_ids.is_empty() {
            i += 1;
            continue;
        }
        let next_is_user = messages.get(i + 1).is_some_and(|m| m.role == "user");
        let answered: HashSet<String> = if next_is_user {
            messages[i + 1]
                .content
                .iter()
                .filter_map(|b| match b {
                    ApiContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.clone()),
                    _ => None,
                })
                .collect()
        } else {
            HashSet::new()
        };
        let synthetic: Vec<ApiContentBlock> = tool_use_ids
            .into_iter()
            .filter(|id| !answered.contains(id))
            .map(|id| ApiContentBlock::ToolResult {
                tool_use_id: id,
                content: ToolResultContent::Text(DISPLACED_TOOL_RESULT_TEXT.to_string()),
                is_error: true,
            })
            .collect();
        if !synthetic.is_empty() {
            injected += synthetic.len();
            if next_is_user {
                // tool_results must lead the user turn. Place the synthetic
                // results after any existing paired results.
                let content = &mut messages[i + 1].content;
                let insert_at = content
                    .iter()
                    .position(|b| !matches!(b, ApiContentBlock::ToolResult { .. }))
                    .unwrap_or(content.len());
                content.splice(insert_at..insert_at, synthetic);
            } else {
                messages.insert(
                    i + 1,
                    ApiMessage {
                        role: "user".to_string(),
                        content: synthetic,
                    },
                );
            }
        }
        i += 2;
    }

    if injected > 0 {
        jcode_logging::warn(&format!(
            "[anthropic] Injected {injected} synthetic tool_result(s) for tool_use(s) whose \
             result was not in the next message, to prevent a 400"
        ));
    }
}

/// Returns true when a tool_result body is one of the synthetic placeholders
/// injected by the missing tool-output repair paths rather than real output.
pub(crate) fn is_placeholder_tool_result(content: &str, is_error: Option<bool>) -> bool {
    is_error.unwrap_or(false)
        && (content.contains(TOOL_OUTPUT_MISSING_TEXT)
            || content.contains("[Session interrupted before tool execution completed]"))
}

/// Repair persisted Anthropic native web-search blocks that no longer form a valid pair.
///
/// An interrupted turn can persist a server_tool_use without its matching
/// web_search_tool_result. Replaying that block verbatim causes Anthropic to
/// reject the entire request with a 400. In completed history, an orphaned
/// server-tool block is dropped rather than inventing a server response; a
/// final assistant turn ending on an unmatched server-tool block is preserved
/// because that is the valid pause_turn resume shape.
pub(crate) fn repair_dangling_anthropic_server_tools(messages: &[Message]) -> Vec<Message> {
    use std::collections::HashSet;

    let mut starts = HashSet::new();
    let mut results = HashSet::new();
    for msg in messages {
        for block in &msg.content {
            let ContentBlock::ProviderNative { provider, item } = block else {
                continue;
            };
            if provider != jcode_message_types::provider_native::PROVIDER_NATIVE_ANTHROPIC {
                continue;
            }
            match item.get("type").and_then(Value::as_str) {
                Some("server_tool_use") => {
                    if let Some(id) = item.get("id").and_then(Value::as_str) {
                        starts.insert(id.to_string());
                    }
                }
                Some("web_search_tool_result") => {
                    if let Some(id) = item.get("tool_use_id").and_then(Value::as_str) {
                        results.insert(id.to_string());
                    }
                }
                _ => {}
            }
        }
    }

    let final_paused_id = messages.last().and_then(|message| {
        if !matches!(message.role, Role::Assistant) {
            return None;
        }
        let Some(ContentBlock::ProviderNative { provider, item }) = message.content.last() else {
            return None;
        };
        if provider != jcode_message_types::provider_native::PROVIDER_NATIVE_ANTHROPIC {
            return None;
        }
        (item.get("type").and_then(Value::as_str) == Some("server_tool_use"))
            .then(|| item.get("id").and_then(Value::as_str))
            .flatten()
            .map(str::to_string)
    });

    let mut repaired = Vec::with_capacity(messages.len());
    for message in messages {
        let mut content = Vec::with_capacity(message.content.len());
        for block in &message.content {
            let ContentBlock::ProviderNative { provider, item } = block else {
                content.push(block.clone());
                continue;
            };

            if provider != jcode_message_types::provider_native::PROVIDER_NATIVE_ANTHROPIC {
                content.push(block.clone());
                continue;
            }

            match item.get("type").and_then(Value::as_str) {
                Some("server_tool_use") => {
                    let Some(id) = item.get("id").and_then(Value::as_str) else {
                        continue;
                    };
                    if results.contains(id) || final_paused_id.as_deref() == Some(id) {
                        content.push(block.clone());
                    }
                }
                Some("web_search_tool_result") => {
                    let Some(id) = item.get("tool_use_id").and_then(Value::as_str) else {
                        continue;
                    };
                    if starts.contains(id) {
                        content.push(block.clone());
                    }
                }
                _ => content.push(block.clone()),
            }
        }

        if !content.is_empty() {
            let mut repaired_message = message.clone();
            repaired_message.content = content;
            repaired.push(repaired_message);
        }
    }
    repaired
}

/// Keep only the first `tool_use` block for each id. Anthropic requires every
/// `tool_use` to be answered by a `tool_result` in the very next message, and a
/// repeated id can never satisfy that because its result is deduplicated to
/// the first occurrence.
pub(crate) fn dedupe_tool_uses(messages: &[Message]) -> Vec<Message> {
    use std::collections::HashSet;
    let mut seen: HashSet<&str> = HashSet::new();
    let mut duplicate_seen = false;
    for msg in messages {
        for block in &msg.content {
            if let ContentBlock::ToolUse { id, .. } = block
                && !seen.insert(id.as_str())
            {
                duplicate_seen = true;
            }
        }
    }
    if !duplicate_seen {
        return messages.to_vec();
    }

    let mut kept: HashSet<String> = HashSet::new();
    let mut dropped = 0usize;
    let out = messages
        .iter()
        .map(|msg| {
            let mut msg = msg.clone();
            msg.content.retain(|block| match block {
                ContentBlock::ToolUse { id, .. } => {
                    let keep = kept.insert(id.clone());
                    if !keep {
                        dropped += 1;
                    }
                    keep
                }
                _ => true,
            });
            msg
        })
        .collect();
    jcode_logging::warn(&format!(
        "[anthropic] Dropped {dropped} repeated tool_use block(s); each tool_use id may appear only once"
    ));
    out
}

/// Remove duplicate `tool_result` blocks so each `tool_use_id` is answered
/// exactly once, preferring real output over a synthetic placeholder.
/// Messages left with no content at all are dropped by the caller's
/// `!content.is_empty()` guard.
pub(crate) fn dedupe_tool_results(messages: &[Message]) -> Vec<Message> {
    use std::collections::HashMap;

    // Winner position per tool_use_id: the first real result if one exists,
    // otherwise the first occurrence at all.
    let mut winner: HashMap<&str, (usize, usize)> = HashMap::new();
    let mut winner_is_real: HashMap<&str, bool> = HashMap::new();
    let mut duplicate_seen = false;

    for (mi, msg) in messages.iter().enumerate() {
        for (bi, block) in msg.content.iter().enumerate() {
            let ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } = block
            else {
                continue;
            };
            let real = !is_placeholder_tool_result(content, *is_error);
            match winner_is_real.get(tool_use_id.as_str()) {
                None => {
                    winner.insert(tool_use_id, (mi, bi));
                    winner_is_real.insert(tool_use_id, real);
                }
                Some(false) if real => {
                    // Upgrade a placeholder winner to the real output.
                    winner.insert(tool_use_id, (mi, bi));
                    winner_is_real.insert(tool_use_id, true);
                    duplicate_seen = true;
                }
                Some(_) => duplicate_seen = true,
            }
        }
    }

    if !duplicate_seen {
        return messages.to_vec();
    }

    let dropped = std::cell::Cell::new(0usize);
    let out: Vec<Message> = messages
        .iter()
        .enumerate()
        .map(|(mi, msg)| {
            let mut msg = msg.clone();
            let mut bi = 0usize;
            msg.content.retain(|block| {
                let index = bi;
                bi += 1;
                let ContentBlock::ToolResult { tool_use_id, .. } = block else {
                    return true;
                };
                let keep = winner.get(tool_use_id.as_str()) == Some(&(mi, index));
                if !keep {
                    dropped.set(dropped.get() + 1);
                }
                keep
            });
            msg
        })
        .collect();

    if dropped.get() > 0 {
        jcode_logging::warn(&format!(
            "[anthropic] Dropped {} duplicate tool_result block(s); each tool_use_id may be \
             answered only once",
            dropped.get()
        ));
    }
    out
}
