#![allow(dead_code)]

#![allow(dead_code)]

use crate::todo::TodoItem;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use tokio::sync::broadcast;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ToolStatus {
    Running,
    Completed,
    Error,
}

impl ToolStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            ToolStatus::Running => "running",
            ToolStatus::Completed => "completed",
            ToolStatus::Error => "error",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolEvent {
    pub session_id: String,
    pub message_id: String,
    pub tool_call_id: String,
    pub tool_name: String,
    pub status: ToolStatus,
    pub title: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TodoEvent {
    pub session_id: String,
    pub todos: Vec<TodoItem>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolSummaryState {
    pub status: String,
    pub title: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolSummary {
    pub id: String,
    pub tool: String,
    pub state: ToolSummaryState,
}

/// Status update from a subagent (used by Task tool)
#[derive(Clone, Debug)]
pub struct SubagentStatus {
    pub session_id: String,
    pub status: String, // e.g., "calling API", "running grep", "streaming"
}

#[derive(Clone, Debug)]
pub enum BusEvent {
    ToolUpdated(ToolEvent),
    TodoUpdated(TodoEvent),
    SubagentStatus(SubagentStatus),
}

pub struct Bus {
    sender: broadcast::Sender<BusEvent>,
}

impl Bus {
    pub fn global() -> &'static Bus {
        static INSTANCE: OnceLock<Bus> = OnceLock::new();
        INSTANCE.get_or_init(|| {
            let (sender, _) = broadcast::channel(256);
            Bus { sender }
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<BusEvent> {
        self.sender.subscribe()
    }

    pub fn publish(&self, event: BusEvent) {
        let _ = self.sender.send(event);
    }
}
