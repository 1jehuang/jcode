//! Aggregate persisted provider calls, not rendered rows (tool-only calls may
//! render no assistant row). Work over the full record even for a compacted view.
use super::{ContentBlock, RenderedMessage, Role};
use jcode_session_types::{ResponseStats, StoredMessage};

fn is_user_prompt(message: &StoredMessage) -> bool {
    matches!(message.role, Role::User)
        && message.display_role.is_none()
        && !super::is_internal_system_reminder(message)
        && !super::is_auto_poke_user_message(message)
        && !super::super::is_scheduled_task_message(message)
        && message.content.iter().any(|block| match block {
            ContentBlock::Text { text, .. } => {
                !text.trim().is_empty() && super::parse_attached_image_label(text).is_none()
            }
            ContentBlock::Image { .. } => true,
            _ => false,
        })
        // Tool-result rows can include synthetic text and images. They are a
        // continuation of the same turn, not another human prompt.
        && !message.content.iter().any(|block| matches!(block, ContentBlock::ToolResult { .. }))
}

pub(super) fn attach(messages: &[StoredMessage], rendered: &mut [RenderedMessage]) {
    let row_by_stored_index: std::collections::HashMap<_, _> = rendered
        .iter()
        .enumerate()
        .filter(|(_, row)| row.role == "assistant")
        .filter_map(|(i, row)| row.stored_index.map(|stored| (stored, i)))
        .collect();
    let mut start = 0;
    for end in 0..=messages.len() {
        if end == messages.len() || is_user_prompt(&messages[end]) {
            attach_turn(messages, start, end, rendered, &row_by_stored_index);
            start = end;
        }
    }
}

