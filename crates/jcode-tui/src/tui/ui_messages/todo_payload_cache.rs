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
        let parsed = serde_json::from_str::<TodoCardPayload>(content)
            .ok()
            .map(std::sync::Arc::new);
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

pub(super) fn todo_blocker_footer_at(
    payload: Option<&str>,
    width: u16,
    now: chrono::DateTime<chrono::Utc>,
) -> Vec<Line<'static>> {
    let Some(payload) = payload.and_then(cached_todo_payload) else {
        return Vec::new();
    };
    render_todo_blocker_footer(&payload, width, now)
}

fn render_todo_blocker_footer(
    payload: &TodoCardPayload,
    width: u16,
    now: chrono::DateTime<chrono::Utc>,
) -> Vec<Line<'static>> {
    payload
        .todos()
        .iter()
        .filter(|todo| crate::todo::is_incomplete_todo(todo))
        .filter_map(|todo| {
            let reason = if crate::todo::todo_completely_blocked(todo) {
                todo.completely_blocked.as_ref().map(|b| b.reason.clone())
            } else if crate::todo::todo_has_external_blocker_at(todo, now) {
                todo.external_blocker.as_ref().map(|b| b.reason.clone())
            } else if !todo.blocked_by.is_empty() {
                Some(format!("waiting on {}", todo.blocked_by.join(", ")))
            } else {
                None
            }?;
            let text = format!("Blocked · {}: {}", todo.content, reason)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            Some(super::truncate_line_with_ellipsis_to_width(
                &Line::styled(text, Style::default().fg(todo_warning_color())),
                usize::from(width),
            ))
        })
        .collect()
}

#[cfg(test)]
#[path = "todo_payload_cache_tests.rs"]
mod tests;
