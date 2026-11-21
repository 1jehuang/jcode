use super::{VersionedPlan, now_unix_ms, truncate_detail};

/// A worker turn that ends while its task is still running auto-completes the
/// task. Anything already terminal or re-queued mid-turn is left alone.
pub(super) fn turn_end_should_auto_complete(status: &str) -> bool {
    matches!(status, "running" | "running_stale")
}

/// Returns whether this turn closed the task, or the reason completion failed.
pub(super) fn finish_running_task(
    plan: &mut VersionedPlan,
    task_id: &str,
    worker: &str,
    open_todo: Option<String>,
) -> Result<bool, String> {
    let Some(item) = plan.items.iter_mut().find(|item| item.id == task_id) else {
        return Ok(false);
    };
    if item.assigned_to.as_deref() != Some(worker) || !turn_end_should_auto_complete(&item.status) {
        return Ok(false);
    }

    let result = match open_todo {
        Some(todo) => Err(format!(
            "Worker ended its turn with an open lane todo: {todo}"
        )),
        None => Ok(true),
    };
    item.status = if result.is_err() { "failed" } else { "done" }.into();
    let progress = plan.task_progress.entry(task_id.to_owned()).or_default();
    let now = now_unix_ms();
    progress.last_heartbeat_unix_ms = Some(now);
    progress.last_checkpoint_unix_ms = Some(now);
    progress.completed_at_unix_ms = Some(now);
    progress.checkpoint_summary = Some(truncate_detail(
        result
            .as_ref()
            .err()
            .map(String::as_str)
            .unwrap_or("task completed"),
        120,
    ));
    progress.stale_since_unix_ms = None;
    progress.checkpoint_count = Some(progress.checkpoint_count.unwrap_or(0) + 1);
    plan.version += 1;
    result
}
