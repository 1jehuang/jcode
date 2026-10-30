use super::*;

pub(super) fn foreground_budget() -> Duration {
    Duration::from_millis(crate::config::config().tools.auto_background_timeout_ms)
}

pub(super) fn automatic_handoff_reason(budget: Duration) -> String {
    format!("automatically after {} seconds", budget.as_secs_f64())
}

pub(super) fn auto_background_allowed(name: &str, input: &serde_json::Value) -> bool {
    !(name == "bg" && input.get("action").and_then(|value| value.as_str()) == Some("wait"))
}

/// SDK callbacks and nonstreaming turns use the same policy as TUI turns.
pub(in crate::agent) async fn execute_with_background(
    registry: Registry,
    name: String,
    input: serde_json::Value,
    ctx: ToolContext,
) -> Result<crate::tool::ToolOutput> {
    execute_with_background_budget(registry, name, input, ctx, foreground_budget()).await
}

async fn execute_with_background_budget(
    registry: Registry,
    name: String,
    input: serde_json::Value,
    ctx: ToolContext,
    budget: Duration,
) -> Result<crate::tool::ToolOutput> {
    let auto_background = auto_background_allowed(&name, &input);
    let session_id = ctx.session_id.clone();
    let tool_name = name.clone();
    let mut work = crate::runtime_activity::spawn_session(&session_id, async move {
        registry.execute(&tool_name, input, ctx).await
    })
    .ok_or_else(|| anyhow::anyhow!("Session transfer in progress; tool not accepted"))?;
    tokio::select! {
        biased;
        result = &mut work => result.map_err(|e| anyhow::anyhow!("Tool task panicked: {}", e))?,
        _ = tokio::time::sleep(budget), if auto_background && !budget.is_zero() => {
            let info = crate::background::global().adopt(&name, &session_id, work).await;
            Ok(crate::tool::ToolOutput::new(format!(
                "Tool '{}' was moved to background {} (task_id: {}).\n\n{}",
                name, automatic_handoff_reason(budget), info.task_id, crate::tool::BACKGROUND_WAIT_GUIDANCE
            )))
        }
    }
}

/// Returns true for an automatic handoff, false for a user/reload signal.
pub(super) async fn wait_for_tool_handoff(
    bg: &InterruptSignal,
    shutdown: &InterruptSignal,
    auto_background: bool,
    budget: Duration,
) -> bool {
    tokio::select! {
        biased;
        _ = shutdown.notified() => false,
        _ = bg.notified() => false,
        _ = tokio::time::sleep(budget), if auto_background && !budget.is_zero() => true,
    }
}

#[cfg(test)]
#[path = "turn_streaming_mpsc_handoff_tests.rs"]
mod handoff_tests;

pub(super) fn reload_interrupted_tool_result(tc: &ToolCall, elapsed_secs: f64) -> (String, bool) {
    if tc.name == "selfdev" {
        return ("Reload initiated. Process restarting...".to_string(), false);
    }

    let action = tc
        .input
        .get("action")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let is_wait_like = (tc.name == "bg" && action == "wait")
        || (tc.name == "swarm" && matches!(action, "await_members" | "run_plan"));

    if is_wait_like {
        let input = serde_json::to_string(&tc.input).unwrap_or_else(|_| "{}".to_string());
        return (
            format!(
                "[Tool '{}' wait interrupted by server reload after {:.1}s. The underlying operation may still be running. Resume the wait by rerunning the same tool call with input: {}]",
                tc.name, elapsed_secs, input
            ),
            false,
        );
    }

    (
        format!(
            "[Tool '{}' interrupted by server reload after {:.1}s]",
            tc.name, elapsed_secs
        ),
        true,
    )
}

/// Called only after automatic continuations have been exhausted.
pub(super) fn incomplete_turn_stop(stop_reason: Option<&str>) -> Option<ServerEvent> {
    let reason = stop_reason?;
    if Agent::should_continue_after_stop_reason(reason)
        || Agent::is_stranded_tool_use_stop(Some(reason))
    {
        Some(ServerEvent::TurnStopped {
            reason: crate::protocol::TurnStopReason::LimitReached,
            message: format!(
                "The provider stopped with {reason} after automatic continuation attempts were exhausted. Output may be incomplete."
            ),
            provider_stop_reason: Some(reason.to_string()),
        })
    } else {
        None
    }
}
