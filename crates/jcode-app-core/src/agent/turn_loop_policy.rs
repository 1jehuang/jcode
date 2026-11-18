//! Continuation retry limits and turn-loop nudges.

use super::*;

impl Agent {
    /// Maximum number of context-limit compaction retries before giving up.
    pub(in crate::agent) const MAX_CONTEXT_LIMIT_RETRIES: u32 = 5;
    pub(in crate::agent) const MAX_INCOMPLETE_CONTINUATION_ATTEMPTS: u32 = 3;
    /// Retries allowed when the provider returns an empty response right after
    /// tool results. This is a transient provider hiccup, not a signal that the
    /// task is finished, so a single retry is too few: one empty response
    /// observed once in 43 turns silently ended a 20-hour benchmark run with the
    /// task half-done. The counter is per turn-loop, so a genuinely finished
    /// agent still exits promptly.
    pub(crate) const MAX_EMPTY_POST_TOOL_CONTINUATION_ATTEMPTS: u32 = 5;
    pub(in crate::agent) const SEQUENTIAL_TOOL_ROUNDS_BEFORE_BATCH_NUDGE: u32 = 3;
    pub(in crate::agent) const BATCH_NUDGE: &str = "<system-reminder>Several tool calls have been made one at a time. If the next independent operations can run concurrently, use the batch tool instead of making more sequential calls. Keep sequential calls when one result is required to decide the next operation.</system-reminder>";
    pub(in crate::agent) const TOOL_CALLS_BEFORE_TODO_NUDGE: usize = 20;
    pub(in crate::agent) const TODO_NUDGE: &str = "<system-reminder>Many tool calls have been made but this session has no todos. Use the todo tool to plan and track this task.</system-reminder>";
    pub(in crate::agent) fn update_sequential_tool_rounds(
        current: u32,
        tool_count: usize,
        used_batch: bool,
    ) -> u32 {
        if tool_count == 1 && !used_batch {
            current.saturating_add(1)
        } else {
            0
        }
    }

    pub(in crate::agent) fn should_inject_batch_nudge(
        pending: bool,
        batch_available: bool,
    ) -> bool {
        pending && batch_available
    }

    pub(in crate::agent) fn plan_limit_reminder(
        notice: &crate::subscription_notice::QuotaExceeded,
    ) -> String {
        format!(
            "<system-reminder>Jcode plan limit: {notice} An upgrade card with a button is already shown to the user in the chat. Mention the limit in one short sentence, including the upgrade link if one is given. Do not open checkout or purchase anything. Continue the task without that feature.</system-reminder>"
        )
    }
}
