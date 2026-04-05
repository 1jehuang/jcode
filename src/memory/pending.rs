use super::*;

/// Pending memory prompt from background check - ready to inject on next turn.
/// Keyed by session ID so each session gets its own pending memory.
static PENDING_MEMORY: Mutex<Option<HashMap<String, PendingMemory>>> = Mutex::new(None);

/// Signature of the last injected prompt to suppress near-immediate duplicates.
/// Keyed by session ID.
static LAST_INJECTED_PROMPT_SIGNATURE: Mutex<Option<HashMap<String, (String, Instant)>>> =
    Mutex::new(None);

/// Memory IDs that have already been injected into the conversation.
/// Used to prevent the same memory from being re-injected on subsequent turns.
/// Keyed by session ID.
static INJECTED_MEMORY_IDS: Mutex<Option<HashMap<String, HashSet<String>>>> = Mutex::new(None);

/// Guard to ensure only one memory check runs at a time, per session.
/// Keyed by session ID.
static MEMORY_CHECK_IN_PROGRESS: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// Suppress repeated identical memory payloads within this many seconds.
const MEMORY_REPEAT_SUPPRESSION_SECS: u64 = 90;

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

/// Take pending memory if available and fresh for the given session.
pub fn take_pending_memory(session_id: &str) -> Option<PendingMemory> {
    if let Ok(mut guard) = PENDING_MEMORY.lock() {
        let map = guard.get_or_insert_with(HashMap::new);
        if let Some(pending) = map.remove(session_id) {
            if !pending.is_fresh() {
                crate::memory_log::log_pending_discarded(session_id, "stale (>120s)");
                return None;
            }

            let sig = prompt_signature(&pending.prompt);
            if let Ok(mut last_guard) = LAST_INJECTED_PROMPT_SIGNATURE.lock() {
                let sig_map = last_guard.get_or_insert_with(HashMap::new);
                if let Some((last_sig, last_at)) = sig_map.get(session_id) {
                    if *last_sig == sig
                        && last_at.elapsed().as_secs() < MEMORY_REPEAT_SUPPRESSION_SECS
                    {
                        crate::memory_log::log_pending_discarded(
                            session_id,
                            "duplicate suppressed",
                        );
                        return None;
                    }
                }
                sig_map.insert(session_id.to_string(), (sig, Instant::now()));
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

            return Some(pending);
        }
    }
    None
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
    crate::memory_log::log_pending_prepared(session_id, &prompt, count, &memory_ids);

    if let Ok(mut guard) = PENDING_MEMORY.lock() {
        let map = guard.get_or_insert_with(HashMap::new);
        map.insert(
            session_id.to_string(),
            PendingMemory {
                prompt,
                display_prompt,
                computed_at: Instant::now(),
                count,
                memory_ids,
            },
        );
    }
}

/// Mark memory IDs as already injected for a session (prevents re-injection on future turns).
pub fn mark_memories_injected(session_id: &str, ids: &[String]) {
    crate::memory_log::log_marked_injected(session_id, ids);

    if let Ok(mut guard) = INJECTED_MEMORY_IDS.lock() {
        let outer = guard.get_or_insert_with(HashMap::new);
        let set = outer
            .entry(session_id.to_string())
            .or_insert_with(HashSet::new);
        for id in ids {
            set.insert(id.clone());
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

        outer.insert(
            session_id.to_string(),
            ids.iter().cloned().collect::<HashSet<_>>(),
        );
    }
}

/// Check if a memory ID has already been injected for a session.
pub fn is_memory_injected(session_id: &str, id: &str) -> bool {
    if let Ok(guard) = INJECTED_MEMORY_IDS.lock() {
        if let Some(outer) = guard.as_ref() {
            if let Some(set) = outer.get(session_id) {
                return set.contains(id);
            }
        }
    }
    false
}

/// Check if a memory ID has already been injected in ANY session.
/// Used by the singleton memory agent which doesn't track per-session state.
pub fn is_memory_injected_any(id: &str) -> bool {
    if let Ok(guard) = INJECTED_MEMORY_IDS.lock() {
        if let Some(outer) = guard.as_ref() {
            return outer.values().any(|set| set.contains(id));
        }
    }
    false
}

/// Clear injected memory tracking for a session (call on session reset or topic change).
pub fn clear_injected_memories(session_id: &str) {
    if let Ok(mut guard) = INJECTED_MEMORY_IDS.lock() {
        if let Some(outer) = guard.as_mut() {
            if let Some(set) = outer.remove(session_id) {
                if !set.is_empty() {
                    crate::logging::info(&format!(
                        "[{}] Clearing {} tracked injected memory IDs",
                        session_id,
                        set.len()
                    ));
                }
            }
        }
    }
}

/// Clear all injected memory tracking across all sessions.
pub fn clear_all_injected_memories() {
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
    if let Ok(mut guard) = PENDING_MEMORY.lock() {
        if let Some(map) = guard.as_mut() {
            map.remove(session_id);
        }
    }
    if let Ok(mut guard) = LAST_INJECTED_PROMPT_SIGNATURE.lock() {
        if let Some(map) = guard.as_mut() {
            map.remove(session_id);
        }
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
        set.insert(session_id.to_string())
    } else {
        false
    }
}

pub(super) fn finish_memory_check(session_id: &str) {
    if let Ok(mut guard) = MEMORY_CHECK_IN_PROGRESS.lock() {
        if let Some(set) = guard.as_mut() {
            set.remove(session_id);
        }
    }
}

#[cfg(test)]
pub(super) fn insert_pending_memory_for_test(session_id: &str, pending: PendingMemory) {
    let mut guard = PENDING_MEMORY.lock().expect("pending memory lock");
    let map = guard.get_or_insert_with(HashMap::new);
    map.insert(session_id.to_string(), pending);
}
