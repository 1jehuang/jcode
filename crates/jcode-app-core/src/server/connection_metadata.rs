//! Connection metadata transitions and retained event-buffer cleanup.

use super::*;

/// Only metadata-changing events need the shared connection write lock.
/// Borrow tool names until the lock is held, avoiding allocations for lookup misses.
pub(super) enum ConnectionMetadataUpdate<'a> {
    TurnStarted,
    ToolStarted(&'a str),
    ToolDone,
    TurnEnded,
}

pub(super) fn release_large_event_buffer(buffer: &mut Vec<u8>) {
    // Keep ordinary streaming allocations, but do not pin a large History or
    // tool-result payload for the rest of a long-lived connection.
    if buffer.capacity() > 64 * 1024 {
        *buffer = Vec::new();
    }
}

impl<'a> ConnectionMetadataUpdate<'a> {
    pub(super) fn from_event(event: &'a ServerEvent) -> Option<Self> {
        // Deliberately unwrap exactly one level, matching the forwarder's original
        // behavior. Nested wrappers are not metadata transitions.
        let observed = match event {
            ServerEvent::TurnEvent { event, .. } => event.as_ref(),
            event => event,
        };
        match observed {
            ServerEvent::TurnStarted { .. } => Some(Self::TurnStarted),
            ServerEvent::ToolStart { name, .. } => Some(Self::ToolStarted(name)),
            ServerEvent::ToolDone { .. } => Some(Self::ToolDone),
            ServerEvent::Done { .. } | ServerEvent::Error { .. } | ServerEvent::Interrupted => {
                Some(Self::TurnEnded)
            }
            _ => None,
        }
    }

    pub(super) fn apply(self, is_processing: &mut bool, current_tool_name: &mut Option<String>) {
        match self {
            Self::TurnStarted => {
                *is_processing = true;
                *current_tool_name = None;
            }
            Self::ToolStarted(name) => {
                *is_processing = true;
                *current_tool_name = Some(name.to_owned());
            }
            Self::ToolDone => *current_tool_name = None,
            Self::TurnEnded => {
                *is_processing = false;
                *current_tool_name = None;
            }
        }
    }
}
