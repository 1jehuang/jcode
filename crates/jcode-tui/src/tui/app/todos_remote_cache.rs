//! Per-client decoded todo snapshots. Unchanged refreshes share parsed data,
//! while raw payload equality preserves invalidation without relying on hashes.
use std::{cell::RefCell, sync::Arc};

type TodoState = (
    Vec<crate::todo::TodoItem>,
    crate::todo::TodoPlan,
    Vec<crate::todo::TodoGoal>,
);

pub(crate) struct RemoteTodoCache {
    raw: String,
    state: Arc<TodoState>,
}

pub(super) fn load(cache: &RefCell<Option<RemoteTodoCache>>, raw: Option<&str>) -> Arc<TodoState> {
    if let Some(raw) = raw
        && let Some(cached) = cache.borrow().as_ref()
        && cached.raw == raw
    {
        return Arc::clone(&cached.state);
    }
    let parsed = raw.map(serde_json::from_str::<serde_json::Value>);
    let valid = parsed.as_ref().is_some_and(Result::is_ok);
    let payload = match parsed {
        Some(Ok(payload)) => payload,
        Some(Err(error)) => {
            crate::logging::debug(&format!("Invalid remote todo snapshot: {error}"));
            serde_json::Value::Null
        }
        // No remote snapshot yet is an ordinary empty state, not a parse error.
        None => serde_json::Value::Null,
    };
    let state = Arc::new((
        serde_json::from_value(payload["todos"].clone()).unwrap_or_else(|error| {
            crate::logging::debug(&format!("Invalid remote todos: {error}"));
            Vec::new()
        }),
        serde_json::from_value(payload["plan"].clone()).unwrap_or_else(|error| {
            crate::logging::debug(&format!("Invalid remote todo plan: {error}"));
            crate::todo::TodoPlan::default()
        }),
        serde_json::from_value(payload["goals"].clone()).unwrap_or_else(|error| {
            crate::logging::debug(&format!("Invalid remote todo goals: {error}"));
            Vec::new()
        }),
    ));
    // One snapshot per client, capped so an unusually large history does not
    // become a second retained transcript. Invalid JSON keeps existing logging.
    *cache.borrow_mut() = raw
        .filter(|raw| valid && raw.len() <= 256 * 1024)
        .map(|raw| RemoteTodoCache {
            raw: raw.to_owned(),
            state: Arc::clone(&state),
        });
    state
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn payload(intent: &str) -> String {
        json!({"todos":[{"id":"t1", "content":"keep task details", "status":"pending", "priority":"high"}], "plan":{"user_intention":intent, "understands_user_intent":"clear"}, "goals":[]}).to_string()
    }

    #[test]
    fn unchanged_snapshots_share_decoding_and_equal_length_edits_invalidate() {
        let cache = RefCell::new(None);
        let first = load(&cache, Some(&payload("first")));
        assert_eq!(first.0.len(), 1);
        assert_eq!(first.0[0].content, "keep task details");
        let unchanged = load(&cache, Some(&payload("first")));
        assert!(Arc::ptr_eq(&first, &unchanged));
        let changed = load(&cache, Some(&payload("other")));
        assert!(!Arc::ptr_eq(&first, &changed));
        assert_eq!(changed.1.user_intention.as_deref(), Some("other"));
        assert_eq!(first.1.user_intention.as_deref(), Some("first"));
        let missing = load(&cache, None);
        assert!(missing.0.is_empty());
        assert!(cache.borrow().is_none());
    }

    #[test]
    fn malformed_fields_still_default_independently_and_cache_is_bounded() {
        let cache = RefCell::new(None);
        let raw = json!({"todos":false, "goals":"bad", "plan":{"user_intention":"keep valid plan", "understands_user_intent":"clear"}}).to_string();
        let state = load(&cache, Some(&raw));
        assert!(state.0.is_empty());
        assert!(state.2.is_empty());
        assert_eq!(state.1.user_intention.as_deref(), Some("keep valid plan"));
        let invalid = load(&cache, Some("{"));
        assert!(invalid.0.is_empty());
        assert!(cache.borrow().is_none());
        let _ = load(&cache, Some(&payload(&"x".repeat(256 * 1024))));
        assert!(cache.borrow().is_none());
    }

    #[test]
    #[ignore = "comparative unchanged remote todo decoding benchmark"]
    fn benchmark_unchanged_remote_todo_decoding() {
        use std::{hint::black_box, time::Instant};
        let raw = payload(&"representative todo plan details ".repeat(1024));
        let cache = RefCell::new(None);
        let _ = load(&cache, Some(&raw));
        let start = Instant::now();
        for _ in 0..100 {
            black_box(load(&RefCell::new(None), black_box(Some(raw.as_str()))));
        }
        let uncached = start.elapsed();
        let start = Instant::now();
        for _ in 0..100 {
            black_box(load(&cache, black_box(Some(raw.as_str()))));
        }
        let cached = start.elapsed();
        eprintln!(
            "unchanged remote todo decoding: uncached={uncached:?} cached={cached:?} ratio={:.2}x",
            uncached.as_secs_f64() / cached.as_secs_f64()
        );
    }
}
