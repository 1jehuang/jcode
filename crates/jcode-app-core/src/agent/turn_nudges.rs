use super::Agent;

impl Agent {
    pub(super) const SEQUENTIAL_TOOL_ROUNDS_BEFORE_BATCH_NUDGE: u32 = 3;
    pub(super) const BATCH_NUDGE: &str = "<system-reminder>Several tool calls have been made one at a time. If the next independent operations can run concurrently, use the batch tool instead of making more sequential calls. Keep sequential calls when one result is required to decide the next operation.</system-reminder>";
    pub(super) const TOOL_CALLS_BEFORE_TODO_NUDGE: usize = 20;
    pub(super) const TODO_NUDGE: &str = "<system-reminder>Many tool calls have been made but this session has no todos. Use the todo tool to plan and track this task.</system-reminder>";
    pub(super) fn update_sequential_tool_rounds(
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

    pub(super) fn should_inject_batch_nudge(pending: bool, batch_available: bool) -> bool {
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
