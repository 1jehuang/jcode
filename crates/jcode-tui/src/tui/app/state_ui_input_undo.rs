impl App {
    pub(super) fn remember_input_undo_state(&mut self) {
        self.ensure_input_image_markers();
        self.input_typing_undo = None;
        let images = self
            .input_undo_stack
            .last()
            .filter(|snapshot| snapshot.2.as_ref() == &self.pending_images)
            .map(|snapshot| std::sync::Arc::clone(&snapshot.2))
            .unwrap_or_else(|| std::sync::Arc::new(self.pending_images.clone()));
        let snapshot = (
            self.input.clone(),
            self.cursor_pos.min(self.input.len()),
            images,
            self.input_image_markers.clone(),
        );
        if self.input_undo_stack.last() == Some(&snapshot)
            && self.input_undo_image_counts.last() == Some(&self.pending_images.len())
        {
            return;
        }
        self.push_input_undo_snapshot(snapshot);
    }

    /// Push an undo entry together with the current attachment count,
    /// trimming the oldest entry at the cap.
    pub(super) fn push_input_undo_snapshot(
        &mut self,
        snapshot: super::input_images::InputUndoSnapshot,
    ) {
        if self.input_undo_stack.len() >= Self::INPUT_UNDO_LIMIT {
            self.trim_oldest_input_undo_entry();
        }
        self.input_undo_stack.push(snapshot);
        self.input_undo_image_counts.push(self.pending_images.len());
    }

    /// Drop the oldest undo entry, keeping any stashed Ctrl+C images pointed at
    /// the same snapshot (or dropping them if that snapshot is the one removed).
    pub(super) fn trim_oldest_input_undo_entry(&mut self) {
        self.input_undo_stack.remove(0);
        if !self.input_undo_image_counts.is_empty() {
            self.input_undo_image_counts.remove(0);
        }
        self.cleared_draft_images.retain_mut(|(at, _)| {
            *at -= 1;
            *at > 0
        });
    }

    pub(super) fn clear_input_undo_history(&mut self) {
        self.input_undo_stack.clear();
        self.input_undo_image_counts.clear();
        self.cleared_draft_images.clear();
        self.input_typing_undo = None;
        self.history_draft = None;
    }
}
