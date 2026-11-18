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

pub(super) fn render_todo_blocker_footer(
    payload: &TodoCardPayload,
    width: u16,
) -> Vec<Line<'static>> {
    let row = |text: String| {
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        super::truncate_line_with_ellipsis_to_width(
            &Line::styled(text, Style::default().fg(todo_warning_color())),
            usize::from(width),
        )
    };
    let todos = payload.todos();
    let goals = match payload {
        TodoCardPayload::Current { goals, .. } => goals.as_slice(),
        TodoCardPayload::Legacy(_) => &[],
    };
    let mut lines: Vec<Line<'static>> = todos
        .iter()
        .filter(|todo| crate::todo::is_incomplete_todo(todo))
        .filter_map(|todo| {
            if crate::todo::todo_requires_user_auth(todo) {
                let reason = &todo.requires_user_auth.as_ref()?.reason;
                Some(row(format!("✋ Needs you · {}: {}", todo.content, reason)))
            } else if !todo.blocked_by.is_empty() {
                Some(row(format!(
                    "Blocked · {}: waiting on {}",
                    todo.content,
                    todo.blocked_by.join(", ")
                )))
            } else {
                None
            }
        })
        .collect();
    // A goal declared impossible stops its todos, so say so once per goal
    // while any of that goal's work is still open.
    for goal in goals {
        let Some(reason) = crate::todo::goal_cannot_complete(goal) else {
            continue;
        };
        let open = todos.iter().any(|todo| {
            crate::todo::is_incomplete_todo(todo)
                && crate::todo::todo_goal_cannot_complete(todo, std::slice::from_ref(goal))
        });
        if open {
            let label = goal
                .group
                .as_deref()
                .map(str::trim)
                .filter(|group| !group.is_empty())
                .unwrap_or("goal");
            lines.push(row(format!("Cannot complete · {label}: {reason}")));
        }
    }
    lines
}

#[cfg(test)]
#[path = "todo_payload_cache_tests.rs"]
mod tests;
