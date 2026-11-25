use super::*;

// Cache data only: time-sensitive blocker policy and width/style rendering
// remain live on every draw. One bounded entry retains no transcript history.
const MAX_CACHED_TODO_PAYLOAD_BYTES: usize = 256 * 1024;

#[derive(Default)]
struct TodoPayloadCache {
    entry: Option<(String, Option<std::sync::Arc<TodoCardPayload>>)>,
}

impl TodoPayloadCache {
    fn get(&mut self, content: &str) -> Option<std::sync::Arc<TodoCardPayload>> {
        if let Some((source, parsed)) = &self.entry
            && source == content
        {
            return parsed.clone();
        }
        let parsed = TodoCardPayload::parse(content).map(std::sync::Arc::new);
        self.entry = (content.len() <= MAX_CACHED_TODO_PAYLOAD_BYTES)
            .then(|| (content.to_owned(), parsed.clone()));
        parsed
    }
}

thread_local! {
    static TODO_PAYLOAD_CACHE: std::cell::RefCell<TodoPayloadCache> =
        std::cell::RefCell::new(TodoPayloadCache::default());
}

pub(super) fn cached_todo_payload(content: &str) -> Option<std::sync::Arc<TodoCardPayload>> {
    TODO_PAYLOAD_CACHE.with_borrow_mut(|cache| cache.get(content))
}

#[cfg(test)]
#[path = "todo_payload_cache_tests.rs"]
mod todo_payload_cache_tests;
