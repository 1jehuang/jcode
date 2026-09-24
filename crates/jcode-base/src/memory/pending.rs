use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::Instant;

type LastInjectedMemorySetBySession = HashMap<String, (HashSet<String>, Instant)>;
type InjectedMemoryIdsBySession = HashMap<String, HashMap<String, Instant>>;

/// Pending memory prompt from background check - ready to inject on next turn.
/// Keyed by session ID so each session gets its own pending memory.
static PENDING_MEMORY: Mutex<Option<HashMap<String, QueuedPendingMemory>>> = Mutex::new(None);

/// Keep scope validation private so legacy PendingMemory struct literals and
/// consumers remain source-compatible. None is an unscoped legacy publication,
/// not a global-only publication (whose binding has project_dir = None).
struct QueuedPendingMemory {
    pending: PendingMemory,
    binding: Option<PendingBinding>,
}

#[derive(PartialEq, Eq)]
struct PendingBinding {
    project_dir: Option<String>,
    snapshots: HashMap<String, MemorySnapshot>,
}

#[derive(PartialEq, Eq)]
struct MemorySnapshot {
    global: bool,
    signature: String,
}

/// Signature of the last injected prompt to suppress near-immediate duplicates.
/// Keyed by session ID.
static LAST_INJECTED_PROMPT_SIGNATURE: Mutex<Option<HashMap<String, (String, Instant)>>> =
    Mutex::new(None);

/// Recently injected memory ID sets per session.
/// Used to suppress near-duplicate re-injection even when formatting differs.
static LAST_INJECTED_MEMORY_SET: Mutex<Option<LastInjectedMemorySetBySession>> = Mutex::new(None);

/// Memory IDs that have already been injected into the conversation, with the
/// time they were injected. Used to prevent the same memory from being
/// re-injected on subsequent turns while it is still fresh in the transcript.
/// Keyed by session ID.
static INJECTED_MEMORY_IDS: Mutex<Option<InjectedMemoryIdsBySession>> = Mutex::new(None);

/// Guard to ensure only one memory check runs at a time, per session.
/// Keyed by session ID.
static MEMORY_CHECK_IN_PROGRESS: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// Suppress repeated identical memory payloads within this many seconds.
const MEMORY_REPEAT_SUPPRESSION_SECS: u64 = 90;
/// Suppress substantially overlapping memory sets for a bit longer.
const MEMORY_SET_REPEAT_SUPPRESSION_SECS: u64 = 180;
/// If a new pending payload overlaps this much with the last injected set,
/// treat it as too similar to surface again immediately.
const MEMORY_SET_OVERLAP_SUPPRESSION_RATIO: f32 = 0.8;
/// How long an injected memory counts as "already known" to a session.
///
/// Injection payloads are ephemeral (not persisted into history), but the
/// model's response that consumed them IS part of the transcript, so
/// re-injecting the same memory shortly afterwards is pure noise. This
/// tracking used to be cleared on every detected topic change, which fires
/// often on real sessions (cosine similarity between consecutive coding turns
/// regularly sits below the topic threshold), so the same memory could be
/// re-injected minutes apart in one transcript. A TTL keeps the dedup stable
/// across topic wobble while still letting genuinely old memories resurface in
/// long sessions once they may have scrolled out of (or been compacted from)
/// the context window.
const INJECTED_MEMORY_TTL_SECS: u64 = 45 * 60;

fn injected_recently(at: &Instant) -> bool {
    at.elapsed().as_secs() < INJECTED_MEMORY_TTL_SECS
}

/// A pending memory result from async checking.
#[derive(Debug, Clone)]
pub struct PendingMemory {
    /// The formatted memory prompt ready for injection.
    pub prompt: String,
    /// Optional UI-focused rendering of the injected memory payload.
    /// This can contain extra display-only metadata that is not sent to the model.
    pub display_prompt: Option<String>,
    /// When this was computed.
    pub computed_at: Instant,
    /// Number of relevant memories found.
    pub count: usize,
    /// IDs of memories included in this prompt (for dedup tracking).
    pub memory_ids: Vec<String>,
}

impl PendingMemory {
    /// Check if this pending memory is still fresh (not too old).
    pub fn is_fresh(&self) -> bool {
        self.computed_at.elapsed().as_secs() < 120
    }
}

fn prompt_signature(prompt: &str) -> String {
    prompt
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
        .to_lowercase()
}

fn memory_set(ids: &[String]) -> HashSet<String> {
    ids.iter().cloned().collect()
}

