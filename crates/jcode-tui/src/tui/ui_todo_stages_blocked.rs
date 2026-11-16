//! Blocked phase status spans.
use super::*;

pub(crate) fn blocked_phase_spans(
    todos: &[&crate::todo::TodoItem],
    agents: &[TodoCardAgent],
) -> Vec<Span<'static>> {
    if !crate::todo::stages::has_stages(todos.iter().copied()) {
        return Vec::new();
    }
    let stages = ordered_layout(todos);
    let Some(current) = current_stage(&stages, todos) else {
        return Vec::new();
    };
    let stage = &stages[current];
    if stage
        .item_indices()
        .any(|i| crate::todo::todo_completely_blocked(todos[i]))
    {
        // The phase pip already replaces this todo's dot with ✋.
        // Repeating a blocked phase label adds no information.
        return Vec::new();
    }
    let failed = stage.item_indices().any(|i| {
        !is_done(todos[i])
            && todos[i].assigned_to.as_deref().is_some_and(|owner| {
                agents.iter().any(|agent| {
                    agent.matches(owner) && matches!(agent.status.as_str(), "failed" | "crashed")
                })
            })
    });
    if !failed {
        return Vec::new();
    }
    let label = match stage.number {
        Some(n) => format!("  ⏸ phase {n} blocked"),
        None => "  ⏸ blocked".to_string(),
    };
    vec![Span::styled(
        label,
        Style::default().fg(todo_failure_color()),
    )]
}
