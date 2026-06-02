use super::format_event_for_expanded;
use super::model::shorten_model_name;
use super::{MemoryActivity, MemoryEvent, MemoryEventKind, MemoryState};

pub(super) fn compact_memory_model_label(model: &str) -> String {
    let trimmed = model.trim();
    let model_name = trimmed
        .rsplit_once('·')
        .map(|(_, model)| model.trim())
        .filter(|model| !model.is_empty())
        .unwrap_or(trimmed);
    shorten_model_name(model_name)
}

pub(super) fn memory_active_summary(state: &MemoryState) -> Option<String> {
    match state {
        MemoryState::Idle => None,
        MemoryState::Embedding => Some("searching".to_string()),
        MemoryState::SidecarChecking { count } => Some(format!("verify {count}")),
        MemoryState::FoundRelevant { count } => Some(format!("ready {count}")),
        MemoryState::Extracting { reason } => Some(if reason.trim().is_empty() {
            "extracting".to_string()
        } else {
            format!("extract {}", reason)
        }),
        MemoryState::Maintaining { phase } => Some(if phase.trim().is_empty() {
            "maintaining".to_string()
        } else {
            format!("maintain {}", phase)
        }),
        MemoryState::ToolAction { action, detail } => Some(if detail.trim().is_empty() {
            action.clone()
        } else {
            format!("{} {}", action, detail)
        }),
    }
}

pub(crate) fn is_traceworthy_memory_event(event: &MemoryEvent) -> bool {
    !matches!(
        event.kind,
        MemoryEventKind::EmbeddingStarted
            | MemoryEventKind::SidecarStarted
            | MemoryEventKind::SidecarNotRelevant
            | MemoryEventKind::SidecarComplete { .. }
    )
}

pub(super) fn memory_last_trace_summary(activity: &MemoryActivity) -> Option<String> {
    let event = activity
        .recent_events
        .iter()
        .find(|event| is_traceworthy_memory_event(event))?;
    let (_, text, _) = format_event_for_expanded(event, 120);
    if text.is_empty() { None } else { Some(text) }
}

pub(super) fn memory_state_detail(state: &MemoryState) -> Option<String> {
    match state {
        MemoryState::Idle => None,
        MemoryState::Embedding => Some("embedding search".to_string()),
        MemoryState::SidecarChecking { count } => Some(format!("checking {} candidate(s)", count)),
        MemoryState::FoundRelevant { count } => Some(format!("found {} relevant", count)),
        MemoryState::Extracting { reason } => Some(if reason.trim().is_empty() {
            "extracting".to_string()
        } else {
            format!("extracting {}", reason)
        }),
        MemoryState::Maintaining { phase } => Some(if phase.trim().is_empty() {
            "maintaining graph".to_string()
        } else {
            format!("maintaining {}", phase)
        }),
        MemoryState::ToolAction { action, detail } => Some(if detail.trim().is_empty() {
            action.clone()
        } else {
            format!("{} {}", action, detail)
        }),
    }
}