fn memory_overlap_ratio(left: &HashSet<String>, right: &HashSet<String>) -> f32 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }

    let intersection = left.intersection(right).count() as f32;
    let baseline = left.len().max(right.len()) as f32;
    intersection / baseline
}

/// Read the actual graph file, not the mtime cache, and never migrate/write or
/// run inference from the foreground pending-consumption path. Retrieval has
/// already migrated any legacy entries before they can become scoped pending.
fn read_validation_graph(
    path: &std::path::Path,
) -> anyhow::Result<crate::memory_graph::MemoryGraph> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(crate::memory_graph::MemoryGraph::new());
        }
        Err(error) => return Err(error.into()),
    };
    let graph: crate::memory_graph::MemoryGraph = serde_json::from_slice(&bytes)?;
    anyhow::ensure!(
        graph.graph_version == super::GRAPH_VERSION,
        "unsupported memory graph"
    );
    Ok(graph)
}

fn semantic_signature(entry: &super::MemoryEntry) -> anyhow::Result<String> {
    // Include everything sent to Jev plus semantic/rendering metadata. Access
    // counters and embeddings do not change the fact that was judged.
    Ok(serde_json::to_string(&(
        &entry.content,
        &entry.category,
        &entry.tags,
        &entry.source,
        &entry.trust,
        &entry.updated_at,
    ))?)
}

fn snapshot_selected_memories(
    project_dir: Option<&str>,
    ids: &[String],
) -> anyhow::Result<(HashMap<String, MemorySnapshot>, Vec<super::MemoryEntry>)> {
    anyhow::ensure!(!ids.is_empty(), "scoped pending memory requires IDs");
    let manager = match project_dir {
        Some(project) => super::MemoryManager::new().with_project_dir(project),
        None => super::MemoryManager::new(),
    };
    let project = match manager.project_memory_path()? {
        Some(path) => read_validation_graph(&path)?,
        None => crate::memory_graph::MemoryGraph::new(),
    };
    // Do not swallow a corrupt/unreadable store, even if the other store has
    // matching IDs. Ambiguous duplicate IDs are likewise rejected below.
    let global = read_validation_graph(&manager.global_memory_path()?)?;
    let mut snapshots = HashMap::new();
    let mut entries = Vec::with_capacity(ids.len());
    for id in ids {
        let (entry, is_global) = match (project.get_memory(id), global.get_memory(id)) {
            (Some(entry), None) => (entry, false),
            (None, Some(entry)) => (entry, true),
            _ => anyhow::bail!("selected memory is absent or ambiguous"),
        };
        anyhow::ensure!(
            entry.id == *id,
            "selected memory ID does not match its storage key"
        );
        anyhow::ensure!(
            entry.active && entry.superseded_by.is_none(),
            "selected memory is inactive"
        );
        // Compare semantic/rendered metadata exactly, not a collision-prone hash.
        // Access counters and embeddings are deliberately excluded.
        let signature = semantic_signature(entry)?;
        anyhow::ensure!(
            snapshots
                .insert(
                    id.clone(),
                    MemorySnapshot {
                        global: is_global,
                        signature
                    }
                )
                .is_none(),
            "duplicate selected memory ID"
        );
        entries.push(entry.clone());
    }
    Ok((snapshots, entries))
}

/// Take pending memory if available and fresh for the given session.
pub fn take_pending_memory(session_id: &str) -> Option<PendingMemory> {
    take_pending_memory_inner(session_id, None)
}

/// Take pending memory only for the exact current project binding. None means
/// global-only, never the process cwd. Validation happens before dedup or IDs
/// are marked injected, and failed payloads are removed rather than recycled.
pub fn take_pending_memory_for_project(
    session_id: &str,
    project_dir: Option<&str>,
) -> Option<PendingMemory> {
    take_pending_memory_inner(session_id, Some(project_dir))
}

