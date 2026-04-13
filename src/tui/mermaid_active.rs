use super::ACTIVE_DIAGRAMS_MAX;
use crate::tui::info_widget::DiagramInfo;
use std::sync::{LazyLock, Mutex};

/// Active diagrams for info widget display
/// Updated during markdown rendering, queried by info_widget_data()
static ACTIVE_DIAGRAMS: LazyLock<Mutex<Vec<ActiveDiagram>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

/// Ephemeral diagram preview for in-flight streaming markdown.
/// This should never persist once a streaming segment is committed.
static STREAMING_PREVIEW_DIAGRAM: LazyLock<Mutex<Option<ActiveDiagram>>> =
    LazyLock::new(|| Mutex::new(None));

/// Info about an active diagram (for info widget)
#[derive(Clone)]
struct ActiveDiagram {
    hash: u64,
    width: u32,
    height: u32,
    label: Option<String>,
}

fn to_diagram_info(diagram: ActiveDiagram) -> DiagramInfo {
    DiagramInfo {
        hash: diagram.hash,
        width: diagram.width,
        height: diagram.height,
        label: diagram.label,
    }
}

fn to_active_diagram(diagram: DiagramInfo) -> ActiveDiagram {
    ActiveDiagram {
        hash: diagram.hash,
        width: diagram.width,
        height: diagram.height,
        label: diagram.label,
    }
}

pub fn register_active_diagram(hash: u64, width: u32, height: u32, label: Option<String>) {
    if let Ok(mut diagrams) = ACTIVE_DIAGRAMS.lock() {
        if let Some(pos) = diagrams.iter().position(|d| d.hash == hash) {
            let mut existing = diagrams.remove(pos);
            existing.width = width;
            existing.height = height;
            if label.is_some() {
                existing.label = label;
            }
            diagrams.push(existing);
        } else {
            diagrams.push(ActiveDiagram {
                hash,
                width,
                height,
                label,
            });
        }
        while diagrams.len() > ACTIVE_DIAGRAMS_MAX {
            diagrams.remove(0);
        }
    }
}

/// Register or replace the current streaming preview diagram.
pub fn set_streaming_preview_diagram(hash: u64, width: u32, height: u32, label: Option<String>) {
    if let Ok(mut preview) = STREAMING_PREVIEW_DIAGRAM.lock() {
        *preview = Some(ActiveDiagram {
            hash,
            width,
            height,
            label,
        });
    }
}

/// Clear the current streaming preview diagram.
pub fn clear_streaming_preview_diagram() {
    if let Ok(mut preview) = STREAMING_PREVIEW_DIAGRAM.lock() {
        *preview = None;
    }
}

/// Get active diagrams for info widget display
pub fn get_active_diagrams() -> Vec<DiagramInfo> {
    let preview = STREAMING_PREVIEW_DIAGRAM
        .lock()
        .ok()
        .and_then(|preview| preview.clone());
    let preview_hash = preview.as_ref().map(|d| d.hash);

    let mut out = Vec::new();
    if let Some(diagram) = preview {
        out.push(to_diagram_info(diagram));
    }

    if let Ok(diagrams) = ACTIVE_DIAGRAMS.lock() {
        out.extend(
            diagrams
                .iter()
                .rev()
                .filter(|d| Some(d.hash) != preview_hash)
                .cloned()
                .map(to_diagram_info),
        );
    }

    out
}

/// Snapshot active diagrams (internal order) for temporary overrides in tests/debug
pub fn snapshot_active_diagrams() -> Vec<DiagramInfo> {
    ACTIVE_DIAGRAMS
        .lock()
        .ok()
        .map(|diagrams| diagrams.iter().cloned().map(to_diagram_info).collect())
        .unwrap_or_default()
}

/// Restore active diagrams from a snapshot
pub fn restore_active_diagrams(snapshot: Vec<DiagramInfo>) {
    if let Ok(mut diagrams) = ACTIVE_DIAGRAMS.lock() {
        diagrams.clear();
        diagrams.extend(snapshot.into_iter().map(to_active_diagram));
        while diagrams.len() > ACTIVE_DIAGRAMS_MAX {
            diagrams.remove(0);
        }
    }
}

pub fn active_diagram_count() -> usize {
    ACTIVE_DIAGRAMS
        .lock()
        .ok()
        .map(|diagrams| diagrams.len())
        .unwrap_or(0)
}

/// Clear active diagrams (call at start of render cycle)
pub fn clear_active_diagrams() {
    if let Ok(mut diagrams) = ACTIVE_DIAGRAMS.lock() {
        diagrams.clear();
    }
    clear_streaming_preview_diagram();
}
