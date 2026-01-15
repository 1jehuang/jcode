mod app;
pub mod backend;
mod markdown;
mod stream_buffer;
mod ui;

// ClientApp is deprecated - use App::new_for_remote().run_remote() instead
#[deprecated(note = "Use App::new_for_remote().run_remote() instead")]
pub mod client;

pub use app::{App, DisplayMessage, ProcessingStatus};
pub use backend::{DebugEvent, DebugMessage, RemoteConnection};

use crate::message::ToolCall;
use std::time::Duration;

/// Trait for TUI state - implemented by both App and ClientApp
/// This allows sharing the UI rendering code between standalone and client modes
pub trait TuiState {
    fn display_messages(&self) -> &[DisplayMessage];
    fn streaming_text(&self) -> &str;
    fn input(&self) -> &str;
    fn cursor_pos(&self) -> usize;
    fn is_processing(&self) -> bool;
    fn queued_messages(&self) -> &[String];
    fn scroll_offset(&self) -> usize;
    fn provider_name(&self) -> String;
    fn provider_model(&self) -> String;
    fn mcp_servers(&self) -> Vec<String>;
    fn available_skills(&self) -> Vec<String>;
    fn streaming_tokens(&self) -> (u64, u64);
    fn streaming_tool_calls(&self) -> Vec<ToolCall>;
    fn elapsed(&self) -> Option<Duration>;
    fn status(&self) -> ProcessingStatus;
    fn command_suggestions(&self) -> Vec<(&'static str, &'static str)>;
    fn active_skill(&self) -> Option<String>;
    fn subagent_status(&self) -> Option<String>;
    fn time_since_activity(&self) -> Option<Duration>;
}
