//! Human-readable swarm labels and cross-swarm target resolution.
//!
//! Swarm ids are opaque (`session:<uuid>`, a git dir, or `KCODE_SWARM_ID`), so
//! agents in one swarm cannot reasonably address another swarm by id. Labels
//! give each swarm a short, unique, human-readable name ("frontend",
//! "release-train") that cross-swarm DMs and `list_swarms` use.
//!
//! Labels are keyed by swarm id and persisted in one small registry file under
//! the durable state dir so they survive server reloads and restarts.

use super::SwarmMember;
use crate::protocol::SwarmInfo;
use crate::storage;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex as StdMutex};
use tokio::sync::RwLock;

const LABELS_FILE: &str = "swarm-labels.json";
pub(super) const MAX_SWARM_LABEL_CHARS: usize = 48;

static LABELS: LazyLock<StdMutex<Option<HashMap<String, String>>>> =
    LazyLock::new(|| StdMutex::new(None));

#[cfg(not(test))]
fn labels_path() -> PathBuf {
    storage::durable_state_dir().join(LABELS_FILE)
}

#[cfg(test)]
fn labels_path() -> PathBuf {
    std::env::temp_dir()
        .join(format!("kcode-test-state-{}", std::process::id()))
        .join(LABELS_FILE)
}

fn with_labels<R>(f: impl FnOnce(&mut HashMap<String, String>) -> R) -> R {
    let mut guard = LABELS.lock().unwrap_or_else(|p| p.into_inner());
    let labels = guard.get_or_insert_with(|| {
        storage::read_json::<HashMap<String, String>>(&labels_path()).unwrap_or_default()
    });
    f(labels)
}

fn persist(labels: &HashMap<String, String>) {
    if let Err(err) = storage::write_json_fast(&labels_path(), labels) {
        crate::logging::warn(&format!("Failed to persist swarm labels: {err}"));
    }
}

pub(super) fn swarm_label(swarm_id: &str) -> Option<String> {
    with_labels(|labels| labels.get(swarm_id).cloned())
}

