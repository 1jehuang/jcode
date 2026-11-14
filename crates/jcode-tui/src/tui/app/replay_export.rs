//! Offline exports use the same state transitions and frame renderer as the TUI.
use super::App;
use anyhow::{Context, Result, ensure};
use ratatui::{
    Terminal,
    backend::{Backend, CrosstermBackend, TestBackend},
    buffer::Buffer,
};
use serde_json::{Value, json};

impl App {
    pub(super) fn restore_replay_todos_json(&mut self, output: &str) {
        if let Some(payload) = todo_snapshot_from_output(output) {
            self.pinned_todos_payload = Some(payload);
        }
    }

    /// Timestamp-preserving convenience API for CLI exporters.
    pub fn render_replay_states(
        self,
        timeline: &[crate::replay::TimelineEvent],
        times_ms: &[u64],
        width: u16,
        height: u16,
        todo_snapshots: &[(u64, Value)],
    ) -> Result<Vec<(u64, Buffer)>> {
        let frames =
            self.render_replay_frames(timeline, times_ms, todo_snapshots, width, height)?;
        Ok(times_ms.iter().copied().zip(frames).collect())
    }

    /// Render selected historical timeline positions, in milliseconds, without
    /// opening a terminal, contacting a provider, or reading current todo files.
    /// `todo_snapshots` contains timestamped JSON arrays from the todo store or
    /// complete `{todos, plan, goals}` card payloads. Snapshots override recorded
    /// tool results at the same timestamp. Positions must be nondecreasing.
    pub fn render_replay_frames(
        mut self,
        timeline: &[crate::replay::TimelineEvent],
        positions_ms: &[u64],
        todo_snapshots: &[(u64, Value)],
        width: u16,
        height: u16,
    ) -> Result<Vec<Buffer>> {
        ensure!(width > 0 && height > 0, "frame dimensions must be positive");
        ensure!(
            self.is_replay,
            "historical rendering requires App::new_for_replay"
        );
        ensure!(
            positions_ms.windows(2).all(|w| w[0] <= w[1]),
            "frame positions must be ordered"
        );
        ensure!(
            todo_snapshots.windows(2).all(|w| w[0].0 <= w[1].0),
            "todo snapshots must be ordered"
        );
        let snapshots = todo_snapshots
            .iter()
            .map(|(t, value)| Ok((*t, normalize_todo_snapshot(value.clone())?)))
            .collect::<Result<Vec<_>>>()?;
        let mut schedule = Vec::new();
        let mut time = 0u64;
        for (delay, event) in crate::replay::timeline_to_replay_events(timeline) {
            time = time.saturating_add(delay);
            schedule.push((time, event));
        }
        let mut terminal = Terminal::new(TestBackend::new(width, height))?;
        let mut remote = crate::tui::backend::ReplayRemoteState::default();
        let (mut cursor, mut snapshot_cursor, mut turn) = (0, 0, 0);
        self.pinned_todos_payload = None;
        let mut frames = Vec::with_capacity(positions_ms.len());
        for &position in positions_ms {
            // Merge both histories chronologically, with explicit snapshots last
            // on ties. Do not apply an old override after a newer tool result.
            loop {
                let event_at = schedule.get(cursor).map(|e| e.0);
                let snapshot_at = snapshots.get(snapshot_cursor).map(|s| s.0);
                if event_at.is_some_and(|t| t <= position && snapshot_at.is_none_or(|s| t <= s)) {
                    let (at, event) = &schedule[cursor];
                    super::replay::apply_replay_event(
                        &mut self,
                        &mut remote,
                        event,
                        &mut turn,
                        Some(*at as f64),
                    );
                    cursor += 1;
                } else if snapshot_at.is_some_and(|t| t <= position) {
                    self.pinned_todos_payload = Some(snapshots[snapshot_cursor].1.clone());
                    snapshot_cursor += 1;
                } else {
                    break;
                }
            }
            super::replay::update_replay_elapsed_override(&mut self, position as f64);
            let frame = terminal.draw(|f| crate::tui::render_frame(f, &self))?;
            frames.push(frame.buffer.clone());
        }
        Ok(frames)
    }

