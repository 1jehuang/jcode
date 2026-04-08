use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestartSnapshot {
    pub version: u32,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub auto_restore_on_next_start: bool,
    pub sessions: Vec<RestartSnapshotSession>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestartSnapshotSession {
    pub session_id: String,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    #[serde(default)]
    pub is_selfdev: bool,
}

#[derive(Debug, Clone)]
pub struct RestoreLaunchOutcome {
    pub session: RestartSnapshotSession,
    pub launched: bool,
    pub command: String,
}

#[derive(Debug, Clone)]
pub struct RestoreSnapshotResult {
    pub snapshot: RestartSnapshot,
    pub outcomes: Vec<RestoreLaunchOutcome>,
}

pub fn snapshot_path() -> Result<PathBuf> {
    Ok(crate::storage::jcode_dir()?.join("restart-snapshot.json"))
}

pub fn save_current_snapshot() -> Result<RestartSnapshot> {
    let snapshot = capture_current_snapshot()?;
    write_snapshot(&snapshot)?;
    Ok(snapshot)
}

pub fn write_snapshot(snapshot: &RestartSnapshot) -> Result<()> {
    crate::storage::write_json(&snapshot_path()?, snapshot)
}

pub fn load_snapshot() -> Result<RestartSnapshot> {
    crate::storage::read_json(&snapshot_path()?)
}

pub fn clear_snapshot() -> Result<bool> {
    let path = snapshot_path()?;
    if !path.exists() {
        return Ok(false);
    }
    std::fs::remove_file(path)?;
    Ok(true)
}

pub fn set_auto_restore_on_next_start(enabled: bool) -> Result<bool> {
    let mut snapshot = match load_snapshot() {
        Ok(snapshot) => snapshot,
        Err(_) => return Ok(false),
    };
    snapshot.auto_restore_on_next_start = enabled;
    write_snapshot(&snapshot)?;
    Ok(true)
}

pub fn capture_current_snapshot() -> Result<RestartSnapshot> {
    let mut unique_ids = HashSet::new();
    let mut captured: Vec<(DateTime<Utc>, RestartSnapshotSession)> = Vec::new();

    for session_id in crate::session::active_session_ids() {
        if !unique_ids.insert(session_id.clone()) {
            continue;
        }

        let Ok(mut session) = crate::session::Session::load(&session_id) else {
            continue;
        };

        if session.detect_crash() {
            let _ = session.save();
            continue;
        }

        if !matches!(session.status, crate::session::SessionStatus::Active) {
            continue;
        }

        let sort_key = session.last_active_at.unwrap_or(session.updated_at);
        captured.push((
            sort_key,
            RestartSnapshotSession {
                session_id: session.id.clone(),
                display_name: session.display_name().to_string(),
                working_dir: session.working_dir.clone(),
                is_selfdev: session.is_canary,
            },
        ));
    }

    captured.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.display_name.cmp(&b.1.display_name))
            .then_with(|| a.1.session_id.cmp(&b.1.session_id))
    });

    Ok(RestartSnapshot {
        version: 1,
        created_at: Utc::now(),
        auto_restore_on_next_start: false,
        sessions: captured.into_iter().map(|(_, session)| session).collect(),
    })
}

pub fn restore_snapshot(exe: &Path) -> Result<RestoreSnapshotResult> {
    let snapshot = load_snapshot()?;
    let mut outcomes = Vec::new();

    for session in &snapshot.sessions {
        let cwd = resolve_session_cwd(session.working_dir.as_deref());
        let launched = if session.is_selfdev {
            crate::cli::tui_launch::spawn_selfdev_in_new_terminal(exe, &session.session_id, &cwd)?
        } else {
            crate::cli::tui_launch::spawn_resume_in_new_terminal(exe, &session.session_id, &cwd)?
        };
        outcomes.push(RestoreLaunchOutcome {
            session: session.clone(),
            launched,
            command: restore_command_display(exe, session),
        });
    }

    Ok(RestoreSnapshotResult { snapshot, outcomes })
}

