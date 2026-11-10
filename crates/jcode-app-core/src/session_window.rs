//! Acting on a session that another local client window already shows.
//!
//! Resume consults `display.resume_open_elsewhere` before attaching: `move`
//! brings the existing window to the user (niri IPC), `replace` closes the
//! other window's client so only the new one remains. Which client shows which
//! session comes from the `client_sessions` registry, so no server round trip
//! is needed. Linux only: the registry lookup and process walk read `/proc`.

use crate::config::ResumeOpenElsewhere;

/// What happened to the other windows showing a session being resumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenElsewhereOutcome {
    /// Nothing to do: disabled, or no other window shows the session.
    Proceed,
    /// The existing window was brought here and focused. Do not attach again.
    Moved,
    /// This many other clients were asked to close. Attach as usual.
    Replaced(usize),
}

/// Apply `display.resume_open_elsewhere` for `session_id`.
pub fn apply_resume_open_elsewhere(session_id: &str) -> OpenElsewhereOutcome {
    let mode = crate::config::config().display.resume_open_elsewhere;
    if mode == ResumeOpenElsewhere::Off {
        return OpenElsewhereOutcome::Proceed;
    }
    let clients = crate::client_focus::clients_showing_session(session_id);
    if clients.is_empty() {
        return OpenElsewhereOutcome::Proceed;
    }
    if mode == ResumeOpenElsewhere::Move && bring_client_window_here(&clients) {
        return OpenElsewhereOutcome::Moved;
    }
    OpenElsewhereOutcome::Replaced(close_clients(&clients))
}

/// Signal a client uses to exit cleanly because another window took over its
/// session. Unlike SIGHUP/SIGTERM it never marks the session crashed.
#[cfg(unix)]
pub const CLIENT_REPLACED_SIGNAL: i32 = libc::SIGUSR1;

#[cfg(unix)]
fn close_clients(pids: &[u32]) -> usize {
    pids.iter()
        .filter(|pid| {
            let Ok(pid) = i32::try_from(**pid) else {
                return false;
            };
            // SAFETY: kill only sends a signal; the pid was just verified to be
            // a live jcode client by the registry lookup.
            pid > 1 && unsafe { libc::kill(pid, CLIENT_REPLACED_SIGNAL) } == 0
        })
        .count()
}

#[cfg(not(unix))]
fn close_clients(_pids: &[u32]) -> usize {
    0
}

