use super::*;
use crate::protocol::RewindTargetKind;

impl Dialog {
    pub fn rewind_loading() -> Self {
        Self {
            stack: vec![View::new(Route::RewindList, "Rewind")],
            mcp: None,
            skills: None,
            plugin: None,
            rewind: Some(Vec::new()),
            pending_rewind: false,
            status: Some("Loading checkpoints…".to_string()),
            pending_mcp: None,
        }
    }

    pub fn is_rewind(&self) -> bool {
        self.rewind.is_some()
    }

    pub fn set_rewind_checkpoints(&mut self, checkpoints: Vec<RewindCheckpoint>) {
        if !self.is_rewind() {
            return;
        }
        self.rewind = Some(checkpoints);
        self.status = None;
        self.stack = vec![self.rewind_list_view()];
    }

    pub(super) fn rewind_list_view(&self) -> View {
        let mut view = View::new(Route::RewindList, "Rewind");
        view.subtitle = "Choose a message to restore to".to_string();
        view.empty = "No checkpoints in this conversation.".to_string();
        view.searchable = true;
        // Keep transcript order. The current-position row is the safe default;
        // Up selects the latest message without wrapping to the oldest.
        for checkpoint in self.rewind.iter().flatten() {
            let mut meta = vec![match checkpoint.target_kind {
                RewindTargetKind::UserPrompt => "You".to_string(),
                RewindTargetKind::AssistantMessage => "Assistant".to_string(),
            }];
            meta.extend(checkpoint.timestamp.iter().cloned());
            view.items.push(Item {
                group: None,
                icon: None,
                label: checkpoint
                    .prompt
                    .lines()
                    .next()
                    .unwrap_or("(empty message)")
                    .to_string(),
                meta,
                search_text: checkpoint.prompt.clone(),
                action: Some(ItemAction::Open(Route::RewindModes(checkpoint.id.clone()))),
            });
        }
        view.items.push(Item {
            group: None,
            icon: None,
            label: "(current)".to_string(),
            meta: Vec::new(),
            search_text: String::new(),
            action: Some(ItemAction::Close),
        });
        view.selected = view.items.len() - 1;
        view.notes
            .push("Only file changes tracked by Jcode can be restored.".to_string());
        view
    }

    pub(super) fn rewind_modes_view(&self, id: &str) -> View {
        let mut view = View::new(Route::RewindModes(id.to_string()), "Restore checkpoint");
        let Some(checkpoint) = self.rewind.iter().flatten().find(|row| row.id == id) else {
            view.empty = "This checkpoint is no longer available.".to_string();
            return view;
        };
        view.subtitle = checkpoint
            .prompt
            .lines()
            .next()
            .unwrap_or("(empty message)")
            .to_string();
        view.numbered = true;
        let is_user = checkpoint.target_kind == RewindTargetKind::UserPrompt;
        let files_available =
            is_user && checkpoint.can_restore_files && checkpoint.changed_file_count > 0;
        let mut modes = Vec::new();
        if files_available {
            modes.push((
                RewindMode::ConversationAndFiles,
                "Restore conversation and files",
            ));
        }
        modes.push((RewindMode::ConversationOnly, "Restore conversation only"));
        if files_available {
            modes.push((RewindMode::FilesOnly, "Restore files only"));
        }
        for (mode, label) in modes {
            view.items.push(Item {
                group: None,
                icon: None,
                label: label.to_string(),
                meta: Vec::new(),
                search_text: String::new(),
                action: Some(ItemAction::Rewind(id.to_string(), mode)),
            });
        }
        view.items.push(Item {
            group: None,
            icon: None,
            label: "Never mind".to_string(),
            meta: Vec::new(),
            search_text: String::new(),
            action: Some(ItemAction::Back),
        });
        if !is_user {
            view.notes
                .push("Restores the conversation to after this assistant message.".to_string());
            view.notes.push(
                "Keeps this assistant message. Files and your draft stay unchanged.".to_string(),
            );
            return view;
        }
        view.notes.push(if files_available {
            format!(
                "{} tracked file(s) changed since this message.",
                checkpoint.changed_file_count
            )
        } else if checkpoint.changed_file_count > 0 {
            "Tracked file changes cannot be restored at this checkpoint.".to_string()
        } else {
            "No tracked file changes to restore.".to_string()
        });
        view.notes
            .push("Conversation restore returns to before this user message and puts the original prompt in the editor.".to_string());
        view
    }
}
