//! Shared, best-effort sidebar metadata for the bridge and older local daemons.

use crate::SessionInfo;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Deserialize)]
struct Snapshot {
    #[serde(default)]
    updated_at_unix_ms: u64,
    #[serde(default)]
    members: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct Member {
    session_id: String,
    #[serde(default)]
    report_back_to_session_id: Option<String>,
    #[serde(default)]
    task_label: Option<String>,
    #[serde(default)]
    status: Option<String>,
}

/// Fill missing session ownership, task labels and lifecycle statuses from
/// local durable swarm snapshots. Useful with older local daemons that do not
/// yet return these fields. This is opt-in: never use local metadata to enrich
/// a remote daemon's session list.
///
/// Respects `JCODE_RUNTIME_DIR` (isolated `durable-state/swarm`), then
/// `JCODE_HOME` (or the default `~/.jcode`) under `state/swarm`. Missing or
/// malformed snapshots are harmless. Values already supplied by the API win.
/// Only swarm ownership is used, never a transcript's ordinary fork parent.
/// Statuses describe the last persisted snapshot, not a live liveness probe.
pub fn enrich_sessions_from_local_swarm_state(sessions: &mut [SessionInfo]) {
    let dir = if let Ok(runtime) = std::env::var("JCODE_RUNTIME_DIR") {
        std::path::PathBuf::from(runtime).join("durable-state")
    } else {
        let home = std::env::var_os("JCODE_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .or_else(|| std::env::var_os("USERPROFILE"))
                    .map(|home| std::path::PathBuf::from(home).join(".jcode"))
            });
        let Some(home) = home else { return };
        home.join("state")
    };
    enrich_sessions_from_swarm_state(sessions, dir.join("swarm"));
}

/// Like [`enrich_sessions_from_local_swarm_state`], but reads an explicit
/// directory of swarm snapshot JSON files. Does not read session transcripts,
/// modify persisted state, add sessions, or replace existing API metadata.
pub fn enrich_sessions_from_swarm_state(sessions: &mut [SessionInfo], dir: impl AsRef<Path>) {
    if sessions.is_empty() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<_> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    let mut members: BTreeMap<String, (u64, Member)> = BTreeMap::new();
    for path in paths {
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let snapshot = std::fs::File::open(path).ok().and_then(|file| {
            serde_json::from_reader::<_, Snapshot>(std::io::BufReader::new(file)).ok()
        });
        let Some(snapshot) = snapshot else { continue };
        for value in snapshot.members {
            let Ok(member) = serde_json::from_value::<Member>(value) else {
                continue;
            };
            // A member may appear in old snapshots after changing swarms.
            // Choose the newest record as a whole, not a mix of old fields.
            if members
                .get(&member.session_id)
                .is_none_or(|(updated, _)| snapshot.updated_at_unix_ms >= *updated)
            {
                members.insert(
                    member.session_id.clone(),
                    (snapshot.updated_at_unix_ms, member),
                );
            }
        }
    }
    for session in sessions {
        let Some((_, member)) = members.get(&session.session_id) else {
            continue;
        };
        if session.parent_session_id.is_none() {
            session.parent_session_id = member
                .report_back_to_session_id
                .clone()
                .filter(|parent| !parent.trim().is_empty() && parent != &session.session_id);
        }
        if session.agent_label.is_none() {
            session.agent_label = member
                .task_label
                .clone()
                .filter(|label| !label.trim().is_empty());
        }
        if session.swarm_status.is_none() {
            session.swarm_status = member
                .status
                .clone()
                .filter(|status| !status.trim().is_empty());
        }
    }
}

#[cfg(test)]
#[path = "harness_api_tests/swarm_metadata.rs"]
mod tests;
