//! `/loop` command family.
//!
//! Manages recurring task loops that persist across sessions:
//!
//! ```text
//! /loop <interval> <task>   Create a recurring loop (e.g. /loop 30m check builds)
//! /loop status              Show all active loops
//! /loop cancel <id>         Cancel a loop
//! /loop pause <id>          Pause a loop
//! /loop resume <id>         Resume a paused loop
//! ```

use super::{App, DisplayMessage};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum LoopStatus {
    Running,
    Paused,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopItem {
    pub id: String,
    pub task: String,
    pub interval_minutes: u32,
    pub status: LoopStatus,
    pub created_at: DateTime<Utc>,
    pub last_run: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LoopStore {
    pub loops: Vec<LoopItem>,
}

// ---------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------

fn loops_dir() -> Option<PathBuf> {
    crate::storage::jcode_dir().ok().map(|d| d.join("loops"))
}

fn loops_file() -> Option<PathBuf> {
    loops_dir().map(|d| d.join("loops.json"))
}

fn load_loops() -> LoopStore {
    let Some(path) = loops_file() else {
        return LoopStore::default();
    };
    crate::storage::read_json(&path).unwrap_or_default()
}

fn save_loops(store: &LoopStore) -> Result<(), String> {
    let Some(path) = loops_file() else {
        return Err("Cannot resolve loops directory".into());
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    crate::storage::write_json(&path, store).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Interval parsing
// ---------------------------------------------------------------------------

fn parse_interval(input: &str) -> Option<u32> {
    let input = input.trim().to_ascii_lowercase();
    if input.is_empty() {
        return None;
    }
    let (num_part, unit) = input.split_at(input.len() - 1);
    let value: u32 = num_part.parse().ok()?;
    match unit {
        "m" => Some(value),
        "h" => Some(value * 60),
        "d" => Some(value * 60 * 24),
        _ => None,
    }
}

fn format_interval(minutes: u32) -> String {
    if minutes < 60 {
        format!("{minutes}m")
    } else if minutes < 60 * 24 {
        format!("{}h", minutes / 60)
    } else {
        format!("{}d", minutes / (60 * 24))
    }
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

/// Dispatch `/loop ...`. Returns `false` when `trimmed` is not a `/loop`
/// command so callers can fall through to other handlers.
pub(super) fn handle_loop_command(app: &mut App, trimmed: &str) -> bool {
    let Some(rest) = trimmed.strip_prefix("/loop") else {
        return false;
    };
    let rest = rest.trim();

    if rest.is_empty() || matches!(rest, "help" | "--help" | "-h") {
        app.push_display_message(DisplayMessage::system(loop_usage()));
        return true;
    }
    if rest == "status" {
        return show_loop_status(app);
    }
    if let Some(id) = rest.strip_prefix("cancel ") {
        return cancel_loop(app, id.trim());
    }
    if let Some(id) = rest.strip_prefix("pause ") {
        return pause_loop(app, id.trim());
    }
    if let Some(id) = rest.strip_prefix("resume ") {
        return resume_loop(app, id.trim());
    }
    // /loop <interval> <task>
    create_loop(app, rest)
}

// ---------------------------------------------------------------------------
// Subcommands
// ---------------------------------------------------------------------------

fn loop_usage() -> String {
    "/loop <interval> <task> | status | cancel <id> | pause <id> | resume <id>\n\
     \u{2022} Intervals: 5m, 30m, 1h, 2h, 1d\n\
     \u{2022} Example: /loop 30m run cargo test"
        .into()
}

fn create_loop(app: &mut App, rest: &str) -> bool {
    let Some(space_pos) = rest.find(char::is_whitespace) else {
        app.push_display_message(DisplayMessage::error(
            "Usage: /loop <interval> <task> (e.g. /loop 30m check builds)",
        ));
        return true;
    };
    let (interval_str, task) = rest.split_at(space_pos);
    let task = task.trim();
    if task.is_empty() {
        app.push_display_message(DisplayMessage::error(
            "Task description is required. Usage: /loop <interval> <task>",
        ));
        return true;
    }
    let Some(minutes) = parse_interval(interval_str) else {
        app.push_display_message(DisplayMessage::error(
            "Invalid interval. Use: 5m, 30m, 1h, 2h, 1d",
        ));
        return true;
    };
    if minutes == 0 {
        app.push_display_message(DisplayMessage::error("Interval must be greater than zero."));
        return true;
    }

    let item = LoopItem {
        id: crate::id::new_id("loop"),
        task: task.to_string(),
        interval_minutes: minutes,
        status: LoopStatus::Running,
        created_at: Utc::now(),
        last_run: None,
    };

    let mut store = load_loops();
    store.loops.push(item.clone());
    if let Err(e) = save_loops(&store) {
        app.push_display_message(DisplayMessage::error(format!("Failed to save loop: {e}")));
        return true;
    }

    app.push_display_message(DisplayMessage::system(format!(
        "Loop created: {} (every {}) -> \"{}\"",
        item.id,
        format_interval(item.interval_minutes),
        item.task,
    )));
    true
}

fn show_loop_status(app: &mut App) -> bool {
    let store = load_loops();
    let active: Vec<_> = store
        .loops
        .iter()
        .filter(|l| l.status != LoopStatus::Cancelled)
        .collect();

    if active.is_empty() {
        app.push_display_message(DisplayMessage::system("No active loops."));
        return true;
    }

    let mut msg = format!("Active loops ({}):\n", active.len());
    for item in &active {
        let status_str = match item.status {
            LoopStatus::Running => "running",
            LoopStatus::Paused => "paused",
            LoopStatus::Cancelled => "cancelled",
        };
        let last = item
            .last_run
            .map(|t| format!("last: {t}"))
            .unwrap_or_else(|| "never run".into());
        msg.push_str(&format!(
            "  {} [{}] every {} \"{}\" ({})\n",
            item.id,
            status_str,
            format_interval(item.interval_minutes),
            item.task,
            last,
        ));
    }
    app.push_display_message(DisplayMessage::system(msg));
    true
}

fn find_loop_mut<'a>(store: &'a mut LoopStore, id: &str) -> Option<&'a mut LoopItem> {
    store.loops.iter_mut().find(|l| l.id == id)
}

fn cancel_loop(app: &mut App, id: &str) -> bool {
    let mut store = load_loops();
    let Some(item) = find_loop_mut(&mut store, id) else {
        app.push_display_message(DisplayMessage::error(format!("Loop not found: {id}")));
        return true;
    };
    if item.status == LoopStatus::Cancelled {
        app.push_display_message(DisplayMessage::system(format!(
            "Loop {id} is already cancelled."
        )));
        return true;
    }
    item.status = LoopStatus::Cancelled;
    if let Err(e) = save_loops(&store) {
        app.push_display_message(DisplayMessage::error(format!("Failed to save: {e}")));
        return true;
    }
    app.push_display_message(DisplayMessage::system(format!(
        "Loop {id} cancelled."
    )));
    true
}

fn pause_loop(app: &mut App, id: &str) -> bool {
    let mut store = load_loops();
    let Some(item) = find_loop_mut(&mut store, id) else {
        app.push_display_message(DisplayMessage::error(format!("Loop not found: {id}")));
        return true;
    };
    if item.status == LoopStatus::Cancelled {
        app.push_display_message(DisplayMessage::error(format!(
            "Loop {id} is cancelled. Create a new one."
        )));
        return true;
    }
    if item.status == LoopStatus::Paused {
        app.push_display_message(DisplayMessage::system(format!(
            "Loop {id} is already paused."
        )));
        return true;
    }
    item.status = LoopStatus::Paused;
    if let Err(e) = save_loops(&store) {
        app.push_display_message(DisplayMessage::error(format!("Failed to save: {e}")));
        return true;
    }
    app.push_display_message(DisplayMessage::system(format!(
        "Loop {id} paused."
    )));
    true
}

fn resume_loop(app: &mut App, id: &str) -> bool {
    let mut store = load_loops();
    let Some(item) = find_loop_mut(&mut store, id) else {
        app.push_display_message(DisplayMessage::error(format!("Loop not found: {id}")));
        return true;
    };
    if item.status == LoopStatus::Cancelled {
        app.push_display_message(DisplayMessage::error(format!(
            "Loop {id} is cancelled. Create a new one."
        )));
        return true;
    }
    if item.status == LoopStatus::Running {
        app.push_display_message(DisplayMessage::system(format!(
            "Loop {id} is already running."
        )));
        return true;
    }
    item.status = LoopStatus::Running;
    if let Err(e) = save_loops(&store) {
        app.push_display_message(DisplayMessage::error(format!("Failed to save: {e}")));
        return true;
    }
    app.push_display_message(DisplayMessage::system(format!(
        "Loop {id} resumed."
    )));
    true
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_interval_minutes() {
        assert_eq!(parse_interval("5m"), Some(5));
        assert_eq!(parse_interval("30m"), Some(30));
        assert_eq!(parse_interval("1h"), Some(60));
        assert_eq!(parse_interval("2h"), Some(120));
        assert_eq!(parse_interval("1d"), Some(1440));
    }

    #[test]
    fn parse_interval_rejects_invalid() {
        assert_eq!(parse_interval(""), None);
        assert_eq!(parse_interval("abc"), None);
        assert_eq!(parse_interval("5x"), None);
        assert_eq!(parse_interval("0m"), Some(0));
    }

    #[test]
    fn format_interval_round_trips() {
        assert_eq!(format_interval(5), "5m");
        assert_eq!(format_interval(60), "1h");
        assert_eq!(format_interval(1440), "1d");
    }

    #[test]
    fn loop_store_serde_round_trip() {
        let store = LoopStore {
            loops: vec![LoopItem {
                id: "loop_test".into(),
                task: "test task".into(),
                interval_minutes: 30,
                status: LoopStatus::Running,
                created_at: Utc::now(),
                last_run: None,
            }],
        };
        let json = serde_json::to_string(&store).unwrap();
        let decoded: LoopStore = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.loops.len(), 1);
        assert_eq!(decoded.loops[0].id, "loop_test");
    }
}
