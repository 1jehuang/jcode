//! Swarm identity registration and the `swarm_member_start` /
//! `swarm_member_end` observer hooks.
//!
//! Both events fire in the coordinator's context (`session_id` is the
//! coordinator) with Claude-shaped `SubagentStart` / `SubagentStop` payloads
//! on stdin. Hooks that fire inside the worker itself receive the same
//! identity through the registry in `crate::hooks`.

use crate::hooks::{self, HookEvent, SwarmIdentity};
use serde_json::json;

/// Maximum bytes of the completion report tail passed to `swarm_member_end`.
const END_REPORT_LIMIT: usize = 4000;

/// States a running worker can leave for, which end its work.
pub(in crate::server) fn is_end_status(status: &str) -> bool {
    matches!(
        status,
        "ready" | "completed" | "failed" | "stopped" | "crashed"
    )
}

/// States that mean the worker is doing work.
pub(in crate::server) fn is_running_status(status: &str) -> bool {
    matches!(
        status,
        "running" | "streaming" | "thinking" | "running_stale"
    )
}

/// Details of a freshly spawned worker.
pub(in crate::server) struct SpawnedWorker<'a> {
    pub session_id: &'a str,
    pub swarm_id: &'a str,
    pub coordinator_session_id: &'a str,
    pub label: Option<&'a str>,
    pub agent_type: Option<&'a str>,
    pub spawn_mode: &'a str,
    pub working_dir: Option<&'a str>,
}

/// Register the worker's identity and fire `swarm_member_start`.
pub(in crate::server) fn worker_spawned(worker: &SpawnedWorker<'_>) {
    let identity = SwarmIdentity {
        swarm_id: worker.swarm_id.to_string(),
        coordinator_session_id: worker.coordinator_session_id.to_string(),
        parent_session_id: worker.coordinator_session_id.to_string(),
        label: worker.label.map(str::to_string),
        agent_type: worker.agent_type.map(str::to_string),
    };
    hooks::register_swarm_identity(worker.session_id, identity.clone());
    if !hooks::hook_configured("swarm_member_start") {
        return;
    }
    let agent_type = worker.agent_type.or(worker.label);
    let mut payload = hooks::common_payload(
        "SubagentStart",
        "swarm_member_start",
        worker.coordinator_session_id,
        worker.working_dir,
    );
    payload.insert("agent_id".into(), worker.session_id.into());
    if let Some(agent_type) = agent_type {
        payload.insert("agent_type".into(), agent_type.into());
    }
    let extra = hooks::jcode_object(&mut payload);
    extra.insert("label".into(), json!(worker.label));
    extra.insert("spawn_mode".into(), worker.spawn_mode.into());
    extra.insert("swarm".into(), identity.to_json());
    let mut event = HookEvent::new("swarm_member_start")
        .session_id(worker.coordinator_session_id)
        .field("AGENT_ID", worker.session_id)
        .field("SWARM_ID", worker.swarm_id)
        .field("SPAWN_MODE", worker.spawn_mode)
        .stdin_json(serde_json::Value::Object(payload).to_string());
    if let Some(agent_type) = agent_type {
        event = event.field("AGENT_TYPE", agent_type);
    }
    if let Some(label) = worker.label {
        event = event.field("SWARM_LABEL", label);
    }
    if let Some(cwd) = worker.working_dir {
        event = event.cwd(cwd);
    }
    hooks::dispatch_observer(event);
}

/// A worker's status transition, for `swarm_member_end`.
pub(in crate::server) struct StatusTransition<'a> {
    pub session_id: &'a str,
    pub old_status: &'a str,
    pub new_status: &'a str,
    pub status_changed: bool,
    pub duration_ms: u64,
    pub completion_report: Option<&'a str>,
}

/// Fire `swarm_member_end` when a registered worker leaves the running state
/// for an end state. Repeated updates of the same status never refire.
pub(in crate::server) fn member_status_changed(transition: &StatusTransition<'_>) {
    if !transition.status_changed
        || !is_running_status(transition.old_status)
        || !is_end_status(transition.new_status)
        || !hooks::hook_configured("swarm_member_end")
    {
        return;
    }
    let Some(identity) = hooks::swarm_identity(transition.session_id) else {
        return;
    };
    let agent_type = identity.agent_type.as_ref().or(identity.label.as_ref());
    let mut payload = hooks::common_payload(
        "SubagentStop",
        "swarm_member_end",
        &identity.coordinator_session_id,
        None,
    );
    payload.insert("agent_id".into(), transition.session_id.into());
    if let Some(agent_type) = agent_type {
        payload.insert("agent_type".into(), agent_type.as_str().into());
    }
    payload.insert("stop_hook_active".into(), false.into());
    if let Some(report) = transition.completion_report {
        payload.insert(
            "last_assistant_message".into(),
            hooks::truncate_marked(report, END_REPORT_LIMIT).into(),
        );
    }
    let extra = hooks::jcode_object(&mut payload);
    extra.insert("status".into(), transition.new_status.into());
    extra.insert("duration_ms".into(), transition.duration_ms.into());
    extra.insert("label".into(), json!(identity.label));
    extra.insert("swarm".into(), identity.to_json());
    let mut event = HookEvent::new("swarm_member_end")
        .session_id(identity.coordinator_session_id.clone())
        .field("AGENT_ID", transition.session_id)
        .field("SWARM_ID", identity.swarm_id.clone())
        .field("STATUS", transition.new_status)
        .field("DURATION_MS", transition.duration_ms.to_string())
        .stdin_json(serde_json::Value::Object(payload).to_string());
    if let Some(agent_type) = agent_type {
        event = event.field("AGENT_TYPE", agent_type.clone());
    }
    if let Some(label) = &identity.label {
        event = event.field("SWARM_LABEL", label.clone());
    }
    hooks::dispatch_observer(event);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn end_and_running_status_sets() {
        assert!(is_running_status("running"));
        assert!(!is_running_status("ready"));
        assert!(is_end_status("crashed"));
        assert!(!is_end_status("running"));
    }

    #[test]
    fn unregistered_workers_and_noop_updates_do_not_fire() {
        // No hook is configured in the test process, so this only checks that
        // the guard conditions return without panicking.
        member_status_changed(&StatusTransition {
            session_id: "swarm-hooks-unregistered",
            old_status: "running",
            new_status: "ready",
            status_changed: true,
            duration_ms: 1,
            completion_report: None,
        });
        member_status_changed(&StatusTransition {
            session_id: "swarm-hooks-unregistered",
            old_status: "ready",
            new_status: "ready",
            status_changed: false,
            duration_ms: 1,
            completion_report: None,
        });
    }
}