fn take_pending_memory_inner(
    session_id: &str,
    expected_project: Option<Option<&str>>,
) -> Option<PendingMemory> {
    let queued = {
        let mut guard = PENDING_MEMORY.lock().ok()?;
        guard.as_mut()?.remove(session_id)?
    };
    if let Some(expected) = expected_project
        && queued
            .binding
            .as_ref()
            .map(|binding| binding.project_dir.as_deref())
            != Some(expected)
    {
        crate::memory_log::log_pending_discarded(session_id, "project binding changed or missing");
        return None;
    }
    if !queued.pending.is_fresh() {
        crate::memory_log::log_pending_discarded(session_id, "stale (>120s)");
        return None;
    }
    if let Some(binding) = &queued.binding {
        let valid =
            snapshot_selected_memories(binding.project_dir.as_deref(), &queued.pending.memory_ids)
                .is_ok_and(|(current, _)| current == binding.snapshots);
        if !valid {
            crate::memory_log::log_pending_discarded(
                session_id,
                "selected memory changed, forgotten, or unreadable",
            );
            return None;
        }
    }
    let pending = queued.pending;

    // If every memory in this payload is still fresh in the session's
    // injected set, the model already knows all of it; do not re-inject
    // just because formatting or ranking shifted slightly.
    if !pending.memory_ids.is_empty()
        && pending
            .memory_ids
            .iter()
            .all(|id| is_memory_injected(session_id, id))
    {
        crate::memory_log::log_pending_discarded(
            session_id,
            "all memories already known to session",
        );
        return None;
    }

    let sig = prompt_signature(&pending.prompt);
    if let Ok(mut last_guard) = LAST_INJECTED_PROMPT_SIGNATURE.lock() {
        let sig_map = last_guard.get_or_insert_with(HashMap::new);
        if let Some((last_sig, last_at)) = sig_map.get(session_id)
            && *last_sig == sig
            && last_at.elapsed().as_secs() < MEMORY_REPEAT_SUPPRESSION_SECS
        {
            crate::memory_log::log_pending_discarded(session_id, "duplicate suppressed");
            return None;
        }
        sig_map.insert(session_id.to_string(), (sig, Instant::now()));
    }

    if !pending.memory_ids.is_empty() {
        let pending_set = memory_set(&pending.memory_ids);
        if let Ok(mut last_guard) = LAST_INJECTED_MEMORY_SET.lock() {
            let set_map = last_guard.get_or_insert_with(HashMap::new);
            if let Some((last_set, last_at)) = set_map.get(session_id) {
                let overlap = memory_overlap_ratio(last_set, &pending_set);
                if overlap >= MEMORY_SET_OVERLAP_SUPPRESSION_RATIO
                    && last_at.elapsed().as_secs() < MEMORY_SET_REPEAT_SUPPRESSION_SECS
                {
                    crate::memory_log::log_pending_discarded(
                        session_id,
                        "overlapping memory set suppressed",
                    );
                    return None;
                }
            }
            set_map.insert(session_id.to_string(), (pending_set, Instant::now()));
        }
    }

    if !pending.memory_ids.is_empty() {
        mark_memories_injected(session_id, &pending.memory_ids);
    }

    crate::memory_log::log_pending_consumed(
        session_id,
        pending.count,
        pending.computed_at.elapsed().as_millis() as u64,
        pending.prompt.chars().count(),
    );

    Some(pending)
}

/// Store a pending memory result for the given session.
pub fn set_pending_memory(session_id: &str, prompt: String, count: usize) {
    set_pending_memory_with_ids(session_id, prompt, count, Vec::new());
}

/// Store a pending memory result with associated memory IDs for dedup tracking.
pub fn set_pending_memory_with_ids(
    session_id: &str,
    prompt: String,
    count: usize,
    memory_ids: Vec<String>,
) {
    set_pending_memory_with_ids_and_display(session_id, prompt, count, memory_ids, None);
}

/// Store a pending memory result with associated memory IDs and optional display-only content.
pub fn set_pending_memory_with_ids_and_display(
    session_id: &str,
    prompt: String,
    count: usize,
    memory_ids: Vec<String>,
    display_prompt: Option<String>,
) {
    store_pending_memory(session_id, prompt, count, memory_ids, display_prompt, None);
}

/// Publish a result bound to the selected project and its current memory
/// content. Verify the canonical prompt too, closing the race where a fact was
/// edited during the async judge call, before this publication could snapshot it.
pub fn set_pending_memory_for_project(
    session_id: &str,
    prompt: String,
    count: usize,
    memory_ids: Vec<String>,
    display_prompt: Option<String>,
    project_dir: Option<&str>,
) {
    publish_scoped_memory(
        session_id,
        prompt,
        count,
        memory_ids,
        display_prompt,
        project_dir,
        None,
    );
}

/// Automatic publishers must retain the entries actually judged by Jev. A
/// canonical prompt alone cannot detect tags-only edits while inference was in
/// flight, because tags are model input but not part of the rendered prompt.
pub(crate) fn set_pending_memory_for_project_with_selection(
    session_id: &str,
    prompt: String,
    count: usize,
    selected_entries: &[super::MemoryEntry],
    display_prompt: Option<String>,
    project_dir: Option<&str>,
) {
    let memory_ids = selected_entries
        .iter()
        .map(|entry| entry.id.clone())
        .collect();
    publish_scoped_memory(
        session_id,
        prompt,
        count,
        memory_ids,
        display_prompt,
        project_dir,
        Some(selected_entries),
    );
}

