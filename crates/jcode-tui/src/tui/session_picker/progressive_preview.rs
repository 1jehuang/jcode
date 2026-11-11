//! Deliver the transcript before optional filesystem/SSH workspace facts.
use super::*;
use crate::tui::info_widget::InfoWidgetData;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::Instant;

pub(super) struct PendingWorkspaceLoad {
    pub session_id: String,
    pub receiver: Receiver<(Instant, InfoWidgetData)>,
}

/// The timestamp is also a generation token. A late workspace result must not
/// modify a preview replaced by a list refresh or a newer transcript load.
pub(super) fn publish_then_hydrate(
    preview: Option<LoadedSessionPreview>,
    transcript: Sender<Option<LoadedSessionPreview>>,
    workspace: Sender<(Instant, InfoWidgetData)>,
    hydrate: impl FnOnce(&mut InfoWidgetData),
) {
    let Some(mut preview) = preview else {
        if transcript.send(None).is_err() {
            // Navigation already dropped this receiver. Nothing remains to publish.
            return;
        }
        return;
    };
    let generation = Instant::now();
    preview.extras.loaded_at = Some(generation);
    // A transcript can arrive before any optional workspace metadata.
    let mut data = preview
        .extras
        .info_data
        .as_ref()
        .map_or_else(InfoWidgetData::default, Clone::clone);
    // Navigating away drops the receiver. Do not start a potentially slow Git
    // or SSH probe for a transcript that nobody is waiting for anymore.
    if transcript.send(Some(preview)).is_err() {
        return;
    }
    hydrate(&mut data);
    if workspace.send((generation, data)).is_err() {
        crate::logging::debug("Workspace preview receiver closed after navigation");
    }
}

impl SessionPicker {
    /// Leave a completed/disconnected receiver for the normal render pass to
    /// drain, without spawning filesystem or workspace probes in UI tests.
    #[cfg(test)]
    pub(crate) fn set_pending_preview_for_test(&mut self, workspace: bool) {
        if workspace {
            let (_, receiver) = std::sync::mpsc::channel();
            self.pending_workspace_load = Some(PendingWorkspaceLoad {
                session_id: "selected".into(),
                receiver,
            });
        } else {
            let (_, receiver) = std::sync::mpsc::channel();
            self.pending_preview_load = Some(PendingSessionPreviewLoad {
                session_id: "selected".into(),
                receiver,
            });
        }
    }

    pub(super) fn poll_workspace_load(&mut self) -> bool {
        let Some(pending) = self.pending_workspace_load.as_ref() else {
            return false;
        };
        match pending.receiver.try_recv() {
            Ok((generation, data)) => {
                let Some(pending) = self.pending_workspace_load.take() else {
                    return false;
                };
                if let Some(extras) = self.preview_extras.get_mut(&pending.session_id)
                    && extras.loaded_at == Some(generation)
                {
                    extras.info_data = Some(data);
                    if self
                        .selected_session()
                        .is_some_and(|s| s.id == pending.session_id)
                    {
                        self.resume_latency.workspace(&pending.session_id);
                    }
                    return true;
                }
                false
            }
            Err(TryRecvError::Disconnected) => {
                self.pending_workspace_load = None;
                false
            }
            Err(TryRecvError::Empty) => false,
        }
    }

    /// Poll promptly only while asynchronous work can change the displayed
    /// preview. Idle pickers retain their existing low-frequency wakeup rate.
    pub(crate) fn has_pending_preview_work(&self) -> bool {
        self.search_edit_pending
            || self.transcript_search.pending()
            || self.pending_preview_load.is_some()
            || self.pending_workspace_load.is_some()
    }

    pub(super) fn preview_poll_interval(&self) -> Duration {
        if self.has_pending_preview_work() {
            Duration::from_millis(16)
        } else {
            Duration::from_millis(100)
        }
    }
}

#[cfg(test)]
#[path = "progressive_preview_tests.rs"]
mod tests;