fn attach_turn(
    messages: &[StoredMessage],
    start: usize,
    end: usize,
    rendered: &mut [RenderedMessage],
    row_by_stored_index: &std::collections::HashMap<usize, usize>,
) {
    let assistants: Vec<_> = (start..end)
        .filter(|&i| matches!(messages[i].role, Role::Assistant))
        .collect();
    let Some(&last) = assistants.last() else {
        return;
    };
    // A pending tool call is not a completed response. Do not attach its usage
    // to an earlier visible assistant row, even when the pending call is hidden.
    if messages[last]
        .content
        .iter()
        .any(|block| matches!(block, ContentBlock::ToolUse { .. }))
    {
        return;
    }
    let Some(&row_index) = row_by_stored_index.get(&last) else {
        return;
    };
    let row = &mut rendered[row_index];
    let sum = |field: fn(&jcode_session_types::StoredTokenUsage) -> Option<u64>| {
        assistants.iter().try_fold(0u64, |total, &i| {
            total.checked_add(field(messages[i].token_usage.as_ref()?)?)
        })
    };
    let stats = ResponseStats {
        duration_secs: None,
        input_tokens: sum(|usage| Some(usage.input_tokens)),
        output_tokens: sum(|usage| Some(usage.output_tokens)),
        cache_read_tokens: sum(|usage| usage.cache_read_input_tokens),
        cache_creation_tokens: sum(|usage| usage.cache_creation_input_tokens),
    };
    if stats.input_tokens.is_some()
        || stats.output_tokens.is_some()
        || stats.cache_read_tokens.is_some()
        || stats.cache_creation_tokens.is_some()
    {
        row.response_stats = Some(stats);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jcode_session_types::StoredTokenUsage;

    fn text(role: Role, text: &str, usage: Option<StoredTokenUsage>) -> StoredMessage {
        StoredMessage {
            id: String::new(),
            role,
            content: vec![ContentBlock::Text {
                text: text.into(),
                cache_control: None,
            }],
            display_role: None,
            timestamp: None,
            tool_duration_ms: None,
            token_usage: usage,
        }
    }
    fn usage(input: u64) -> Option<StoredTokenUsage> {
        Some(StoredTokenUsage {
            input_tokens: input,
            output_tokens: 2,
            cache_read_input_tokens: Some(3),
            cache_creation_input_tokens: Some(4),
        })
    }
    fn row(index: usize) -> RenderedMessage {
        RenderedMessage {
            role: "assistant".into(),
            content: "answer".into(),
            stored_index: Some(index),
            tool_calls: vec![],
            tool_data: None,
            response_stats: None,
        }
    }
    fn tool_call(message: &mut StoredMessage) {
        message.content = vec![ContentBlock::ToolUse {
            id: "tool".into(),
            name: "read".into(),
            input: serde_json::json!({}),
            thought_signature: None,
        }];
    }

    #[test]
    fn response_stats_aggregate_hidden_tool_rounds_and_separate_user_turns() {
        let mut messages = vec![
            text(Role::User, "first", None),
            text(Role::Assistant, "", usage(10)),
            text(Role::User, "", None),
            text(Role::Assistant, "answer", usage(20)),
            text(Role::User, "next", None),
            text(Role::Assistant, "answer", usage(40)),
        ];
        tool_call(&mut messages[1]);
        messages[2].content = vec![ContentBlock::ToolResult {
            tool_use_id: "tool".into(),
            content: "result".into(),
            is_error: None,
        }];
        messages[2].tool_duration_ms = Some(1234);
        let mut rows = vec![row(3), row(5)];
        attach(&messages, &mut rows);
        let first = rows[0].response_stats.as_ref().unwrap();
        assert_eq!(first.input_tokens, Some(30));
        assert_eq!(first.output_tokens, Some(4));
        assert_eq!(first.cache_read_tokens, Some(6));
        assert_eq!(first.cache_creation_tokens, Some(8));
        assert_eq!(first.duration_secs, None);
        assert_eq!(
            rows[1].response_stats.as_ref().unwrap().input_tokens,
            Some(40)
        );
        // A compacted rendering still includes all calls in the visible turn.
        let mut tail = vec![row(3)];
        attach(&messages, &mut tail);
        assert_eq!(tail[0].response_stats, rows[0].response_stats);
    }

    #[test]
    fn response_stats_unknown_cache_and_old_usage_are_not_zero() {
        let mut messages = vec![
            text(Role::Assistant, "round", usage(10)),
            text(Role::Assistant, "answer", usage(20)),
        ];
        messages[0]
            .token_usage
            .as_mut()
            .unwrap()
            .cache_read_input_tokens = None;
        let mut rows = vec![row(0), row(1)];
        attach(&messages, &mut rows);
        assert!(rows[0].response_stats.is_none());
        assert_eq!(
            rows[1].response_stats.as_ref().unwrap().cache_read_tokens,
            None
        );
        assert_eq!(
            rows[1].response_stats.as_ref().unwrap().input_tokens,
            Some(30)
        );
        messages[0].token_usage = None;
        let mut rows = vec![row(1)];
        attach(&messages, &mut rows);
        assert!(rows[0].response_stats.is_none());
    }

    #[test]
    fn response_stats_pending_tool_call_has_no_footer() {
        let mut messages = vec![
            text(Role::Assistant, "working", usage(10)),
            text(Role::Assistant, "", usage(20)),
        ];
        tool_call(&mut messages[1]);
        let mut rows = vec![row(0)];
        attach(&messages, &mut rows);
        assert!(rows[0].response_stats.is_none());
    }
    #[test]
    fn response_stats_survive_stored_json_and_real_rendering() {
        let messages = vec![
            text(Role::User, "prompt", None),
            text(Role::Assistant, "first", usage(10)),
            text(Role::Assistant, "final", usage(20)),
        ];
        let encoded = serde_json::to_string(&messages).unwrap();
        let mut session = super::super::Session::create(None, None);
        session.messages = serde_json::from_str(&encoded).unwrap();
        let rows = super::super::render_messages(&session);
        assert_eq!(rows.len(), 3);
        assert!(rows[0].response_stats.is_none());
        assert!(rows[1].response_stats.is_none());
        let wire = serde_json::to_value(&rows[2]).unwrap();
        assert_eq!(wire["response_stats"]["input_tokens"], 30);
        assert!(wire["response_stats"].get("duration_secs").is_none());
    }
    #[test]
    fn response_stats_internal_rows_do_not_split_and_overflow_is_unknown() {
        let mut internal = text(Role::User, "internal continuation", None);
        internal.display_role = Some(jcode_session_types::StoredDisplayRole::System);
        let messages = vec![
            text(Role::Assistant, "first", usage(u64::MAX)),
            internal,
            text(Role::Assistant, "final", usage(1)),
        ];
        let mut rows = vec![row(0), row(2)];
        attach(&messages, &mut rows);
        assert!(rows[0].response_stats.is_none());
        let stats = rows[1].response_stats.as_ref().unwrap();
        assert_eq!(stats.input_tokens, None);
        assert_eq!(stats.output_tokens, Some(4));
    }
}