fn publish_scoped_memory(
    session_id: &str,
    prompt: String,
    count: usize,
    memory_ids: Vec<String>,
    display_prompt: Option<String>,
    project_dir: Option<&str>,
    selected_entries: Option<&[super::MemoryEntry]>,
) {
    let Ok((snapshots, entries)) = snapshot_selected_memories(project_dir, &memory_ids) else {
        crate::memory_log::log_pending_discarded(
            session_id,
            "cannot validate selected memory at publication",
        );
        return;
    };
    if let Some(selected) = selected_entries {
        let matches_selection = selected.len() == memory_ids.len()
            && selected.iter().zip(&memory_ids).all(|(entry, id)| {
                entry.id == *id
                    && entry.active
                    && entry.superseded_by.is_none()
                    && semantic_signature(entry).is_ok_and(|signature| {
                        snapshots
                            .get(id)
                            .is_some_and(|current| current.signature == signature)
                    })
            });
        if !matches_selection {
            crate::memory_log::log_pending_discarded(
                session_id,
                "selected memory metadata changed during relevance evaluation",
            );
            return;
        }
    }
    if super::format_relevant_prompt(&entries, entries.len())
        .as_deref()
        .map(str::trim)
        != Some(prompt.trim())
    {
        crate::memory_log::log_pending_discarded(
            session_id,
            "selected memory changed before publication",
        );
        return;
    }
    store_pending_memory(
        session_id,
        prompt,
        count,
        memory_ids,
        display_prompt,
        Some(PendingBinding {
            project_dir: project_dir.map(str::to_owned),
            snapshots,
        }),
    );
}

fn store_pending_memory(
    session_id: &str,
    prompt: String,
    count: usize,
    memory_ids: Vec<String>,
    display_prompt: Option<String>,
    binding: Option<PendingBinding>,
) {
    crate::memory_log::log_pending_prepared(session_id, &prompt, count, &memory_ids);

    if let Ok(mut guard) = PENDING_MEMORY.lock() {
        let map = guard.get_or_insert_with(HashMap::new);
        let new_sig = prompt_signature(&prompt);
        let new_memory_set = memory_set(&memory_ids);

        if let Some(existing) = map.get(session_id)
            && existing.pending.is_fresh()
            && existing.binding == binding
        {
            let existing_sig = prompt_signature(&existing.pending.prompt);
            let overlap =
                memory_overlap_ratio(&memory_set(&existing.pending.memory_ids), &new_memory_set);
            if existing_sig == new_sig || overlap >= MEMORY_SET_OVERLAP_SUPPRESSION_RATIO {
                crate::memory_log::log_pending_discarded(
                    session_id,
                    "similar pending payload already queued",
                );
                return;
            }
        }

        map.insert(
            session_id.to_string(),
            QueuedPendingMemory {
                pending: PendingMemory {
                    prompt,
                    display_prompt,
                    computed_at: Instant::now(),
                    count,
                    memory_ids,
                },
                binding,
            },
        );
    }
}

/// Mark memory IDs as already injected for a session (prevents re-injection on future turns).
pub fn mark_memories_injected(session_id: &str, ids: &[String]) {
    crate::memory_log::log_marked_injected(session_id, ids);
    insert_injected_ids(session_id, ids);
}

/// Mark memory IDs as already KNOWN to a session without them having been
/// injected, e.g. because they were just extracted from this session's own
/// transcript. The conversation already contains this information, so
/// re-injecting it would be a pure echo.
pub fn mark_memories_known(session_id: &str, ids: &[String], reason: &str) {
    if ids.is_empty() {
        return;
    }
    crate::memory_log::log_marked_known(session_id, ids, reason);
    insert_injected_ids(session_id, ids);
}

fn insert_injected_ids(session_id: &str, ids: &[String]) {
    if let Ok(mut guard) = INJECTED_MEMORY_IDS.lock() {
        let outer = guard.get_or_insert_with(HashMap::new);
        let set = outer
            .entry(session_id.to_string())
            .or_insert_with(HashMap::new);
        set.retain(|_, at| injected_recently(at));
        let now = Instant::now();
        for id in ids {
            set.insert(id.clone(), now);
        }
        crate::logging::info(&format!(
            "[{}] Marked {} memory IDs as injected (total tracked: {})",
            session_id,
            ids.len(),
            set.len()
        ));
    }
}