fn resolve_session_cwd(configured: Option<&str>) -> PathBuf {
    configured
        .filter(|path| Path::new(path).is_dir())
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

fn shell_escape(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\"'\"'"))
}

pub fn restore_command_display(exe: &Path, session: &RestartSnapshotSession) -> String {
    let exe = shell_escape(exe.to_string_lossy().as_ref());
    if session.is_selfdev {
        format!("{} --resume {} self-dev", exe, session.session_id)
    } else {
        format!("{} --resume {}", exe, session.session_id)
    }
}

#[cfg(test)]
mod tests {
    use super::{capture_current_snapshot, clear_snapshot, load_snapshot, save_current_snapshot};
    use crate::session::Session;
    use std::ffi::OsString;

    struct TestEnvGuard {
        prev_home: Option<OsString>,
        _temp_home: tempfile::TempDir,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl TestEnvGuard {
        fn new() -> anyhow::Result<Self> {
            let lock = crate::storage::lock_test_env();
            let temp_home = tempfile::Builder::new()
                .prefix("jcode-restart-snapshot-test-home-")
                .tempdir()?;
            let prev_home = std::env::var_os("JCODE_HOME");
            crate::env::set_var("JCODE_HOME", temp_home.path());
            Ok(Self {
                prev_home,
                _temp_home: temp_home,
                _lock: lock,
            })
        }
    }

    impl Drop for TestEnvGuard {
        fn drop(&mut self) {
            if let Some(prev_home) = &self.prev_home {
                crate::env::set_var("JCODE_HOME", prev_home);
            } else {
                crate::env::remove_var("JCODE_HOME");
            }
        }
    }

    #[test]
    fn capture_current_snapshot_includes_active_sessions_only() {
        let _guard = TestEnvGuard::new().expect("setup test env");

        let mut active = Session::create(None, Some("Active".to_string()));
        active.working_dir = Some("/tmp".to_string());
        active.mark_active_with_pid(std::process::id());
        active.save().expect("save active session");

        let mut closed = Session::create(None, Some("Closed".to_string()));
        closed.mark_closed();
        closed.save().expect("save closed session");

        let snapshot = capture_current_snapshot().expect("capture snapshot");
        assert_eq!(snapshot.sessions.len(), 1);
        assert_eq!(snapshot.sessions[0].session_id, active.id);
        assert_eq!(snapshot.sessions[0].working_dir.as_deref(), Some("/tmp"));
    }

    #[test]
    fn save_and_load_snapshot_round_trip() {
        let _guard = TestEnvGuard::new().expect("setup test env");

        let mut active = Session::create(None, Some("Restore Me".to_string()));
        active.mark_active_with_pid(std::process::id());
        active.save().expect("save active session");

        let saved = save_current_snapshot().expect("save snapshot");
        let loaded = load_snapshot().expect("load snapshot");
        assert_eq!(saved.sessions.len(), 1);
        assert_eq!(loaded.sessions.len(), 1);
        assert!(!loaded.auto_restore_on_next_start);
        assert_eq!(loaded.sessions[0].session_id, active.id);
    }

    #[test]
    fn set_auto_restore_updates_saved_snapshot() {
        let _guard = TestEnvGuard::new().expect("setup test env");

        let mut active = Session::create(None, Some("Auto Restore".to_string()));
        active.mark_active_with_pid(std::process::id());
        active.save().expect("save active session");
        save_current_snapshot().expect("save snapshot");

        assert!(super::set_auto_restore_on_next_start(true).expect("set auto restore"));
        let loaded = load_snapshot().expect("load snapshot");
        assert!(loaded.auto_restore_on_next_start);
    }

    #[test]
    fn clear_snapshot_removes_saved_file() {
        let _guard = TestEnvGuard::new().expect("setup test env");

        let mut active = Session::create(None, Some("Clear Me".to_string()));
        active.mark_active_with_pid(std::process::id());
        active.save().expect("save active session");
        save_current_snapshot().expect("save snapshot");

        assert!(clear_snapshot().expect("clear snapshot"));
        assert!(load_snapshot().is_err());
    }
}