fn niri_json(args: &[&str]) -> anyhow::Result<serde_json::Value> {
    let output = std::process::Command::new("niri")
        .arg("msg")
        .arg("-j")
        .args(args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "niri exited with {}",
        output.status
    );
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn niri_action(args: &[String]) -> bool {
    std::process::Command::new("niri")
        .args(["msg", "action"])
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// The process and its ancestors, nearest first.
fn process_ancestry(pid: u32) -> Vec<u32> {
    let mut chain = Vec::new();
    let mut pid = pid;
    while pid > 1 && !chain.contains(&pid) {
        chain.push(pid);
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            break;
        };
        // The command name may contain spaces and parentheses; the parent pid
        // is the second field after the final ')'.
        let Some(parent) = stat
            .rsplit_once(')')
            .and_then(|(_, rest)| rest.split_whitespace().nth(1))
        else {
            break;
        };
        let Ok(parent) = parent.parse::<u32>() else {
            break;
        };
        pid = parent;
    }
    chain
}

/// A move the compositor should perform, derived from its JSON state.
#[derive(Debug, PartialEq, Eq)]
struct NiriMove {
    window_id: u64,
    /// Set when the window sits on another monitor.
    output: Option<String>,
    /// Set when the window is not already on the focused workspace.
    workspace_idx: Option<u64>,
}

/// Pick the single window owned by one of `ancestries` and work out how to get
/// it onto the focused workspace. `None` when no window, or more than one,
/// matches: a terminal that hosts several windows in one process is ambiguous,
/// and moving the wrong one is worse than not moving any.
fn plan_niri_move(
    windows: &serde_json::Value,
    workspaces: &serde_json::Value,
    ancestries: &[Vec<u32>],
) -> Option<NiriMove> {
    let windows = windows.as_array()?;
    let workspaces = workspaces.as_array()?;
    let window_pid = |window: &serde_json::Value| window.get("pid").and_then(|pid| pid.as_u64());
    // Nearest ancestor that owns a window, for the first client that has one.
    let owner = ancestries.iter().find_map(|chain| {
        chain.iter().copied().find(|pid| {
            windows
                .iter()
                .any(|window| window_pid(window) == Some(u64::from(*pid)))
        })
    })?;
    let mut owned = windows
        .iter()
        .filter(|window| window_pid(window) == Some(u64::from(owner)));
    let window = owned.next()?;
    if owned.next().is_some() {
        return None;
    }
    let window_id = window.get("id")?.as_u64()?;
    let window_workspace = window.get("workspace_id").and_then(|id| id.as_u64());

    let focused = workspaces
        .iter()
        .find(|ws| ws.get("is_focused").and_then(|f| f.as_bool()) == Some(true))?;
    let focused_id = focused.get("id")?.as_u64()?;
    if window_workspace == Some(focused_id) {
        return Some(NiriMove {
            window_id,
            output: None,
            workspace_idx: None,
        });
    }
    let output_of = |ws: &serde_json::Value| {
        ws.get("output")
            .and_then(|output| output.as_str())
            .map(str::to_string)
    };
    let focused_output = output_of(focused);
    let window_output = workspaces
        .iter()
        .find(|ws| ws.get("id").and_then(|id| id.as_u64()) == window_workspace)
        .and_then(output_of);
    Some(NiriMove {
        window_id,
        output: focused_output.filter(|output| Some(output) != window_output.as_ref()),
        workspace_idx: Some(focused.get("idx")?.as_u64()?),
    })
}

/// Bring the window hosting one of `client_pids` to the focused workspace and
/// focus it. Returns false when there is no niri, or no unambiguous window.
fn bring_client_window_here(client_pids: &[u32]) -> bool {
    if cfg!(test) || std::env::var_os("NIRI_SOCKET").is_none() {
        return false;
    }
    let (Ok(windows), Ok(workspaces)) = (niri_json(&["windows"]), niri_json(&["workspaces"]))
    else {
        return false;
    };
    let ancestries: Vec<Vec<u32>> = client_pids
        .iter()
        .map(|pid| process_ancestry(*pid))
        .collect();
    let Some(plan) = plan_niri_move(&windows, &workspaces, &ancestries) else {
        return false;
    };
    let id = plan.window_id.to_string();
    if let Some(output) = plan.output {
        niri_action(&[
            "move-window-to-monitor".into(),
            "--id".into(),
            id.clone(),
            output,
        ]);
    }
    if let Some(idx) = plan.workspace_idx
        && !niri_action(&[
            "move-window-to-workspace".into(),
            "--window-id".into(),
            id.clone(),
            "--focus".into(),
            "false".into(),
            idx.to_string(),
        ])
    {
        return false;
    }
    niri_action(&["focus-window".into(), "--id".into(), id])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn workspaces() -> serde_json::Value {
        json!([
            {"id": 1, "idx": 1, "output": "eDP-1", "is_focused": false},
            {"id": 4, "idx": 2, "output": "eDP-1", "is_focused": true},
            {"id": 9, "idx": 1, "output": "DP-2", "is_focused": false},
        ])
    }

    #[test]
    fn moves_window_from_another_workspace_on_the_same_monitor() {
        let windows = json!([
            {"id": 20, "pid": 500, "workspace_id": 1},
            {"id": 21, "pid": 600, "workspace_id": 4},
        ]);
        // Client 777 runs under shell 700 inside terminal 500.
        let plan = plan_niri_move(&windows, &workspaces(), &[vec![777, 700, 500, 2]]);
        assert_eq!(
            plan,
            Some(NiriMove {
                window_id: 20,
                output: None,
                workspace_idx: Some(2),
            })
        );
    }

    #[test]
    fn window_on_another_monitor_changes_output_first() {
        let windows = json!([{"id": 30, "pid": 500, "workspace_id": 9}]);
        let plan = plan_niri_move(&windows, &workspaces(), &[vec![777, 500]]).unwrap();
        assert_eq!(plan.output.as_deref(), Some("eDP-1"));
        assert_eq!(plan.workspace_idx, Some(2));
    }

    #[test]
    fn window_already_here_is_only_focused() {
        let windows = json!([{"id": 21, "pid": 600, "workspace_id": 4}]);
        let plan = plan_niri_move(&windows, &workspaces(), &[vec![601, 600]]).unwrap();
        assert_eq!(plan.workspace_idx, None);
        assert_eq!(plan.output, None);
    }

    #[test]
    fn ambiguous_or_missing_windows_are_not_moved() {
        // One terminal process hosting two windows: cannot tell which one.
        let shared = json!([
            {"id": 1, "pid": 500, "workspace_id": 1},
            {"id": 2, "pid": 500, "workspace_id": 1},
        ]);
        assert_eq!(
            plan_niri_move(&shared, &workspaces(), &[vec![777, 500]]),
            None
        );
        let unrelated = json!([{"id": 1, "pid": 900, "workspace_id": 1}]);
        assert_eq!(
            plan_niri_move(&unrelated, &workspaces(), &[vec![777, 500]]),
            None
        );
    }

    #[test]
    fn ancestry_starts_at_the_process_and_reaches_its_parent() {
        let chain = process_ancestry(std::process::id());
        assert_eq!(chain.first(), Some(&std::process::id()));
        if cfg!(target_os = "linux") {
            assert!(chain.len() > 1);
        }
    }
}
