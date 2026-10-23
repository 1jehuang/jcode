use super::*;

impl Session {
    /// Stored-message indices of the rewind targets shown in the TUI's
    /// numbered `/rewind` list, in display order.
    ///
    /// The TUI numbers user/assistant *transcript entries* (what the user
    /// actually sees), not raw stored messages. Stored tool-result messages
    /// and tool-call-only assistant messages render as tool cards or nothing,
    /// so counting raw stored messages diverges wildly from the on-screen
    /// numbering in tool-heavy sessions (issue #432). Deriving targets from
    /// the same rendering used for the transcript keeps `/rewind N` aligned
    /// with the numbers `/rewind` prints.
    ///
    /// A single stored message can produce multiple transcript entries (text
    /// split around a tool result); each entry keeps its own number and maps
    /// to the same stored index so numbering matches the visible list exactly.
    pub fn rewind_target_stored_indices(&self) -> Vec<usize> {
        render_messages(self)
            .into_iter()
            .filter(|message| matches!(message.role.as_str(), "user" | "assistant"))
            .filter_map(|message| message.stored_index)
            .collect()
    }

    /// Number of `/rewind` targets (see [`Self::rewind_target_stored_indices`]).
    pub fn rewind_target_count(&self) -> usize {
        self.rewind_target_stored_indices().len()
    }

    /// Inclusive stored boundary for the legacy 1-based rendered message number.
    pub fn rewind_stored_len_for_message(&self, message_index: usize) -> Option<usize> {
        message_index
            .checked_sub(1)
            .and_then(|index| self.rewind_target_stored_indices().get(index).copied())
            .map(|index| index + 1)
    }

    /// Assistant picker targets use the same renderer as numeric rewind, but
    /// include all compacted history and collapse split rows to one stored ID.
    pub fn rewind_assistant_target_stored_indices(&self) -> Vec<usize> {
        let mut seen = HashSet::new();
        render_messages_and_images_with_compacted_history(self, usize::MAX)
            .0
            .into_iter()
            .filter(|message| message.role == "assistant")
            .filter_map(|message| message.stored_index)
            .filter(|&index| seen.insert(self.messages[index].id.clone()))
            .collect()
    }

    /// Resolve an immutable assistant picker identity to its inclusive boundary.
    /// Tool-only and hidden stored messages are deliberately not targetable.
    pub fn rewind_stored_len_for_assistant(&self, message_id: &str) -> Option<usize> {
        self.rewind_assistant_target_stored_indices()
            .into_iter()
            .find(|&index| self.messages[index].id == message_id)
            .map(|index| index + 1)
    }

    /// Persist an intentional transcript replacement without allowing an old
    /// append journal or compacted/provider branch to resurrect removed turns.
    pub(crate) fn persist_conversation_rewind(&mut self) -> anyhow::Result<()> {
        self.compaction = None;
        self.provider_session_id = None;
        self.model_usage_turn_id = None;
        self.save_rewind()
    }

    /// Conversation-only inclusive boundary used by legacy numeric rewind.
    /// File history stays untouched so its one-slot transcript undo remains usable.
    pub fn rewind_conversation_to_len(&mut self, keep_len: usize) -> anyhow::Result<()> {
        anyhow::ensure!(keep_len <= self.messages.len(), "Invalid rewind boundary");
        self.ensure_rewind_writable()?;
        if keep_len == self.messages.len() {
            return Ok(());
        }
        let previous = self.clone();
        self.truncate_messages(keep_len);
        self.finish_transcript_rewind(previous)
    }

    /// Restore an Agent's in-memory transcript snapshot, never filesystem state
    /// or a provider's old resumable conversation. The undo slot is not durable.
    pub fn restore_rewind_messages(&mut self, messages: Vec<StoredMessage>) -> anyhow::Result<()> {
        self.ensure_rewind_writable()?;
        let previous = self.clone();
        self.replace_messages(messages);
        self.finish_transcript_rewind(previous)
    }

    fn ensure_rewind_writable(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.persist_state.transcript_stripped,
            "Cannot rewind a transcript-less remote session stub"
        );
        anyhow::ensure!(
            self.migration_lease_block().is_none(),
            "Session is owned by another machine"
        );
        Ok(())
    }

    fn finish_transcript_rewind(&mut self, previous: Self) -> anyhow::Result<()> {
        let result = self.persist_conversation_rewind();
        if result.is_err()
            && !self.has_pending_rewind()
            && !crate::file_history::rewind_boundary_saved(self)
        {
            *self = previous;
        }
        result
    }
}
