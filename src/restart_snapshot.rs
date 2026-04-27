use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

const AUTO_RESTORE_CRASH_MAX_AGE_HOURS: i64 = 24;

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

pub fn arm_auto_restore_from_recent_crashes() -> Result<Option<RestartSnapshot>> {
    let cutoff = Utc::now() - chrono::Duration::hours(AUTO_RESTORE_CRASH_MAX_AGE_HOURS);
    let mut unique_ids = HashSet::new();
    let mut captured: Vec<(DateTime<Utc>, RestartSnapshotSession)> = Vec::new();

    for (session_id, _) in crate::session::find_recent_crashed_sessions() {
        if !unique_ids.insert(session_id.clone()) {
            continue;
        }

        let Ok(session) = crate::session::Session::load(&session_id) else {
            continue;
        };

        if !matches!(
            session.status,
            crate::session::SessionStatus::Crashed { .. }
        ) {
            continue;
        }

        let sort_key = session.last_active_at.unwrap_or(session.updated_at);
        if sort_key < cutoff {
            continue;
        }

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

    if captured.is_empty() {
        return Ok(None);
    }

    captured.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.display_name.cmp(&b.1.display_name))
            .then_with(|| a.1.session_id.cmp(&b.1.session_id))
    });

    let snapshot = RestartSnapshot {
        version: 1,
        created_at: Utc::now(),
        auto_restore_on_next_start: true,
        sessions: captured.into_iter().map(|(_, session)| session).collect(),
    };

    write_snapshot(&snapshot)?;
    Ok(Some(snapshot))
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
#[path = "restart_snapshot_tests.rs"]
mod restart_snapshot_tests;