/// Normalize a user-provided label: trimmed, internal whitespace collapsed.
pub(super) fn normalize_swarm_label(label: &str) -> String {
    label.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Set or clear (empty label) the label of `swarm_id`. Labels are unique
/// case-insensitively across swarms, and must not collide with another
/// swarm's id so resolution stays unambiguous.
pub(super) fn set_swarm_label(
    swarm_id: &str,
    label: &str,
    known_swarm_ids: &HashSet<String>,
) -> anyhow::Result<Option<String>> {
    let label = normalize_swarm_label(label);
    if label.chars().count() > MAX_SWARM_LABEL_CHARS {
        anyhow::bail!("Swarm label must be at most {MAX_SWARM_LABEL_CHARS} characters.");
    }
    with_labels(|labels| {
        if label.is_empty() {
            if labels.remove(swarm_id).is_some() {
                persist(labels);
            }
            return Ok(None);
        }
        if known_swarm_ids
            .iter()
            .any(|other| other != swarm_id && other == &label)
        {
            anyhow::bail!("Swarm label '{label}' collides with another swarm's id.");
        }
        if let Some((other, _)) = labels
            .iter()
            .find(|(other, existing)| *other != swarm_id && existing.eq_ignore_ascii_case(&label))
        {
            anyhow::bail!("Swarm label '{label}' is already used by swarm '{other}'.");
        }
        labels.insert(swarm_id.to_string(), label.clone());
        persist(labels);
        Ok(Some(label))
    })
}

/// Resolve a cross-swarm target (label, case-insensitive, or exact swarm id)
/// to a live swarm id.
pub(super) fn resolve_swarm_target(
    target: &str,
    live_swarm_ids: &HashSet<String>,
) -> anyhow::Result<String> {
    let target = target.trim();
    if target.is_empty() {
        anyhow::bail!("'to_swarm' must not be blank.");
    }
    if live_swarm_ids.contains(target) {
        return Ok(target.to_string());
    }
    let normalized = normalize_swarm_label(target);
    let by_label = with_labels(|labels| {
        labels
            .iter()
            .find(|(_, label)| label.eq_ignore_ascii_case(&normalized))
            .map(|(swarm_id, _)| swarm_id.clone())
    });
    match by_label {
        Some(swarm_id) if live_swarm_ids.contains(&swarm_id) => Ok(swarm_id),
        Some(_) => anyhow::bail!("Swarm '{target}' has no live members right now."),
        None => anyhow::bail!(
            "Unknown swarm '{target}'. Use swarm action=list_swarms to see swarm labels and ids."
        ),
    }
}

/// Build the swarm directory from live membership.
pub(super) async fn list_swarms(
    requester_session: &str,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_coordinators: Option<&Arc<RwLock<HashMap<String, String>>>>,
) -> Vec<SwarmInfo> {
    let swarms = swarms_by_id.read().await.clone();
    let coordinators = match swarm_coordinators {
        Some(coordinators) => coordinators.read().await.clone(),
        None => HashMap::new(),
    };
    let members = swarm_members.read().await;
    let own_swarm = members
        .get(requester_session)
        .and_then(|member| member.swarm_id.clone());
    let mut out: Vec<SwarmInfo> = swarms
        .into_iter()
        .filter(|(_, sessions)| !sessions.is_empty())
        .map(|(swarm_id, sessions)| {
            let coordinator_session_id = coordinators.get(&swarm_id).cloned().or_else(|| {
                sessions
                    .iter()
                    .filter(|id| members.get(*id).is_some_and(|m| m.role == "coordinator"))
                    .min()
                    .cloned()
            });
            let coordinator_name = coordinator_session_id
                .as_ref()
                .and_then(|id| members.get(id))
                .and_then(|member| member.friendly_name.clone());
            SwarmInfo {
                label: swarm_label(&swarm_id),
                is_own: own_swarm.as_deref() == Some(swarm_id.as_str()),
                coordinator_session_id,
                coordinator_name,
                member_count: sessions.len(),
                swarm_id,
            }
        })
        .collect();
    out.sort_by(|a, b| {
        b.is_own
            .cmp(&a.is_own)
            .then_with(|| a.label.is_none().cmp(&b.label.is_none()))
            .then_with(|| a.label.cmp(&b.label))
            .then_with(|| a.swarm_id.cmp(&b.swarm_id))
    });
    out
}

/// Display name for a swarm: its label when set, else its id.
pub(super) fn swarm_display_name(swarm_id: &str) -> String {
    swarm_label(swarm_id).unwrap_or_else(|| swarm_id.to_string())
}

#[cfg(test)]
pub(super) static SWARM_LABELS_TEST_LOCK: StdMutex<()> = StdMutex::new(());

#[cfg(test)]
pub(super) fn reset_swarm_labels_for_test() {
    let mut guard = LABELS.lock().unwrap_or_else(|p| p.into_inner());
    *guard = Some(HashMap::new());
    let _ = std::fs::remove_file(labels_path());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_unique_and_resolve_case_insensitively() {
        let _guard = SWARM_LABELS_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        reset_swarm_labels_for_test();
        let live: HashSet<String> = ["swarm-a", "swarm-b"]
            .iter()
            .map(|s| s.to_string())
            .collect();

        assert_eq!(
            set_swarm_label("swarm-a", "  Front   End ", &live).unwrap(),
            Some("Front End".to_string())
        );
        assert!(set_swarm_label("swarm-b", "front end", &live).is_err());
        assert!(set_swarm_label("swarm-b", "swarm-a", &live).is_err());
        assert_eq!(resolve_swarm_target("FRONT END", &live).unwrap(), "swarm-a");
        assert_eq!(resolve_swarm_target("swarm-b", &live).unwrap(), "swarm-b");
        assert!(resolve_swarm_target("nope", &live).is_err());

        assert_eq!(set_swarm_label("swarm-a", "", &live).unwrap(), None);
        assert!(resolve_swarm_target("front end", &live).is_err());
        reset_swarm_labels_for_test();
    }
}
