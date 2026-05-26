use super::*;
use crate::tui::{TuiState, detect_kv_cache_problem, ui};

impl App {
    pub(super) fn current_skills_snapshot(&self) -> std::sync::Arc<crate::skill::SkillRegistry> {
        self.registry
            .skills()
            .try_read()
            .map(|skills| std::sync::Arc::new(skills.clone()))
            .unwrap_or_else(|_| self.skills.clone())
    }

    pub fn cursor_pos(&self) -> usize {
        self.cursor_pos
    }

    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    pub fn is_processing(&self) -> bool {
        self.is_processing || self.pending_queued_dispatch || self.split_launch_in_flight()
    }

    pub fn streaming_text(&self) -> &str {
        &self.streaming_text
    }

    pub fn active_skill(&self) -> Option<&str> {
        self.active_skill.as_deref()
    }

    pub fn available_skills(&self) -> Vec<String> {
        let skills = self.current_skills_snapshot();
        skills.list().iter().map(|s| s.name.clone()).collect()
    }

    pub fn queued_count(&self) -> usize {
        self.queued_messages.len() + self.hidden_queued_system_messages.len()
    }

    pub fn queued_messages(&self) -> &[String] {
        &self.queued_messages
    }

    pub fn streaming_tokens(&self) -> (u64, u64) {
        (self.streaming_input_tokens, self.streaming_output_tokens)
    }

