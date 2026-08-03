//! Structured presentation of the model's `todo` tool calls.
//!
//! The tool input is the authoritative snapshot.  Turning it into markdown here
//! keeps selection, wrapping, copying, and accessibility on the transcript's
//! normal text path while the scene adds the card and progress bar.

use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TodoCard {
    pub source: String,
    pub permille: u16,
    pub states: Vec<TodoState>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TodoState {
    Pending,
    Active,
    Completed,
}

pub fn parse(input: Option<&str>) -> Option<TodoCard> {
    let root: Value = serde_json::from_str(input?).ok()?;
    let todos = root.get("todos")?.as_array()?;
    if todos.is_empty() {
        return None;
    }

    let completed = todos
        .iter()
        .filter(|todo| todo.get("status").and_then(Value::as_str) == Some("completed"))
        .count();
    let total = todos.len();
    let permille = ((completed * 1000) / total) as u16;
    let noun = if total == 1 { "task" } else { "tasks" };
    // One header line: the name, then the count in words. The percentage is
    // deliberately absent, because the scene draws it as a bar on this same
    // line, and a number restating the bar is the bar said twice.
    let mut source = format!("**Plan**  ·  {completed} of {total} {noun}");
    let mut states = Vec::with_capacity(total);

    let mut last_group: Option<&str> = None;
    let mut wrote_group = false;
    for todo in todos {
        let content = todo.get("content").and_then(Value::as_str)?.trim();
        if content.is_empty() {
            continue;
        }
        let group = todo
            .get("group")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|group| !group.is_empty());
        if !wrote_group || group != last_group {
            source.push_str("\n\n");
            source.push_str("**");
            source.push_str(group.unwrap_or("Tasks"));
            source.push_str("**");
            last_group = group;
            wrote_group = true;
        }
        let state = match todo.get("status").and_then(Value::as_str) {
            Some("completed") => TodoState::Completed,
            Some("in_progress") => TodoState::Active,
            _ => TodoState::Pending,
        };
        states.push(state);
        // State is written into the markdown, not only kept beside it: a
        // finished task is struck through and the active one is bold. This is
        // what the reader expects a checklist to look like, and it keeps the
        // source (the layout cache's key) different whenever a state changes,
        // so a task starting or finishing can never be drawn from a stale
        // layout.
        match state {
            TodoState::Completed => source.push_str(&format!("\n- ~~{content}~~")),
            TodoState::Active => source.push_str(&format!("\n- **{content}**")),
            TodoState::Pending => source.push_str(&format!("\n- {content}")),
        }
    }

    Some(TodoCard {
        source,
        permille,
        states,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_a_grouped_ascii_checklist() {
        let card = parse(Some(
            r#"{"todos":[{"content":"Trace events","status":"completed","group":"UI"},{"content":"Draw card","status":"in_progress","group":"UI"},{"content":"Test it","status":"pending","group":"Verification"}]}"#,
        ))
        .unwrap();
        assert_eq!(card.permille, 333);
        assert!(card.source.contains("**Plan**  ·  1 of 3 tasks"));
        assert!(
            card.source
                .contains("**UI**\n- ~~Trace events~~\n- **Draw card**")
        );
        assert!(card.source.contains("**Verification**\n- Test it"));
        assert_eq!(
            card.states,
            [TodoState::Completed, TodoState::Active, TodoState::Pending]
        );
        assert!(!card.source.contains('\u{2610}'));
        assert!(!card.source.contains('\u{2611}'));
    }

    #[test]
    fn labels_ungrouped_tasks() {
        let card = parse(Some(
            r#"{"todos":[{"content":"One thing","status":"pending","group":null}]}"#,
        ))
        .unwrap();
        assert!(card.source.contains("0 of 1 task"));
        assert!(card.source.contains("**Tasks**\n- One thing"));
    }
}