/// Replace injected memory tracking for a session with the provided IDs.
/// Used when restoring persisted session state so the same logical session does
/// not re-inject memories after reload/resume.
pub fn sync_injected_memories(session_id: &str, ids: &[String]) {
    if let Ok(mut guard) = INJECTED_MEMORY_IDS.lock() {
        let outer = guard.get_or_insert_with(HashMap::new);
        if ids.is_empty() {
            outer.remove(session_id);
            return;
        }

        let now = Instant::now();
        outer.insert(
            session_id.to_string(),
            ids.iter().cloned().map(|id| (id, now)).collect(),
        );
    }
}

/// Check if a memory ID has already been injected for a session.
/// An injected ID "expires" after [`INJECTED_MEMORY_TTL_SECS`], at which point
/// the memory may be surfaced again.
pub fn is_memory_injected(session_id: &str, id: &str) -> bool {
    if let Ok(guard) = INJECTED_MEMORY_IDS.lock()
        && let Some(outer) = guard.as_ref()
        && let Some(set) = outer.get(session_id)
        && let Some(at) = set.get(id)
    {
        return injected_recently(at);
    }
    false
}

/// Check if a memory ID has already been injected in ANY session.
/// Used by the singleton memory agent which doesn't track per-session state.
pub fn is_memory_injected_any(id: &str) -> bool {
    if let Ok(guard) = INJECTED_MEMORY_IDS.lock()
        && let Some(outer) = guard.as_ref()
    {
        return outer
            .values()
            .any(|set| set.get(id).is_some_and(injected_recently));
    }
    false
}

/// Clear injected memory tracking for a session (call on session reset or topic change).
pub fn clear_injected_memories(session_id: &str) {
    if let Ok(mut guard) = LAST_INJECTED_PROMPT_SIGNATURE.lock()
        && let Some(map) = guard.as_mut()
    {
        map.remove(session_id);
    }
    if let Ok(mut guard) = LAST_INJECTED_MEMORY_SET.lock()
        && let Some(map) = guard.as_mut()
    {
        map.remove(session_id);
    }

    if let Ok(mut guard) = INJECTED_MEMORY_IDS.lock()
        && let Some(outer) = guard.as_mut()
        && let Some(set) = outer.remove(session_id)
        && !set.is_empty()
    {
        crate::logging::info(&format!(
            "[{}] Clearing {} tracked injected memory IDs",
            session_id,
            set.len()
        ));
    }
}

/// Clear all injected memory tracking across all sessions.
pub fn clear_all_injected_memories() {
    if let Ok(mut guard) = LAST_INJECTED_PROMPT_SIGNATURE.lock() {
        *guard = None;
    }
    if let Ok(mut guard) = LAST_INJECTED_MEMORY_SET.lock() {
        *guard = None;
    }

    if let Ok(mut guard) = INJECTED_MEMORY_IDS.lock() {
        if let Some(outer) = guard.as_ref() {
            let total: usize = outer.values().map(|s| s.len()).sum();
            if total > 0 {
                crate::logging::info(&format!(
                    "Clearing {} tracked injected memory IDs across {} sessions",
                    total,
                    outer.len()
                ));
            }
        }
        *guard = None;
    }
}

/// Clear any pending memory result for a session.
pub fn clear_pending_memory(session_id: &str) {
    if let Ok(mut guard) = PENDING_MEMORY.lock()
        && let Some(map) = guard.as_mut()
    {
        map.remove(session_id);
    }
    if let Ok(mut guard) = LAST_INJECTED_PROMPT_SIGNATURE.lock()
        && let Some(map) = guard.as_mut()
    {
        map.remove(session_id);
    }
    if let Ok(mut guard) = LAST_INJECTED_MEMORY_SET.lock()
        && let Some(map) = guard.as_mut()
    {
        map.remove(session_id);
    }
    clear_injected_memories(session_id);
}

/// Clear all pending memory state across all sessions.
pub fn clear_all_pending_memory() {
    if let Ok(mut guard) = PENDING_MEMORY.lock() {
        *guard = None;
    }
    if let Ok(mut guard) = LAST_INJECTED_PROMPT_SIGNATURE.lock() {
        *guard = None;
    }
    if let Ok(mut guard) = LAST_INJECTED_MEMORY_SET.lock() {
        *guard = None;
    }
    clear_all_injected_memories();
}

