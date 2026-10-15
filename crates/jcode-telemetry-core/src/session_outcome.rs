//! Infer first-useful-action timing, role, and stop reason from session state.

use super::*;

pub(super) fn min_optional_ms(values: impl IntoIterator<Item = Option<u64>>) -> Option<u64> {
    values.into_iter().flatten().min()
}

pub(super) fn time_to_first_agent_action_ms(state: &SessionTelemetry) -> Option<u64> {
    min_optional_ms([
        state.first_assistant_response_ms,
        state.first_tool_call_ms,
        state.first_tool_success_ms,
        state.first_file_edit_ms,
        state.first_test_pass_ms,
    ])
}

pub(super) fn time_to_first_useful_action_ms(state: &SessionTelemetry) -> Option<u64> {
    min_optional_ms([
        state.first_tool_success_ms,
        state.first_file_edit_ms,
        state.first_test_pass_ms,
    ])
    .or(state.first_assistant_response_ms)
}

pub(super) fn infer_agent_role(state: &SessionTelemetry) -> &'static str {
    if state.feature_swarm_used || state.tool_cat_swarm > 0 {
        "swarm"
    } else if state.feature_subagent_used || state.tool_cat_subagent > 0 {
        "subagent"
    } else if state.feature_background_used || state.background_task_count > 0 {
        "background"
    } else {
        "foreground"
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn infer_session_stop_reason(
    event_name: &'static str,
    reason: SessionEndReason,
    state: &SessionTelemetry,
    errors: &ErrorCounts,
    duration_secs: u64,
    session_success: bool,
    abandoned_before_response: bool,
    workflow_coding_used: bool,
) -> &'static str {
    if event_name == "session_crash"
        || matches!(reason, SessionEndReason::Panic | SessionEndReason::Signal)
    {
        return "crash";
    }
    if errors.auth_failed > 0 {
        return "auth_blocked";
    }
    if errors.rate_limited > 0 {
        return "rate_limited";
    }
    if errors.provider_timeout > 0 {
        return "provider_timeout";
    }
    if !state.had_user_prompt {
        return "never_prompted";
    }
    if abandoned_before_response {
        return "no_first_response";
    }
    if state.user_cancelled_count > 0 || matches!(reason, SessionEndReason::Disconnect) {
        return "user_interrupted";
    }
    if matches!(state.first_assistant_response_ms, Some(ms) if ms > 60_000)
        && time_to_first_useful_action_ms(state).is_none_or(|ms| ms > 60_000)
    {
        return "too_slow";
    }
    if state.executed_tool_failures >= 3 && state.executed_tool_successes == 0 {
        return "tool_error_loop";
    }
    if errors.tool_error > 0 && state.executed_tool_successes == 0 {
        return "tool_failures";
    }
    if workflow_coding_used && state.file_write_calls == 0 {
        return "no_file_change";
    }
    if state.tests_run > 0 && state.tests_passed == 0 {
        return "test_failure_unresolved";
    }
    if !session_success && duration_secs >= 300 && state.agent_active_ms_total >= 300_000 {
        return "agent_got_stuck";
    }
    if !session_success {
        return "no_useful_action";
    }
    "completed_successfully"
}
