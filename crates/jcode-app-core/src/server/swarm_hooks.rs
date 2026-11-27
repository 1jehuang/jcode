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
    /// The coordinator's working directory, where the observer runs.
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
    /// The coordinator's working directory, where the observer runs.
    pub coordinator_working_dir: Option<&'a str>,
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
        transition.coordinator_working_dir,
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
    if let Some(cwd) = transition.coordinator_working_dir {
        event = event.cwd(cwd);
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

    /// Run `f` with `event_env` pointing at a recording hook; returns what the
    /// hook wrote (`pwd|stdin`), or an empty string if it never ran.
    #[cfg(unix)]
    fn record_hook(event_env: &str, f: impl FnOnce()) -> String {
        use std::os::unix::fs::PermissionsExt;
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::TempDir::new().expect("temp dir");
        let record = temp.path().join("record.txt");
        let script = temp.path().join("hook.sh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nstdin=$(cat)\nprintf '%s|%s' \"$(pwd -P)\" \"$stdin\" > {}\n",
                crate::terminal_launch::sh_escape(&record.to_string_lossy())
            ),
        )
        .expect("write hook");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("chmod hook");
        let previous = std::env::var_os(event_env);
        crate::env::set_var(event_env, script.to_string_lossy().to_string());
        let event = event_env
            .trim_start_matches("JCODE_HOOK_")
            .to_ascii_lowercase();
        wait_for_hook_config(&event, true);
        f();
        let mut recorded = String::new();
        // Give a (wrongly) fired hook time to run before concluding silence.
        for _ in 0..50 {
            if let Ok(data) = std::fs::read_to_string(&record)
                && !data.is_empty()
            {
                recorded = data;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        match previous {
            Some(value) => crate::env::set_var(event_env, value),
            None => crate::env::remove_var(event_env),
        }
        wait_for_hook_config(&event, false);
        recorded
    }

    /// The config cache re-checks the environment on a throttle, so wait until
    /// it reflects the override before dispatching.
    #[cfg(unix)]
    fn wait_for_hook_config(event: &str, configured: bool) {
        for _ in 0..200 {
            if hooks::hook_configured(event) == configured {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    }

    #[cfg(unix)]
    fn register_worker(session_id: &str) {
        hooks::register_swarm_identity(
            session_id,
            SwarmIdentity {
                swarm_id: "sw".into(),
                coordinator_session_id: "coord".into(),
                parent_session_id: "coord".into(),
                label: Some("task".into()),
                agent_type: None,
            },
        );
    }

    #[cfg(unix)]
    fn transition<'a>(
        session_id: &'a str,
        old_status: &'a str,
        new_status: &'a str,
        status_changed: bool,
        coordinator_working_dir: Option<&'a str>,
    ) -> StatusTransition<'a> {
        StatusTransition {
            session_id,
            old_status,
            new_status,
            status_changed,
            duration_ms: 1,
            completion_report: None,
            coordinator_working_dir,
        }
    }

    #[cfg(unix)]
    #[test]
    fn end_event_fires_for_valid_transition_in_coordinator_dir() {
        let coord_dir = tempfile::TempDir::new().expect("coordinator dir");
        let expected = std::fs::canonicalize(coord_dir.path()).expect("canonical");
        register_worker("swarm-hooks-end-valid");
        let dir = coord_dir.path().to_string_lossy().into_owned();
        let recorded = record_hook("JCODE_HOOK_SWARM_MEMBER_END", || {
            member_status_changed(&transition(
                "swarm-hooks-end-valid",
                "running",
                "ready",
                true,
                Some(&dir),
            ));
        });
        hooks::forget_swarm_identity("swarm-hooks-end-valid");
        assert!(
            recorded.starts_with(&format!("{}|", expected.display())),
            "{recorded}"
        );
        assert!(recorded.contains("\"cwd\":"), "{recorded}");
        assert!(recorded.contains("\"session_id\":\"coord\""), "{recorded}");
    }

    #[cfg(unix)]
    #[test]
    fn end_event_suppressed_for_unregistered_noop_and_non_end_updates() {
        register_worker("swarm-hooks-end-guards");
        let cases = [
            // Unregistered worker.
            ("swarm-hooks-unregistered", "running", "ready", true),
            // Status did not change.
            ("swarm-hooks-end-guards", "running", "ready", false),
            // Not leaving the running state.
            ("swarm-hooks-end-guards", "ready", "completed", true),
            // Not entering an end state.
            ("swarm-hooks-end-guards", "running", "streaming", true),
        ];
        for (session, old, new, changed) in cases {
            let recorded = record_hook("JCODE_HOOK_SWARM_MEMBER_END", || {
                member_status_changed(&transition(session, old, new, changed, None));
            });
            assert_eq!(recorded, "", "{session} {old}->{new} changed={changed}");
        }
        hooks::forget_swarm_identity("swarm-hooks-end-guards");
    }

    #[cfg(unix)]
    #[test]
    fn start_event_runs_in_given_working_dir() {
        let coord_dir = tempfile::TempDir::new().expect("coordinator dir");
        let expected = std::fs::canonicalize(coord_dir.path()).expect("canonical");
        let dir = coord_dir.path().to_string_lossy().into_owned();
        let recorded = record_hook("JCODE_HOOK_SWARM_MEMBER_START", || {
            worker_spawned(&SpawnedWorker {
                session_id: "swarm-hooks-start",
                swarm_id: "sw",
                coordinator_session_id: "coord",
                label: Some("task"),
                agent_type: None,
                spawn_mode: "inline",
                working_dir: Some(&dir),
            });
        });
        hooks::forget_swarm_identity("swarm-hooks-start");
        assert!(
            recorded.starts_with(&format!("{}|", expected.display())),
            "{recorded}"
        );
        assert!(recorded.contains("\"spawn_mode\":\"inline\""), "{recorded}");
    }
}