/// Check if there's a pending memory for a specific session.
pub fn has_pending_memory(session_id: &str) -> bool {
    PENDING_MEMORY
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|m| m.contains_key(session_id)))
        .unwrap_or(false)
}

/// Check if there's any pending memory across all sessions.
pub fn has_any_pending_memory() -> bool {
    PENDING_MEMORY
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|m| !m.is_empty()))
        .unwrap_or(false)
}

pub(super) fn begin_memory_check(session_id: &str) -> bool {
    if let Ok(mut guard) = MEMORY_CHECK_IN_PROGRESS.lock() {
        let set = guard.get_or_insert_with(HashSet::new);
        return set.insert(session_id.to_string());
    }
    false
}

pub(super) fn finish_memory_check(session_id: &str) {
    if let Ok(mut guard) = MEMORY_CHECK_IN_PROGRESS.lock()
        && let Some(set) = guard.as_mut()
    {
        set.remove(session_id);
    }
}

#[cfg(test)]
pub(super) fn insert_pending_memory_for_test(session_id: &str, pending: PendingMemory) {
    let mut guard = PENDING_MEMORY.lock().expect("pending memory lock");
    let map = guard.get_or_insert_with(HashMap::new);
    map.insert(
        session_id.to_string(),
        QueuedPendingMemory {
            pending,
            binding: None,
        },
    );
}

#[cfg(test)]
pub(super) fn backdate_injected_memory_for_test(
    session_id: &str,
    id: &str,
    age: std::time::Duration,
) {
    if let Ok(mut guard) = INJECTED_MEMORY_IDS.lock()
        && let Some(outer) = guard.as_mut()
        && let Some(set) = outer.get_mut(session_id)
        && let Some(at) = set.get_mut(id)
    {
        *at = Instant::now() - age;
    }
}

#[cfg(test)]
mod scoped_tests {
    use super::*;
    use crate::memory::{MemoryCategory, MemoryEntry, MemoryManager};
    use crate::memory_graph::MemoryGraph;

