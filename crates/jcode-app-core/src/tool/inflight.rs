//! Process-global registry of tool calls that are currently executing.
//!
//! Why this exists: the "missing tool output" repair paths treat an assistant
//! `tool_use` with no matching `tool_result` as evidence of an interrupted
//! turn and insert a synthetic placeholder result. That inference is wrong
//! while the tool is *still running*. A confirmed production wedge
//! (session_clover_1785560899476): a 106s `bash` call was mid-flight when a
//! scheduled-task wakeup drove another turn on the same session. Repair
//! injected a placeholder result, the real output landed 28s later as a second
//! `tool_result` for the same `tool_use_id`, and Anthropic then rejected every
//! subsequent request with:
//!
//! ```text
//! unexpected `tool_use_id` found in `tool_result` blocks: <id>
//! ```
//!
//! leaving the session permanently unsendable. Repair must therefore skip any
//! tool call that is still executing; its real result is on the way.
//!
//! `Registry::execute` registers each call here for the duration of its
//! execution via an RAII guard, so entries can never leak past a panic,
//! cancellation, or early return.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

/// Reference counts per tool_call_id. A count (rather than a set) keeps the
/// registry correct if the same id is somehow executed concurrently, e.g. a
/// retry racing the original.
static IN_FLIGHT: LazyLock<Mutex<HashMap<String, usize>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// RAII registration for one executing tool call.
pub struct InFlightToolGuard {
    tool_call_id: String,
}

impl Drop for InFlightToolGuard {
    fn drop(&mut self) {
        let Ok(mut map) = IN_FLIGHT.lock() else {
            return;
        };
        if let Some(count) = map.get_mut(&self.tool_call_id) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                map.remove(&self.tool_call_id);
            }
        }
    }
}

/// Mark `tool_call_id` as executing until the returned guard is dropped.
/// Empty ids are not tracked (nothing can match them during repair).
pub fn mark_tool_in_flight(tool_call_id: &str) -> Option<InFlightToolGuard> {
    if tool_call_id.is_empty() {
        return None;
    }
    let mut map = IN_FLIGHT.lock().ok()?;
    *map.entry(tool_call_id.to_string()).or_insert(0) += 1;
    Some(InFlightToolGuard {
        tool_call_id: tool_call_id.to_string(),
    })
}

/// True while a tool call with this id is executing somewhere in this process.
pub fn is_tool_in_flight(tool_call_id: &str) -> bool {
    IN_FLIGHT
        .lock()
        .map(|map| map.contains_key(tool_call_id))
        .unwrap_or(false)
}

/// Number of tool calls currently executing (diagnostics only).
pub fn in_flight_tool_count() -> usize {
    IN_FLIGHT.lock().map(|map| map.len()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_tracks_and_releases() {
        let id = "toolu_inflight_test_basic";
        assert!(!is_tool_in_flight(id));
        {
            let _guard = mark_tool_in_flight(id).expect("guard");
            assert!(is_tool_in_flight(id));
        }
        assert!(!is_tool_in_flight(id));
    }

    #[test]
    fn nested_guards_release_only_after_the_last_one() {
        let id = "toolu_inflight_test_nested";
        let outer = mark_tool_in_flight(id).expect("guard");
        let inner = mark_tool_in_flight(id).expect("guard");
        assert!(is_tool_in_flight(id));
        drop(inner);
        assert!(is_tool_in_flight(id), "one registration still outstanding");
        drop(outer);
        assert!(!is_tool_in_flight(id));
    }

    #[test]
    fn empty_ids_are_not_tracked() {
        assert!(mark_tool_in_flight("").is_none());
        assert!(!is_tool_in_flight(""));
    }
}