    /// Encode actual terminal cells and styles, not SVG/font approximations.
    /// Display these bytes in the capture terminal at the frame's dimensions.
    pub fn replay_frame_ansi(buffer: &Buffer) -> Result<Vec<u8>> {
        ensure!(
            buffer.area.width > 0 && buffer.area.height > 0,
            "frame dimensions must be positive"
        );
        let mut bytes = b"\x1b[0m\x1b[2J\x1b[H".to_vec();
        let mut backend = CrosstermBackend::new(&mut bytes);
        // Force every visible cell into the diff, including default blanks,
        // retaining ratatui's wide-glyph continuation and image skip handling.
        // The sentinel is only in the previous buffer, never on the wire.
        let mut previous = Buffer::empty(buffer.area);
        for cell in &mut previous.content {
            cell.set_symbol("\0");
        }
        backend.draw(previous.diff(buffer).into_iter())?;
        backend.flush()?;
        bytes.extend_from_slice(b"\x1b[0m\x1b[H\x1b[?25l");
        Ok(bytes)
    }
}

pub(super) fn normalize_todo_snapshot(value: Value) -> Result<String> {
    let mut payload = if value.is_array() {
        json!({"todos": value})
    } else {
        value
    };
    let todos = payload
        .get_mut("todos")
        .and_then(Value::as_array_mut)
        .context("todo snapshot requires a todos array")?;
    let named_ids: std::collections::HashMap<String, Value> = todos
        .iter()
        .enumerate()
        .filter_map(|(index, todo)| {
            let name = todo.get("name").or_else(|| todo.get("content"))?.as_str()?;
            Some((
                name.to_owned(),
                todo.get("id")
                    .cloned()
                    .unwrap_or_else(|| json!(format!("replay_{index}"))),
            ))
        })
        .collect();
    for (index, todo) in todos.iter_mut().enumerate() {
        let fields = todo
            .as_object_mut()
            .context("todo item must be an object")?;
        // New tool output uses public task names. Preserve all other recorded
        // fields and use stable local IDs only where the public output omits one.
        if let Some(name) = fields.remove("name") {
            fields.insert("content".into(), name);
        }
        if let Some(description) = fields.remove("description") {
            fields.insert("details".into(), description);
        }
        if let Some(phase) = fields.remove("phase") {
            fields.insert("stage".into(), phase);
        }
        if let Some(mut dependencies) = fields.remove("lane_dependencies") {
            if let Some(dependencies) = dependencies.as_array_mut() {
                for dependency in dependencies {
                    if let Some(id) = dependency.as_str().and_then(|name| named_ids.get(name)) {
                        *dependency = id.clone();
                    }
                }
            }
            fields.insert("lane_after".into(), dependencies);
        }
        fields
            .entry("id")
            .or_insert_with(|| json!(format!("replay_{index}")));
        serde_json::from_value::<crate::todo::TodoItem>(todo.clone())
            .context("invalid todo item")?;
    }
    let obj = payload
        .as_object_mut()
        .context("todo snapshot must be an object")?;
    obj.entry("plan")
        .or_insert_with(|| json!(crate::todo::TodoPlan::default()));
    obj.entry("goals").or_insert_with(|| json!([]));
    let _: crate::todo::TodoPlan =
        serde_json::from_value(payload["plan"].clone()).context("invalid todo plan")?;
    let _: Vec<crate::todo::TodoGoal> =
        serde_json::from_value(payload["goals"].clone()).context("invalid todo goals")?;
    Ok(serde_json::to_string(&payload)?)
}

/// Todo tools historically returned a JSON array followed by labeled JSON
/// sections. Decode those sections without interpreting model-generated prose.
pub(super) fn todo_snapshot_from_output(output: &str) -> Option<String> {
    let output = crate::tui::ui::strip_todo_tool_output_headers(output);
    let mut stream = serde_json::Deserializer::from_str(output.trim_start()).into_iter::<Value>();
    let first = stream.next()?.ok()?;
    let mut payload = if first.is_array() {
        json!({"todos": first})
    } else {
        first
    };
    for (label, field) in [("\n\nPlan:\n", "plan"), ("\n\nGoals:\n", "goals")] {
        if let Some((_, rest)) = output.split_once(label) {
            let value = serde_json::Deserializer::from_str(rest)
                .into_iter::<Value>()
                .next()?
                .ok()?;
            payload.as_object_mut()?.insert(field.into(), value);
        }
    }
    normalize_todo_snapshot(payload).ok()
}