    pub(super) fn build_turn_footer(&self, duration: Option<f32>) -> Option<String> {
        let mut parts = Vec::new();
        if let Some(secs) = duration {
            let duration_ms = (secs.max(0.0) * 1000.0).round() as u64;
            parts.push(Message::format_duration(duration_ms));
        }
        if let Some(tps) = self.compute_streaming_tps() {
            parts.push(format!("{:.1} tps", tps));
        }
        if self.streaming_input_tokens > 0 || self.streaming_output_tokens > 0 {
            parts.push(format!(
                "↑{} ↓{}",
                format_tokens(self.streaming_input_tokens),
                format_tokens(self.streaming_output_tokens)
            ));
        }
        if let Some(cache) = format_cache_footer(
            self.streaming_cache_read_tokens,
            self.streaming_cache_creation_tokens,
        ) {
            parts.push(cache);
        }

        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" · "))
        }
    }

    pub(super) fn has_streaming_footer_stats(&self) -> bool {
        self.streaming_input_tokens > 0
            || self.streaming_output_tokens > 0
            || self.streaming_cache_read_tokens.is_some()
            || self.streaming_cache_creation_tokens.is_some()
            || self.compute_streaming_tps().is_some()
    }

    pub(super) fn push_turn_footer(&mut self, duration: Option<f32>) {
        self.log_cache_miss_if_unexpected();
        self.record_completed_stream_cache_usage();

        self.last_api_completed = Some(Instant::now());
        self.last_api_completed_provider = Some(<Self as TuiState>::provider_name(self));
        self.last_api_completed_model = Some(<Self as TuiState>::provider_model(self));
        self.last_turn_input_tokens = {
            let input = self.streaming_input_tokens;
            if input > 0 { Some(input) } else { None }
        };

        if let Some(footer) = self.build_turn_footer(duration) {
            self.push_display_message(DisplayMessage {
                role: "meta".to_string(),
                content: footer,
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            });
        }
    }

    /// Log detailed info when an unexpected cache miss occurs (cache write on turn 3+)
    pub(super) fn log_cache_miss_if_unexpected(&self) {
        let user_turn_count = self
            .display_messages
            .iter()
            .filter(|m| m.role == "user")
            .count();

        let provider = <Self as TuiState>::provider_name(self);
        let upstream_provider = self.upstream_provider();
        let cache_ttl = self.cache_ttl_status();
        let cache_problem = detect_kv_cache_problem(
            &provider,
            upstream_provider,
            user_turn_count,
            self.streaming_input_tokens,
            self.streaming_cache_read_tokens,
            self.streaming_cache_creation_tokens,
            cache_ttl.as_ref(),
        );

        if let Some(problem) = cache_problem {
            // Collect context for debugging
            let session_id = self.session_id().to_string();
            let model = <Self as TuiState>::provider_model(self);
            let input_tokens = self.streaming_input_tokens;
            let output_tokens = self.streaming_output_tokens;

            // Format as Option to distinguish None vs Some(0)
            let cache_creation_dbg = format!("{:?}", self.streaming_cache_creation_tokens);
            let cache_read_dbg = format!("{:?}", self.streaming_cache_read_tokens);

            // Count message types in conversation
            let mut user_msgs = 0;
            let mut assistant_msgs = 0;
            let mut tool_msgs = 0;
            let mut other_msgs = 0;
            for msg in &self.display_messages {
                match msg.role.as_str() {
                    "user" => user_msgs += 1,
                    "assistant" => assistant_msgs += 1,
                    "tool_result" | "tool_use" => tool_msgs += 1,
                    _ => other_msgs += 1,
                }
            }

            crate::logging::warn(&format!(
                "CACHE_MISS: {} on turn {} | \
                 cache_creation={} cache_read={} | \
                 input={} output={} affected={:?} | \
                 session={} provider={} upstream={:?} model={} | \
                 msgs: user={} assistant={} tool={} other={}",
                problem.log_reason(),
                user_turn_count,
                cache_creation_dbg,
                cache_read_dbg,
                input_tokens,
                output_tokens,
                problem.affected_tokens,
                session_id,
                provider,
                upstream_provider,
                model,
                user_msgs,
                assistant_msgs,
                tool_msgs,
                other_msgs
            ));
        }
    }

    /// Check if approaching context limit and show warning
    pub(super) fn check_context_warning(&mut self, input_tokens: u64) {
        let usage_percent = (input_tokens as f64 / self.context_limit as f64) * 100.0;

        // Warn at 70%, 80%, 90%
        if !self.context_warning_shown && usage_percent >= 70.0 {
            let warning = format!(
                "\n⚠️  Context usage: {:.0}% ({}/{}k tokens) - compaction approaching\n\n",
                usage_percent,
                input_tokens / 1000,
                self.context_limit / 1000
            );
            self.append_streaming_text(&warning);
            self.context_warning_shown = true;
        } else if self.context_warning_shown && usage_percent >= 80.0 {
            // Reset to show 80% warning
            if usage_percent < 85.0 {
                let warning = format!(
                    "\n⚠️  Context usage: {:.0}% - compaction imminent\n\n",
                    usage_percent
                );
                self.append_streaming_text(&warning);
            }
        }
    }

    /// Get context usage as percentage
    pub fn context_usage_percent(&self) -> f64 {
        self.current_stream_context_tokens()
            .map(|tokens| (tokens as f64 / self.context_limit as f64) * 100.0)
            .unwrap_or(0.0)
    }

    /// Time since last streaming event (for detecting stale connections)
    pub fn time_since_activity(&self) -> Option<Duration> {
        if let Some(last_activity) = self.last_stream_activity {
            return Some(last_activity.elapsed());
        }
        if !self.display_messages.is_empty() && !self.is_processing {
            return Some(crate::tui::REDRAW_DEEP_IDLE_AFTER + Duration::from_secs(1));
        }
        Some(self.app_started.elapsed())
    }

    pub(super) fn split_launch_in_flight(&self) -> bool {
        self.is_remote
            && !self.is_processing
            && self
                .pending_split_started_at
                .is_some_and(|started_at| started_at.elapsed() < Duration::from_millis(350))
    }

    pub fn streaming_tool_calls(&self) -> &[ToolCall] {
        &self.streaming_tool_calls
    }

    pub fn status(&self) -> &ProcessingStatus {
        &self.status
    }

    pub fn subagent_status(&self) -> Option<&str> {
        self.subagent_status.as_deref()
    }

    pub fn elapsed(&self) -> Option<Duration> {
        if let Some(d) = self.replay_elapsed_override {
            return Some(d);
        }
        if self.is_processing() {
            return self
                .visible_turn_started
                .or(self.processing_started)
                .map(|t| t.elapsed());
        }
        self.split_launch_in_flight()
            .then(|| self.pending_split_started_at.map(|t| t.elapsed()))
            .flatten()
    }

    pub(super) fn display_turn_duration_secs(&self) -> Option<f32> {
        self.visible_turn_started
            .or(self.processing_started)
            .map(|started| started.elapsed().as_secs_f32())
    }

    pub(super) fn clear_visible_turn_started(&mut self) {
        self.visible_turn_started = None;
    }

    pub fn provider_name(&self) -> &str {
        self.provider.name()
    }

    pub fn provider_model(&self) -> String {
        self.provider.model()
    }

    /// Get the upstream provider (e.g., which provider OpenRouter routed to)
    pub fn upstream_provider(&self) -> Option<&str> {
        self.upstream_provider.as_deref()
    }

    pub fn mcp_servers(&self) -> Vec<(String, usize)> {
        self.mcp_server_names.clone()
    }

    /// Scroll to the previous user prompt (scroll up - earlier in conversation)
    pub fn scroll_to_prev_prompt(&mut self) {
        let positions = ui::last_user_prompt_positions();
        if positions.is_empty() {
            return;
        }

        let current = self.scroll_offset;

        // positions are in document order (top to bottom).
        // Find the last position that is strictly less than current (i.e. earlier/above).
        // If we're at the bottom (!auto_scroll_paused), treat current as past-the-end.
        if !self.auto_scroll_paused {
            // Jump to the most recent (last) prompt
            if let Some(&pos) = positions.last() {
                self.scroll_offset = pos;
                self.auto_scroll_paused = true;
            }
            return;
        }

        let mut target = None;
        for &pos in positions.iter().rev() {
            if pos < current {
                target = Some(pos);
                break;
            }
        }

        if let Some(pos) = target {
            self.scroll_offset = pos;
        }
        // If no prompt above, stay where we are
    }

    /// Scroll to the next user prompt (scroll down - later in conversation)
    pub fn scroll_to_next_prompt(&mut self) {
        let positions = ui::last_user_prompt_positions();
        if positions.is_empty() || !self.auto_scroll_paused {
            return;
        }

        let current = self.scroll_offset;

        // Find the first position strictly greater than current (i.e. later/below).
        for &pos in &positions {
            if pos > current {
                self.scroll_offset = pos;
                return;
            }
        }

        // No more prompts below - go to bottom
        self.follow_chat_bottom();
    }

    /// Scroll to Nth most-recent user prompt (1 = most recent, 2 = second most recent, etc.).
    /// Uses actual wrapped line positions from the last render frame for accurate placement,
    /// positioning the prompt at the top of the viewport.
    pub(super) fn scroll_to_recent_prompt_rank(&mut self, rank: usize) {
        let rank = rank.max(1);
        let positions = ui::last_user_prompt_positions();
        let max_scroll = ui::last_max_scroll();

        if positions.is_empty() {
            return;
        }

        // positions are in document order (top to bottom), we want most-recent first
        let target_idx = positions.len().saturating_sub(rank);
        let target_line = positions[target_idx];
        self.set_status_notice(format!(
            "Ctrl+{}: idx={}/{} line={} max={}",
            rank,
            target_idx,
            positions.len(),
            target_line,
            max_scroll
        ));
        self.scroll_offset = target_line;
        self.auto_scroll_paused = true;
    }

    pub(super) fn toggle_input_stash(&mut self) {
        self.reset_input_history_browse(); // Prevent stash from interacting with history browsing
        if let Some((stashed, stashed_cursor)) = self.stashed_input.take() {
            let current_input = std::mem::replace(&mut self.input, stashed);
            let current_cursor = std::mem::replace(&mut self.cursor_pos, stashed_cursor);
            if current_input.is_empty() {
                self.set_status_notice("📋 Input restored from stash");
            } else {
                self.stashed_input = Some((current_input, current_cursor));
                self.set_status_notice("📋 Swapped input with stash");
            }
        } else if !self.input.is_empty() {
            let input = std::mem::take(&mut self.input);
            let cursor = std::mem::replace(&mut self.cursor_pos, 0);
            self.stashed_input = Some((input, cursor));
            self.set_status_notice("📋 Input stashed");
        }
    }

    /// Push a submitted input into history (called from `submit_input`).
    pub(super) fn push_input_history(&mut self, text: String) {
        let trimmed = text.trim().to_string();
        if trimmed.is_empty() {
            return;
        }
        // Avoid consecutive duplicates
        if self.input_history.last() == Some(&trimmed) {
            return;
        }
        // Dedup: if the same text already exists, remove it first so the latest
        // position wins (most-recently-used ordering).
        if let Some(existing) = self.input_history.iter().position(|e| e == &trimmed) {
            self.input_history.remove(existing);
        }
        self.input_history.push(trimmed);
        let max = crate::config::config().input_history.max_entries;
        if self.input_history.len() > max {
            self.input_history.remove(0);
        }
        self.save_input_history();
    }

    /// Navigate up (older) in input history. Returns `true` if the input was modified.
    pub(super) fn input_history_up(&mut self) -> bool {
        if self.input_history.is_empty() {
            return false;
        }
        let new_idx = match self.input_history_index {
            Some(idx) => idx.saturating_sub(1),
            None => {
                // Save the current input before entering history browse mode
                // so Down-past-end can restore it.
                self.input_history_pre_browse = Some((self.input.clone(), self.cursor_pos));
                self.input_history.len() - 1
            }
        };
        self.input_history_index = Some(new_idx);
        self.input = self.input_history[new_idx].clone();
        self.cursor_pos = self.input.len();
        true
    }

    /// Navigate down (newer) in input history. Returns `true` if the input was modified.
    pub(super) fn input_history_down(&mut self) -> bool {
        let Some(idx) = self.input_history_index else {
            return false;
        };
        let next = idx + 1;
        if next < self.input_history.len() {
            self.input_history_index = Some(next);
            self.input = self.input_history[next].clone();
            self.cursor_pos = self.input.len();
        } else {
            // Past the end: restore pre-browse input and exit history browsing
            self.input_history_index = None;
            if let Some((saved, saved_cursor)) = self.input_history_pre_browse.take() {
                self.input = saved;
                self.cursor_pos = saved_cursor;
            } else {
                self.input.clear();
                self.cursor_pos = 0;
            }
        }
        true
    }

    /// Reset history browsing state (call when the user manually edits input).
    pub(super) fn reset_input_history_browse(&mut self) {
        self.input_history_index = None;
        self.input_history_pre_browse = None;
        self.input_history_search = None;
    }

    /// Start a Ctrl+R reverse incremental search.
    pub(super) fn start_input_history_search(&mut self) {
        if self.input_history.is_empty() {
            self.set_status_notice("No input history to search");
            return;
        }
        self.reset_input_history_browse();
        self.input_history_search = Some(super::HistorySearchState {
            query: String::new(),
            match_index: None,
            saved_input: self.input.clone(),
            saved_cursor: self.cursor_pos,
        });
    }

    /// Append a character to the search query.
    pub(super) fn input_history_search_char(&mut self, c: char) {
        let Some(ref mut search) = self.input_history_search else {
            return;
        };
        search.query.push(c);
        self.input_history_search_find_match();
    }

    /// Delete the last character from the search query.
    pub(super) fn input_history_search_backspace(&mut self) {
        let Some(ref mut search) = self.input_history_search else {
            return;
        };
        search.query.pop();
        self.input_history_search_find_match();
    }

    /// Cycle to the next older match (Ctrl+R again while searching).
    pub(super) fn input_history_search_next(&mut self) {
        let Some(ref mut search) = self.input_history_search else {
            return;
        };
        if search.query.is_empty() {
            return;
        }
        let query_lower = search.query.to_lowercase();
        // Start searching from one before the current match
        let start = match search.match_index {
            Some(idx) => idx.saturating_sub(1),
            None => self.input_history.len().saturating_sub(1),
        };
        for i in (0..=start).rev() {
            if self.input_history[i].to_lowercase().contains(&query_lower) {
                search.match_index = Some(i);
                self.input = self.input_history[i].clone();
                self.cursor_pos = self.input.len();
                return;
            }
        }
        // No older match found; keep current match
    }

    /// Accept the current search result (Enter).
    pub(super) fn accept_input_history_search(&mut self) {
        let Some(search) = self.input_history_search.take() else {
            return;
        };
        if let Some(idx) = search.match_index {
            // Save undo state: capture the *original* pre-search input. find_match has
            // already overwritten self.input, so temporarily restore the original.
            if search.saved_input != self.input_history[idx] {
                let matched_input = self.input.clone();
                let matched_cursor = self.cursor_pos;
                self.input = search.saved_input;
                self.cursor_pos = search.saved_cursor;
                self.remember_input_undo_state();
                self.input = matched_input;
                self.cursor_pos = matched_cursor;
            }
            self.input = self.input_history[idx].clone();
            self.cursor_pos = self.input.len();
            self.input_history_index = Some(idx);
            self.reset_tab_completion();
            self.sync_model_picker_preview_from_input();
        }
        // If no match, leave input as-is (cleared by find_match during search)
    }

    /// Cancel the search and restore original input (Esc with no match).
    pub(super) fn cancel_input_history_search(&mut self) {
        if let Some(search) = self.input_history_search.take() {
            if search.match_index.is_none() {
                self.input = search.saved_input;
                self.cursor_pos = search.saved_cursor;
                self.reset_tab_completion();
                self.sync_model_picker_preview_from_input();
            } else if let Some(idx) = search.match_index {
                // Esc with match: accept the match (input was already set by find_match)
                // Save undo state with the original pre-search input
                if search.saved_input != self.input_history[idx] {
                    let matched_input = self.input.clone();
                    let matched_cursor = self.cursor_pos;
                    self.input = search.saved_input;
                    self.cursor_pos = search.saved_cursor;
                    self.remember_input_undo_state();
                    self.input = matched_input;
                    self.cursor_pos = matched_cursor;
                }
                self.input_history_index = Some(idx);
                self.reset_tab_completion();
                self.sync_model_picker_preview_from_input();
            }
        }
    }

    /// Internal: find the most recent match for the current query.
    fn input_history_search_find_match(&mut self) {
        let Some(ref mut search) = self.input_history_search else {
            return;
        };
        if search.query.is_empty() {
            search.match_index = None;
            self.input = search.saved_input.clone();
            self.cursor_pos = search.saved_cursor;
            return;
        }
        let query_lower = search.query.to_lowercase();
        // Search backwards from end (or from current match position to avoid jumping)
        let start = match search.match_index {
            Some(idx)
                if self.input_history[idx]
                    .to_lowercase()
                    .contains(&query_lower) =>
            {
                idx
            }
            _ => self.input_history.len().saturating_sub(1),
        };
        for i in (0..=start).rev() {
            if self.input_history[i].to_lowercase().contains(&query_lower) {
                search.match_index = Some(i);
                self.input = self.input_history[i].clone();
                self.cursor_pos = self.input.len();
                return;
            }
        }
        search.match_index = None;
        self.input.clear();
        self.cursor_pos = 0;
    }

    /// Clear all input history entries.
    pub(super) fn clear_input_history(&mut self) {
        self.input_history.clear();
        self.reset_input_history_browse();
        self.save_input_history();
    }

    /// Delete a single input history entry by 0-based index.
    pub(super) fn delete_input_history_entry(&mut self, idx: usize) -> bool {
        if idx >= self.input_history.len() {
            return false;
        }
        self.input_history.remove(idx);
        // Reset browse if we deleted the entry being browsed or one before it
        if let Some(browse_idx) = self.input_history_index {
            if browse_idx == idx {
                self.reset_input_history_browse();
            } else if browse_idx > idx {
                self.input_history_index = Some(browse_idx - 1);
            }
        }
        self.save_input_history();
        true
    }

    /// Path to the global input-history file.
    fn input_history_path() -> Option<std::path::PathBuf> {
        crate::storage::jcode_dir()
            .ok()
            .map(|dir| dir.join("input-history.json"))
    }

    /// Save input history to disk (global, not session-specific).
    pub(super) fn save_input_history(&self) {
        if let Some(path) = Self::input_history_path() {
            if self.input_history.is_empty() {
                // Remove the file so cleared history doesn't reappear on restart.
                let _ = std::fs::remove_file(&path);
                return;
            }
            let data = serde_json::json!({
                "history": self.input_history,
                "version": 1,
            });
            let _ = std::fs::write(&path, data.to_string());
        }
    }

    /// Load input history from disk. Returns entries if available.
    pub(super) fn load_input_history() -> Vec<String> {
        let Some(path) = Self::input_history_path() else {
            return Vec::new();
        };
        let Ok(contents) = std::fs::read_to_string(&path) else {
            return Vec::new();
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&contents) else {
            return Vec::new();
        };
        let Some(arr) = value.get("history").and_then(|v| v.as_array()) else {
            return Vec::new();
        };
        let max = crate::config::config().input_history.max_entries;
        arr.iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .take(max)
            .collect()
    }
}