    struct HomeGuard {
        previous: Option<std::ffi::OsString>,
        _dir: tempfile::TempDir,
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            clear_all_pending_memory();
            match self.previous.take() {
                Some(home) => crate::env::set_var("JCODE_HOME", home),
                None => crate::env::remove_var("JCODE_HOME"),
            }
        }
    }

    fn fixture(test: impl FnOnce()) {
        let _env = crate::storage::lock_test_env();
        let _pending = super::super::tests::PENDING_MEMORY_TEST_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let home = HomeGuard {
            previous: std::env::var_os("JCODE_HOME"),
            _dir: tempfile::tempdir().expect("isolated memory home"),
        };
        crate::env::set_var("JCODE_HOME", home._dir.path());
        clear_all_pending_memory();
        test();
    }

    fn manager(project: Option<&str>) -> MemoryManager {
        match project {
            Some(project) => MemoryManager::new().with_project_dir(project),
            None => MemoryManager::new(),
        }
    }

    fn fact(content: &str) -> MemoryEntry {
        MemoryEntry::new(MemoryCategory::Fact, content)
    }

    fn save(project: Option<&str>, global: bool, entries: &[MemoryEntry]) {
        let mut graph = MemoryGraph::new();
        for entry in entries {
            graph.add_memory(entry.clone());
        }
        let manager = manager(project);
        if global {
            manager.save_global_graph(&graph).unwrap();
        } else {
            manager.save_project_graph(&graph).unwrap();
        }
    }

    fn publish(session: &str, project: Option<&str>, entries: &[MemoryEntry]) {
        set_pending_memory_for_project(
            session,
            super::super::format_relevant_prompt(entries, entries.len()).unwrap(),
            entries.len(),
            entries.iter().map(|entry| entry.id.clone()).collect(),
            None,
            project,
        );
    }

    #[test]
    fn project_switch_drops_payload_before_injected_bookkeeping() {
        fixture(|| {
            let entry = fact("Project-specific build command");
            save(Some("/project/a"), false, std::slice::from_ref(&entry));
            // Even an identical ID/content in another project cannot authorize
            // a result selected against the previous project's context.
            save(Some("/project/b"), false, std::slice::from_ref(&entry));
            publish("switch", Some("/project/a"), std::slice::from_ref(&entry));
            assert!(take_pending_memory_for_project("switch", Some("/project/b")).is_none());
            assert!(!is_memory_injected("switch", &entry.id));
            assert!(!has_pending_memory("switch"));
            publish("switch", Some("/project/b"), std::slice::from_ref(&entry));
            assert!(take_pending_memory_for_project("switch", Some("/project/b")).is_some());
            assert!(is_memory_injected("switch", &entry.id));
        });
    }

    #[test]
    fn new_project_publication_replaces_same_prompt_from_old_project() {
        fixture(|| {
            let entry = fact("A globally stored preference");
            save(None, true, std::slice::from_ref(&entry));
            publish("replace", Some("/project/a"), std::slice::from_ref(&entry));
            publish("replace", Some("/project/b"), std::slice::from_ref(&entry));
            assert!(take_pending_memory_for_project("replace", Some("/project/b")).is_some());
        });
    }

    #[test]
    fn forgotten_after_selection_drops_whole_payload() {
        fixture(|| {
            let first = fact("First selected fact");
            let second = fact("Second selected fact");
            let entries = [first.clone(), second.clone()];
            save(Some("/project/a"), false, &entries);
            publish("forgotten", Some("/project/a"), &entries);
            assert!(manager(Some("/project/a")).forget(&first.id).unwrap());
            assert!(take_pending_memory_for_project("forgotten", Some("/project/a")).is_none());
            assert!(!is_memory_injected("forgotten", &first.id));
            assert!(!is_memory_injected("forgotten", &second.id));
        });
    }

    #[test]
    fn inactive_or_superseded_memory_is_not_consumed() {
        fixture(|| {
            for superseded in [false, true] {
                let mut entry = fact("Old database port");
                save(None, true, std::slice::from_ref(&entry));
                publish("inactive", None, std::slice::from_ref(&entry));
                if superseded {
                    entry.superseded_by = Some("replacement-id".into());
                } else {
                    entry.active = false;
                }
                save(None, true, std::slice::from_ref(&entry));
                assert!(take_pending_memory_for_project("inactive", None).is_none());
                assert!(!is_memory_injected("inactive", &entry.id));
            }
        });
    }

    #[test]
    fn updated_fact_is_rejected_even_when_disk_mtime_matches_cache() {
        fixture(|| {
            let mut entry = fact("Database port is 5432");
            save(Some("/project/a"), false, std::slice::from_ref(&entry));
            publish("updated", Some("/project/a"), std::slice::from_ref(&entry));
            let path = manager(Some("/project/a"))
                .project_memory_path()
                .unwrap()
                .unwrap();
            let old_mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
            entry.content = "Database port is 6432".into();
            let mut graph = MemoryGraph::new();
            graph.add_memory(entry.clone());
            // Rewrite outside MemoryManager, preserving mtime, so a cached graph
            // would still show the old content. Scoped validation must not use it.
            std::fs::write(&path, serde_json::to_vec(&graph).unwrap()).unwrap();
            std::fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(old_mtime))
                .unwrap();
            assert!(take_pending_memory_for_project("updated", Some("/project/a")).is_none());
            assert!(!is_memory_injected("updated", &entry.id));
            publish("updated", Some("/project/a"), std::slice::from_ref(&entry));
            let fresh = take_pending_memory_for_project("updated", Some("/project/a")).unwrap();
            assert!(fresh.prompt.contains("6432"));
        });
    }

    #[test]
    fn changed_before_publication_cannot_bind_new_signature_to_old_prompt() {
        fixture(|| {
            let old = fact("Database port is 5432");
            let mut current = old.clone();
            current.content = "Database port is 6432".into();
            save(None, true, std::slice::from_ref(&current));
            publish("publish-race", None, std::slice::from_ref(&old));
            assert!(!has_pending_memory("publish-race"));
            assert!(!is_memory_injected("publish-race", &old.id));
            publish("publish-race", None, std::slice::from_ref(&current));
            assert!(take_pending_memory_for_project("publish-race", None).is_some());
        });
    }

    #[test]
    fn tags_only_change_during_selection_is_rejected_before_publication() {
        fixture(|| {
            let mut selected = fact("Use the documented database configuration");
            selected.tags = vec!["postgres".into()];
            let mut current = selected.clone();
            current.tags = vec!["sqlite".into()];
            // Keep content, category and timestamps identical: canonical prompt
            // comparison cannot detect this changed input to the Jev decision.
            let prompt =
                super::super::format_relevant_prompt(std::slice::from_ref(&selected), 1).unwrap();
            assert_eq!(
                Some(prompt.clone()),
                super::super::format_relevant_prompt(std::slice::from_ref(&current), 1)
            );
            save(Some("/project/a"), false, std::slice::from_ref(&current));
            set_pending_memory_for_project_with_selection(
                "tags-race",
                prompt.clone(),
                1,
                std::slice::from_ref(&selected),
                None,
                Some("/project/a"),
            );
            assert!(!has_pending_memory("tags-race"));
            assert!(!is_memory_injected("tags-race", &selected.id));

            // An evaluation performed against the new metadata remains usable.
            set_pending_memory_for_project_with_selection(
                "tags-race",
                prompt,
                1,
                std::slice::from_ref(&current),
                None,
                Some("/project/a"),
            );
            assert!(take_pending_memory_for_project("tags-race", Some("/project/a")).is_some());
        });
    }

    #[test]
    fn global_only_binding_never_becomes_an_implicit_project() {
        fixture(|| {
            let global = fact("Prefer concise messages");
            let local = fact("Local-only database setting");
            save(None, true, std::slice::from_ref(&global));
            save(Some("/project/a"), false, std::slice::from_ref(&local));
            publish("global", None, std::slice::from_ref(&global));
            assert!(take_pending_memory_for_project("global", None).is_some());
            publish("local-without-scope", None, std::slice::from_ref(&local));
            assert!(!has_pending_memory("local-without-scope"));
            publish("global-switched", None, std::slice::from_ref(&global));
            assert!(
                take_pending_memory_for_project("global-switched", Some("/project/a")).is_none()
            );
            assert!(!is_memory_injected("global-switched", &global.id));
        });
    }

    #[test]
    fn distinct_sessions_keep_independent_pending_and_injected_state() {
        fixture(|| {
            let entry = fact("Shared project fact");
            save(Some("/project/a"), false, std::slice::from_ref(&entry));
            for sid in ["session-a", "session-b"] {
                publish(sid, Some("/project/a"), std::slice::from_ref(&entry));
            }
            assert!(take_pending_memory_for_project("session-a", Some("/project/a")).is_some());
            assert!(has_pending_memory("session-b"));
            assert!(!is_memory_injected("session-b", &entry.id));
            assert!(take_pending_memory_for_project("session-b", Some("/project/a")).is_some());
        });
    }

    #[test]
    fn graph_load_error_fails_closed_even_for_selected_global_memory() {
        fixture(|| {
            let entry = fact("Globally stored selected fact");
            save(None, true, std::slice::from_ref(&entry));
            save(Some("/project/a"), false, &[]);
            publish("corrupt", Some("/project/a"), std::slice::from_ref(&entry));
            let path = manager(Some("/project/a"))
                .project_memory_path()
                .unwrap()
                .unwrap();
            std::fs::write(path, b"not a memory graph").unwrap();
            assert!(take_pending_memory_for_project("corrupt", Some("/project/a")).is_none());
            assert!(!is_memory_injected("corrupt", &entry.id));
            publish(
                "corrupt-publish",
                Some("/project/a"),
                std::slice::from_ref(&entry),
            );
            assert!(!has_pending_memory("corrupt-publish"));
        });
    }

    #[test]
    fn scoped_take_rejects_legacy_payload_but_legacy_api_still_works() {
        fixture(|| {
            set_pending_memory("legacy-scoped", "legacy prompt".into(), 1);
            assert!(take_pending_memory_for_project("legacy-scoped", None).is_none());
            set_pending_memory("legacy", "legacy prompt".into(), 1);
            assert_eq!(
                take_pending_memory("legacy").unwrap().prompt,
                "legacy prompt"
            );
        });
    }

    #[test]
    fn same_id_in_project_and_global_is_ambiguous_and_rejected() {
        fixture(|| {
            let entry = fact("Ambiguous stored fact");
            save(Some("/project/a"), false, std::slice::from_ref(&entry));
            save(None, true, std::slice::from_ref(&entry));
            publish(
                "ambiguous",
                Some("/project/a"),
                std::slice::from_ref(&entry),
            );
            assert!(!has_pending_memory("ambiguous"));
            assert!(!is_memory_injected("ambiguous", &entry.id));
        });
    }

    #[test]
    fn publication_and_validation_preserve_selected_id_order() {
        fixture(|| {
            let first = fact("First selected fact");
            let second = fact("Second selected fact");
            let entries = [second.clone(), first.clone()];
            save(None, true, &entries);
            publish("ordered", None, &entries);
            let pending = take_pending_memory_for_project("ordered", None).unwrap();
            assert_eq!(pending.memory_ids, [second.id, first.id]);
            assert!(
                pending.prompt.find("Second selected").unwrap()
                    < pending.prompt.find("First selected").unwrap()
            );
        });
    }
}