#[cfg(test)]
#[path = "replay_export_tests.rs"]
mod historical_export_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replay::{TimelineEvent, TimelineEventKind};

    fn task(label: &str) -> Value {
        json!({"id":"one", "content":label, "status":"in_progress", "priority":"high"})
    }

    fn app() -> App {
        App::new_for_replay_silent(crate::session::Session::create_with_id(
            "offline_replay_export_test".into(),
            None,
            None,
        ))
    }

    #[test]
    fn snapshots_validate_arrays_full_cards_and_public_tool_output() {
        let array = json!([task("Historical task")]);
        let payload = normalize_todo_snapshot(array).unwrap();
        let full: Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(normalize_todo_snapshot(full).unwrap(), payload);
        let public = "[{\"name\":\"Recorded task\",\"status\":\"pending\",\"priority\":\"high\"}]\n\nPlan:\n{}\n\nGoals:\n[]\n\nContinuation text";
        let decoded: Value =
            serde_json::from_str(&todo_snapshot_from_output(public).unwrap()).unwrap();
        assert_eq!(decoded["todos"][0]["content"], "Recorded task");
        assert!(normalize_todo_snapshot(json!({"todos":[{}]})).is_err());
        assert!(normalize_todo_snapshot(json!({"unrelated":[]})).is_err());
        assert!(normalize_todo_snapshot(json!({"todos":[],"plan":42})).is_err());
        assert!(todo_snapshot_from_output("Error: not a snapshot").is_none());
    }

    #[tokio::test]
    async fn selected_frame_is_exact_production_draw_and_repeated_times_match() {
        let timeline = vec![TimelineEvent {
            t: 0,
            kind: TimelineEventKind::UserMessage {
                text: "Saved historical user message".into(),
            },
        }];
        let frames = app()
            .render_replay_frames(&timeline, &[0, 0], &[], 100, 30)
            .unwrap();
        assert_eq!(frames[0], frames[1]);
        let mut expected = app();
        let mut remote = crate::tui::backend::ReplayRemoteState::default();
        let mut turn = 0;
        for (_, event) in crate::replay::timeline_to_replay_events(&timeline) {
            super::super::replay::apply_replay_event(
                &mut expected,
                &mut remote,
                &event,
                &mut turn,
                Some(0.0),
            );
        }
        super::super::replay::update_replay_elapsed_override(&mut expected, 0.0);
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let direct = terminal
            .draw(|frame| crate::tui::render_frame(frame, &expected))
            .unwrap()
            .buffer
            .clone();
        assert_eq!(frames[0], direct);
    }

    #[tokio::test]
    async fn todo_events_restore_public_names_and_ignore_errors_and_live_refresh() {
        let mut app = app();
        let mut remote = crate::tui::backend::ReplayRemoteState::default();
        let mut turn = 0;
        let output =
            json!([{"name":"Historical pinned work","status":"in_progress","priority":"high"}])
                .to_string();
        let event = crate::replay::ReplayEvent::Server(crate::protocol::ServerEvent::ToolDone {
            id: "recorded_tool".into(),
            name: "todo".into(),
            output,
            error: None,
        });
        super::super::replay::apply_replay_event(
            &mut app,
            &mut remote,
            &event,
            &mut turn,
            Some(0.0),
        );
        let restored = app.pinned_todos_payload.clone().unwrap();
        assert!(restored.contains("Historical pinned work"));
        assert!(!app.refresh_pinned_todos_if_needed());
        assert_eq!(app.pinned_todos_payload.as_deref(), Some(restored.as_str()));
        let error = crate::replay::ReplayEvent::Server(crate::protocol::ServerEvent::ToolDone {
            id: "failed_tool".into(),
            name: "todo".into(),
            output: "[]".into(),
            error: Some("failed".into()),
        });
        super::super::replay::apply_replay_event(
            &mut app,
            &mut remote,
            &error,
            &mut turn,
            Some(0.0),
        );
        assert_eq!(app.pinned_todos_payload.as_deref(), Some(restored.as_str()));
    }

    #[tokio::test]
    async fn invalid_dimensions_and_unordered_histories_are_rejected() {
        assert!(app().render_replay_frames(&[], &[0], &[], 0, 30).is_err());
        assert!(
            app()
                .render_replay_frames(&[], &[10, 0], &[], 100, 30)
                .is_err()
        );
        assert!(
            app()
                .render_replay_frames(&[], &[0], &[(10, json!([])), (0, json!([]))], 100, 30)
                .is_err()
        );
    }

    #[test]
    fn ansi_preserves_unicode_and_blank_cell_backgrounds() {
        use ratatui::{layout::Rect, style::Color};
        let mut buffer = Buffer::empty(Rect::new(0, 0, 4, 2));
        buffer[(0, 0)]
            .set_symbol("界")
            .set_fg(Color::Rgb(10, 20, 30));
        buffer[(1, 0)].set_skip(true);
        buffer[(3, 1)].set_bg(Color::Rgb(40, 50, 60));
        let ansi = String::from_utf8(App::replay_frame_ansi(&buffer).unwrap()).unwrap();
        assert!(ansi.starts_with("\x1b[0m\x1b[2J\x1b[H"));
        assert!(ansi.contains("界"));
        assert!(ansi.contains("10;20;30"));
        assert!(ansi.contains("40;50;60"));
    }
}
