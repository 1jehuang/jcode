use super::*;
use crate::tui::{TuiState, backend, core, is_unexpected_cache_miss, ui};

pub(super) struct RestoredReloadInput {
    pub input: String,
    pub cursor: usize,
    pub queued_messages: Vec<String>,
    pub hidden_queued_system_messages: Vec<String>,
    pub startup_status_notice: Option<String>,
    pub startup_display_message: Option<(String, String)>,
    pub interleave_message: Option<String>,
    pub pending_soft_interrupts: Vec<String>,
    pub rate_limit_pending_message: Option<super::PendingRemoteMessage>,
    pub rate_limit_reset: Option<Instant>,
    pub observe_mode_enabled: bool,
    pub observe_page_markdown: String,
    pub observe_page_updated_at_ms: u64,
}

fn infer_spawned_session_startup_hints(message: &str) -> Option<(String, (String, String))> {
    let label = if message.starts_with("You are the automatic reviewer for parent session `") {
        "Autoreview"
    } else if message.starts_with("You are the automatic judge for parent session `") {
        "Autojudge"
    } else if message.starts_with("You are the one-shot reviewer for parent session `") {
        "Review"
    } else if message.starts_with("You are the one-shot judge for parent session `") {
        "Judge"
    } else {
        return None;
    };

    let parent_session_id = message.split('`').nth(1).unwrap_or("parent");
    let body = if label == "Autojudge" {
        format!(
            "🔍 {} session started for parent `{}`.\n\nThis session is analysis-only: it will inspect the recent work, send exactly one DM back telling the parent either to `CONTINUE:` with specific next steps or `STOP:` because the work is complete, and then stop. It should not continue the work or modify repo state.\n\nJudge sessions use a user-visible mirror of the parent conversation: user prompts, visible assistant replies, and shallow tool-call summaries — not the parent's full hidden tool context.",
            label, parent_session_id
        )
    } else {
        format!(
            "🔍 {} session started for parent `{}`.\n\nThis session is analysis-only: it will inspect the recent work, send exactly one DM back to the parent session, and stop. It should not continue the work or modify repo state.\n\nJudge sessions use a user-visible mirror of the parent conversation: user prompts, visible assistant replies, and shallow tool-call summaries — not the parent's full hidden tool context.",
            label, parent_session_id
        )
    };

    Some((format!("{} starting", label), (label.to_string(), body)))
}

impl App {
    pub(super) fn active_client_session_id(&self) -> Option<&str> {
        if self.is_remote {
            self.remote_session_id.as_deref()
        } else {
            Some(self.session.id.as_str())
        }
    }

    fn client_maintenance_busy_message(
        current: crate::bus::ClientMaintenanceAction,
        requested: crate::bus::ClientMaintenanceAction,
    ) -> String {
        if current == requested {
            format!("{} already running in the background.", current.title())
        } else {
            format!(
                "{} already running in the background. Wait for it to finish before starting {}.",
                current.title(),
                requested.noun()
            )
        }
    }

    fn client_maintenance_card_title(action: crate::bus::ClientMaintenanceAction) -> String {
        action.title().to_string()
    }

    fn client_maintenance_card_message(
        action: crate::bus::ClientMaintenanceAction,
        status: impl Into<String>,
        note: impl Into<String>,
    ) -> String {
        let note = note.into();
        let mut content = format!("**Status:** {}", status.into());
        if !note.is_empty() {
            content.push_str("\n\n");
            content.push_str(&note);
        }
        if action == crate::bus::ClientMaintenanceAction::Rebuild {
            content.push_str(
                "\n\n**Pipeline:** `git pull --ff-only` → `cargo build --release` → `cargo test --release -- --test-threads=1`",
            );
        }
        content
    }

    fn set_client_maintenance_message(
        &mut self,
        action: crate::bus::ClientMaintenanceAction,
        content: String,
    ) {
        let title = Self::client_maintenance_card_title(action);
        if let Some(idx) = self
            .display_messages
            .iter()
            .rposition(|message| Self::is_client_maintenance_message(message, &title))
        {
            let message = &mut self.display_messages[idx];
            let title_changed = message.title.as_deref() != Some(title.as_str());
            if title_changed {
                message.title = Some(title);
            }
            if message.content != content || title_changed {
                message.content = content;
                self.bump_display_messages_version();
            }
        } else {
            self.push_display_message(DisplayMessage::system(content).with_title(title));
        }
    }

    pub(super) fn start_background_client_rebuild(&mut self, session_id: String) {
        self.start_background_client_maintenance(
            crate::bus::ClientMaintenanceAction::Rebuild,
            session_id,
        );
    }

    pub(super) fn start_background_client_update(&mut self, session_id: String) {
        self.start_background_client_maintenance(
            crate::bus::ClientMaintenanceAction::Update,
            session_id,
        );
    }

    fn start_background_client_maintenance(
        &mut self,
        action: crate::bus::ClientMaintenanceAction,
        session_id: String,
    ) {
        if let Some(current) = self.background_client_action {
            let message = Self::client_maintenance_busy_message(current, action);
            self.set_status_notice(&message);
            self.set_client_maintenance_message(
                current,
                Self::client_maintenance_card_message(current, "already running", message),
            );
            return;
        }

        self.background_client_action = Some(action);
        self.pending_background_client_reload = None;

        match action {
            crate::bus::ClientMaintenanceAction::Update => {
                self.set_status_notice("Checking for updates...");
                self.set_client_maintenance_message(
                    action,
                    Self::client_maintenance_card_message(
                        action,
                        "checking for updates",
                        "Running in the background. jcode will reload automatically when the update is ready.",
                    ),
                );
                crate::update::spawn_background_session_update(session_id);
            }
            crate::bus::ClientMaintenanceAction::Rebuild => {
                self.set_status_notice("Starting background rebuild...");
                self.set_client_maintenance_message(
                    action,
                    Self::client_maintenance_card_message(
                        action,
                        "starting background rebuild",
                        "Running in the background. jcode will reload automatically after the rebuild succeeds.",
                    ),
                );
                crate::cli::hot_exec::spawn_background_session_rebuild(session_id);
            }
        }
    }

    pub(super) fn maybe_finish_background_client_reload(&mut self) {
        if self.is_processing {
            return;
        }

        let Some((session_id, action)) = self.pending_background_client_reload.take() else {
            return;
        };

        self.set_client_maintenance_message(
            action,
            Self::client_maintenance_card_message(
                action,
                "reloading client",
                "The new binary is ready, so jcode is switching over now.",
            ),
        );
        self.save_input_for_reload(&session_id);
        self.reload_requested = Some(session_id);
        self.should_quit = true;
    }

    pub(super) fn handle_session_update_status(&mut self, status: crate::bus::SessionUpdateStatus) {
        use crate::bus::{ClientMaintenanceAction, SessionUpdateStatus};

        let Some(active_session_id) = self.active_client_session_id().map(str::to_string) else {
            return;
        };

        match status {
            SessionUpdateStatus::Status {
                session_id,
                action,
                message,
            } => {
                if session_id != active_session_id {
                    return;
                }
                self.background_client_action = Some(action);
                self.set_status_notice(message.clone());
                self.set_client_maintenance_message(
                    action,
                    Self::client_maintenance_card_message(
                        action,
                        message,
                        "Still running in the background. jcode will reload automatically when ready.",
                    ),
                );
            }
            SessionUpdateStatus::NoUpdate {
                session_id,
                current,
            } => {
                if session_id != active_session_id {
                    return;
                }
                self.background_client_action = None;
                self.pending_background_client_reload = None;
                let message = format!("Already up to date ({})", current);
                self.set_status_notice(&message);
                self.set_client_maintenance_message(
                    ClientMaintenanceAction::Update,
                    Self::client_maintenance_card_message(
                        ClientMaintenanceAction::Update,
                        "already up to date",
                        format!("Current version: `{}`", current),
                    ),
                );
            }
            SessionUpdateStatus::ReadyToReload {
                session_id,
                action,
                version,
            } => {
                if session_id != active_session_id {
                    return;
                }
                self.background_client_action = None;
                let ready_message = match action {
                    ClientMaintenanceAction::Update => format!("✅ Updated to {}.", version),
                    ClientMaintenanceAction::Rebuild => {
                        format!("✅ Rebuild finished ({}).", version)
                    }
                };
                if self.is_processing {
                    self.pending_background_client_reload = Some((session_id, action));
                    self.set_status_notice(format!(
                        "{} ready — will reload after the current turn",
                        action.title()
                    ));
                    self.set_client_maintenance_message(
                        action,
                        Self::client_maintenance_card_message(
                            action,
                            ready_message,
                            "Waiting for the current turn to finish before reloading.",
                        ),
                    );
                    return;
                }

                self.set_client_maintenance_message(
                    action,
                    Self::client_maintenance_card_message(action, ready_message, "Reloading now."),
                );
                self.pending_background_client_reload = Some((session_id, action));
                self.maybe_finish_background_client_reload();
            }
            SessionUpdateStatus::Error {
                session_id,
                action,
                message,
            } => {
                if session_id != active_session_id {
                    return;
                }
                self.background_client_action = None;
                self.pending_background_client_reload = None;
                self.set_status_notice(format!("{} failed", action.title()));
                self.set_client_maintenance_message(
                    action,
                    Self::client_maintenance_card_message(action, "failed", message.clone()),
                );
                self.push_display_message(DisplayMessage::error(message));
            }
        }
    }

    pub(super) fn note_client_focus(&self) {
        if let Some(session_id) = self.active_client_session_id() {
            let _ = crate::dictation::remember_last_focused_session(session_id);
        }
    }

    pub fn display_messages(&self) -> &[DisplayMessage] {
        &self.display_messages
    }

    pub(super) fn bump_display_messages_version(&mut self) {
        self.display_messages_version = self.display_messages_version.wrapping_add(1);
    }

    pub fn push_display_message(&mut self, message: DisplayMessage) {
        if self.try_coalesce_repeated_display_message(&message) {
            return;
        }
        let is_tool = message.role == "tool";
        self.display_messages.push(message);
        self.bump_display_messages_version();
        if is_tool && self.diff_mode.has_side_pane() && self.diff_pane_auto_scroll {
            self.diff_pane_scroll = usize::MAX;
        }
    }

    pub(super) fn replace_display_message_content(&mut self, idx: usize, content: String) -> bool {
        if let Some(message) = self.display_messages.get_mut(idx) {
            if message.content != content {
                message.content = content;
                self.bump_display_messages_version();
            }
            true
        } else {
            false
        }
    }

    pub(super) fn replace_display_message_title_and_content(
        &mut self,
        idx: usize,
        title: Option<String>,
        content: String,
    ) -> bool {
        if let Some(message) = self.display_messages.get_mut(idx) {
            if message.title != title || message.content != content {
                message.title = title;
                message.content = content;
                self.bump_display_messages_version();
            }
            true
        } else {
            false
        }
    }

    pub(super) fn replace_latest_tool_display_message(
        &mut self,
        tool_call_id: &str,
        title: Option<String>,
        content: String,
    ) -> bool {
        let Some(idx) = self.display_messages.iter().rposition(|message| {
            message.tool_data.as_ref().map(|tool| tool.id.as_str()) == Some(tool_call_id)
        }) else {
            return false;
        };

        self.replace_display_message_title_and_content(idx, title, content)
    }

    pub(super) fn remove_display_message(&mut self, idx: usize) -> Option<DisplayMessage> {
        if idx < self.display_messages.len() {
            let removed = self.display_messages.remove(idx);
            self.bump_display_messages_version();
            Some(removed)
        } else {
            None
        }
    }

    pub(super) fn append_reload_message(&mut self, line: &str) {
        if let Some(idx) = self
            .display_messages
            .iter()
            .rposition(Self::is_reload_message)
        {
            let msg = &mut self.display_messages[idx];
            if !msg.content.is_empty() {
                msg.content.push('\n');
            }
            msg.content.push_str(line);
            msg.title = Some("Reload".to_string());
            self.bump_display_messages_version();
        } else {
            self.push_display_message(
                DisplayMessage::system(line.to_string()).with_title("Reload"),
            );
        }
    }

    pub(super) fn is_client_maintenance_message(message: &DisplayMessage, title: &str) -> bool {
        message.role == "system" && message.title.as_deref() == Some(title)
    }

    pub(super) fn is_reload_message(message: &DisplayMessage) -> bool {
        message.role == "system"
            && message
                .title
                .as_deref()
                .is_some_and(|title| title == "Reload" || title.starts_with("Reload: "))
    }

    fn try_coalesce_repeated_display_message(&mut self, message: &DisplayMessage) -> bool {
        if !Self::is_repeat_compactable_display_message(message) {
            return false;
        }

        let Some(last) = self.display_messages.last_mut() else {
            return false;
        };
        if !Self::is_repeat_compactable_display_message(last) {
            return false;
        }

        let (last_base, last_count) = Self::split_repeat_suffix(&last.content);
        if last.role != message.role
            || last.title != message.title
            || last.tool_calls != message.tool_calls
            || last.duration_secs != message.duration_secs
            || last_base != message.content
        {
            return false;
        }

        let next_count = last_count.saturating_add(1);
        last.content = Self::format_repeated_display_content(message.content.as_str(), next_count);
        self.bump_display_messages_version();
        true
    }

    fn is_repeat_compactable_display_message(message: &DisplayMessage) -> bool {
        matches!(message.role.as_str(), "system" | "error")
            && message.title.is_none()
            && message.tool_calls.is_empty()
            && message.tool_data.is_none()
            && message.duration_secs.is_none()
            && !message.content.contains(['\n', '\r'])
    }

    fn split_repeat_suffix(content: &str) -> (&str, u32) {
        const REPEAT_PREFIX: &str = " [×";

        let Some(prefix_idx) = content.rfind(REPEAT_PREFIX) else {
            return (content, 1);
        };
        if !content.ends_with(']') {
            return (content, 1);
        }

        let digits = &content[prefix_idx + REPEAT_PREFIX.len()..content.len() - 1];
        if digits.is_empty() || !digits.chars().all(|ch| ch.is_ascii_digit()) {
            return (content, 1);
        }

        match digits.parse::<u32>() {
            Ok(count) if count >= 2 => (&content[..prefix_idx], count),
            _ => (content, 1),
        }
    }

    fn format_repeated_display_content(content: &str, repeat_count: u32) -> String {
        if repeat_count <= 1 {
            content.to_string()
        } else {
            format!("{content} [×{repeat_count}]")
        }
    }

    pub(super) fn clear_display_messages(&mut self) {
        if !self.display_messages.is_empty() {
            self.display_messages.clear();
            self.bump_display_messages_version();
        }
    }

    /// Find word boundary going backward (for Ctrl+W, Alt+B)
    pub(super) fn find_word_boundary_back(&self) -> usize {
        if self.cursor_pos == 0 {
            return 0;
        }
        let mut pos = self.cursor_pos;

        // Move back one char
        pos = core::prev_char_boundary(&self.input, pos);

        // Skip trailing whitespace
        while pos > 0 {
            let ch = self.input[pos..].chars().next().unwrap_or(' ');
            if !ch.is_whitespace() {
                break;
            }
            pos = core::prev_char_boundary(&self.input, pos);
        }

        // Skip word characters
        while pos > 0 {
            let prev = core::prev_char_boundary(&self.input, pos);
            let ch = self.input[prev..].chars().next().unwrap_or(' ');
            if ch.is_whitespace() {
                break;
            }
            pos = prev;
        }

        pos
    }

    /// Find word boundary going forward (for Alt+F, Alt+D)
    pub(super) fn find_word_boundary_forward(&self) -> usize {
        let len = self.input.len();
        if self.cursor_pos >= len {
            return len;
        }
        let mut pos = self.cursor_pos;

        // Skip current word
        while pos < len {
            let ch = self.input[pos..].chars().next().unwrap_or(' ');
            if ch.is_whitespace() {
                break;
            }
            pos = core::next_char_boundary(&self.input, pos);
        }

        // Skip whitespace
        while pos < len {
            let ch = self.input[pos..].chars().next().unwrap_or(' ');
            if !ch.is_whitespace() {
                break;
            }
            pos = core::next_char_boundary(&self.input, pos);
        }

        pos
    }

    pub fn input(&self) -> &str {
        &self.input
    }

    #[cfg(test)]
    pub(crate) fn set_queue_mode_for_test(&mut self, enabled: bool) {
        self.queue_mode = enabled;
    }

    #[cfg(test)]
    pub(crate) fn set_diff_mode_for_test(&mut self, mode: crate::config::DiffDisplayMode) {
        self.diff_mode = mode;
    }

    #[cfg(test)]
    pub(crate) fn set_input_for_test(&mut self, input: impl Into<String>) {
        self.input = input.into();
        self.cursor_pos = self.input.len();
    }

    pub(super) fn fuzzy_score(needle: &str, haystack: &str) -> Option<usize> {
        if needle.is_empty() {
            return Some(0);
        }
        // Both needle and haystack should start with '/', match from char 1 onward
        let n = needle.strip_prefix('/').unwrap_or(needle);
        let h = haystack.strip_prefix('/').unwrap_or(haystack);
        if n.is_empty() {
            return Some(0);
        }
        // First char of the command (after /) must match
        if let Some(first_char) = n.chars().next() {
            if !h.starts_with(&n[..first_char.len_utf8()]) {
                return None;
            }
        }
        let mut score = 0usize;
        let mut pos = 0usize;
        for ch in n.chars() {
            let Some(idx) = h[pos..].find(ch) else {
                return None;
            };
            score += idx;
            pos += idx + ch.len_utf8();
        }
        // Penalize large gaps - reject if average gap is too big
        if n.len() > 1 && score > n.len() * 3 {
            return None;
        }
        Some(score)
    }

    pub(super) fn rank_suggestions(
        &self,
        needle: &str,
        candidates: Vec<(String, &'static str)>,
    ) -> Vec<(String, &'static str)> {
        let needle = needle.to_lowercase();
        let mut scored: Vec<(bool, usize, String, &'static str)> = Vec::new();
        for (cmd, help) in candidates {
            let lower = cmd.to_lowercase();
            if lower.starts_with(&needle) {
                scored.push((true, 0, cmd, help));
            } else if let Some(score) = Self::fuzzy_score(&needle, &lower) {
                scored.push((false, score, cmd, help));
            }
        }
        scored.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| a.1.cmp(&b.1))
                .then_with(|| a.2.len().cmp(&b.2.len()))
                .then_with(|| a.2.cmp(&b.2))
        });
        scored
            .into_iter()
            .map(|(_, _, cmd, help)| (cmd, help))
            .collect()
    }

    fn command_candidates(&self) -> Vec<(String, &'static str)> {
        let mut commands: Vec<(String, &'static str)> = vec![
            ("/help".into(), "Show help and keyboard shortcuts"),
            ("/?".into(), "Alias for /help"),
            ("/commands".into(), "Alias for /help"),
            ("/model".into(), "List or switch models"),
            ("/agents".into(), "Configure models for agent roles"),
            ("/subagent".into(), "Launch a subagent manually"),
            (
                "/observe".into(),
                "Show the latest tool context in the side panel",
            ),
            ("/btw".into(), "Ask a side question in the side panel"),
            (
                "/subagent-model".into(),
                "Show/change subagent model policy",
            ),
            (
                "/autoreview".into(),
                "Show/toggle automatic end-of-turn review",
            ),
            (
                "/autojudge".into(),
                "Show/toggle automatic end-of-turn judging",
            ),
            ("/review".into(), "Launch a one-shot headed review session"),
            ("/judge".into(), "Launch a one-shot headed judge session"),
            ("/effort".into(), "Show/change reasoning effort (Alt+←/→)"),
            ("/fast".into(), "Toggle OpenAI/Codex fast mode"),
            (
                "/transport".into(),
                "Show/change connection transport (auto/https/websocket)",
            ),
            (
                "/alignment".into(),
                "Show/change default text alignment (centered/left)",
            ),
            ("/clear".into(), "Clear conversation history"),
            ("/rewind".into(), "Rewind conversation to previous message"),
            ("/poke".into(), "Poke model to resume with incomplete todos"),
            (
                "/improve".into(),
                "Autonomously find and implement highest-leverage improvements",
            ),
            (
                "/refactor".into(),
                "Run a safe refactor loop with independent review",
            ),
            (
                "/compact".into(),
                "Compact context (summarize old messages)",
            ),
            ("/fix".into(), "Recover when the model cannot continue"),
            (
                "/dictate".into(),
                "Run configured external dictation command",
            ),
            ("/memory".into(), "Toggle memory feature (on/off/status)"),
            (
                "/goals".into(),
                "Open goals overview / resume tracked goals",
            ),
            ("/swarm".into(), "Toggle swarm feature (on/off/status)"),
            ("/context".into(), "Show the full session context snapshot"),
            ("/version".into(), "Show current version"),
            ("/changelog".into(), "Show recent changes in this build"),
            ("/info".into(), "Show session info and tokens"),
            ("/usage".into(), "Show connected provider usage limits"),
            (
                "/subscription".into(),
                "Show jcode subscription status and account details",
            ),
            ("/config".into(), "Show or edit configuration"),
            ("/reload".into(), "Reload into newest available binary"),
            ("/restart".into(), "Restart with current binary (no build)"),
            (
                "/rebuild".into(),
                "Background rebuild + auto reload when ready",
            ),
            ("/selfdev".into(), "Open a new self-dev jcode session"),
            (
                "/update".into(),
                "Background update + auto reload when ready",
            ),
            ("/resume".into(), "Open session picker"),
            (
                "/catchup".into(),
                "Open Catch Up picker for sessions needing attention",
            ),
            (
                "/back".into(),
                "Return to the previous session visited via Catch Up",
            ),
            ("/save".into(), "Bookmark session for easy access"),
            ("/unsave".into(), "Remove bookmark from session"),
            ("/split".into(), "Split session into a new window"),
            (
                "/workspace".into(),
                "Niri-style session workspace (status/on/off/add)",
            ),
            ("/quit".into(), "Exit jcode"),
            ("/auth".into(), "Show authentication status"),
            ("/cache".into(), "Toggle cache TTL between 5min and 1h"),
            (
                "/login".into(),
                "Login to a provider (use `/login <provider>` for the full list)",
            ),
            (
                "/account".into(),
                "Open the combined Claude/OpenAI account picker",
            ),
        ];

        if self.is_remote {
            commands.push(("/client-reload".into(), "Force reload client binary"));
            commands.push(("/server-reload".into(), "Force reload server binary"));
        }

        for skill in self.skills.list() {
            commands.push((format!("/{}", skill.name), "Activate skill"));
        }

        commands
    }

    fn model_suggestion_candidates(&self) -> Vec<(String, &'static str)> {
        fn push_unique(
            seen: &mut std::collections::HashSet<String>,
            entries: &mut Vec<String>,
            model: String,
        ) {
            if !model.is_empty() && seen.insert(model.clone()) {
                entries.push(model);
            }
        }

        let mut seen = std::collections::HashSet::new();
        let mut models = Vec::new();

        if self.is_remote {
            if let Some(current) = self.remote_provider_model.clone() {
                push_unique(&mut seen, &mut models, current);
            }

            let routes = if !self.remote_model_options.is_empty() {
                self.remote_model_options.clone()
            } else {
                self.build_remote_model_routes_fallback()
            };

            for route in routes {
                push_unique(&mut seen, &mut models, route.model);
            }

            for model in &self.remote_available_entries {
                push_unique(&mut seen, &mut models, model.clone());
            }
        } else {
            push_unique(&mut seen, &mut models, self.provider.model());
            for model in self.provider.available_models_display() {
                push_unique(&mut seen, &mut models, model);
            }
        }

        models
            .into_iter()
            .map(|model| (format!("/model {}", model), "Switch to model"))
            .collect()
    }

    /// Get command suggestions based on current input (or base input for cycling)
    pub(super) fn get_suggestions_for(&self, input: &str) -> Vec<(String, &'static str)> {
        let input = input.trim_start();

        // Only show suggestions when input starts with /
        if !input.starts_with('/') {
            return vec![];
        }

        let prefix = input.to_lowercase();
        let prefix_trimmed = prefix.trim_end();

        if prefix.starts_with("/model ") || prefix.starts_with("/models ") {
            let suggestions = self.model_suggestion_candidates();
            if suggestions.is_empty() {
                return vec![("/model".into(), "Open model picker")];
            }
            return self.rank_suggestions(input, suggestions);
        }

        if prefix.starts_with("/agents ") {
            return self.rank_suggestions(
                input,
                vec![
                    ("/agents swarm".into(), "Configure swarm/subagent model"),
                    ("/agents review".into(), "Configure code review model"),
                    ("/agents judge".into(), "Configure judge model"),
                    ("/agents memory".into(), "Configure memory sidecar model"),
                    ("/agents ambient".into(), "Configure ambient model"),
                ],
            );
        }

        if prefix.starts_with("/subagent-model ") {
            let mut suggestions = vec![
                (
                    "/subagent-model inherit".into(),
                    "Use the current active model",
                ),
                (
                    "/subagent-model show".into(),
                    "Show the current subagent model policy",
                ),
            ];
            suggestions.extend(
                self.model_suggestion_candidates()
                    .into_iter()
                    .map(|(cmd, _)| {
                        (
                            cmd.replacen("/model ", "/subagent-model ", 1),
                            "Pin this subagent model",
                        )
                    }),
            );
            return self.rank_suggestions(input, suggestions);
        }

        if prefix.starts_with("/autoreview ") {
            return self.rank_suggestions(
                input,
                vec![
                    (
                        "/autoreview status".into(),
                        "Show current autoreview status",
                    ),
                    ("/autoreview on".into(), "Enable end-of-turn autoreview"),
                    ("/autoreview off".into(), "Disable end-of-turn autoreview"),
                    ("/autoreview now".into(), "Launch a reviewer immediately"),
                ],
            );
        }

        if prefix_trimmed == "/autoreview" {
            return vec![
                (
                    "/autoreview status".into(),
                    "Show current autoreview status",
                ),
                ("/autoreview on".into(), "Enable end-of-turn autoreview"),
                ("/autoreview off".into(), "Disable end-of-turn autoreview"),
                ("/autoreview now".into(), "Launch a reviewer immediately"),
            ];
        }

        if prefix.starts_with("/autojudge ") {
            return self.rank_suggestions(
                input,
                vec![
                    ("/autojudge status".into(), "Show current autojudge status"),
                    ("/autojudge on".into(), "Enable end-of-turn autojudge"),
                    ("/autojudge off".into(), "Disable end-of-turn autojudge"),
                    ("/autojudge now".into(), "Launch a judge immediately"),
                ],
            );
        }

        if prefix_trimmed == "/autojudge" {
            return vec![
                ("/autojudge status".into(), "Show current autojudge status"),
                ("/autojudge on".into(), "Enable end-of-turn autojudge"),
                ("/autojudge off".into(), "Disable end-of-turn autojudge"),
                ("/autojudge now".into(), "Launch a judge immediately"),
            ];
        }

        if prefix.starts_with("/review ") {
            return self.rank_suggestions(
                input,
                vec![("/review".into(), "Launch a one-shot review immediately")],
            );
        }

        if prefix_trimmed == "/review" {
            return vec![("/review".into(), "Launch a one-shot review immediately")];
        }

        if prefix.starts_with("/judge ") {
            return self.rank_suggestions(
                input,
                vec![("/judge".into(), "Launch a one-shot judge immediately")],
            );
        }

        if prefix_trimmed == "/judge" {
            return vec![("/judge".into(), "Launch a one-shot judge immediately")];
        }

        if prefix_trimmed == "/subagent-model" {
            return vec![
                (
                    "/subagent-model show".into(),
                    "Show the current subagent model policy",
                ),
                (
                    "/subagent-model inherit".into(),
                    "Use the current active model",
                ),
            ];
        }

        if prefix.starts_with("/subagent ") {
            return self.rank_suggestions(
                input,
                vec![
                    (
                        "/subagent --type general ".into(),
                        "Launch a general-purpose subagent",
                    ),
                    (
                        "/subagent --model ".into(),
                        "Launch a subagent with an explicit model",
                    ),
                    (
                        "/subagent --continue ".into(),
                        "Resume an existing subagent session",
                    ),
                ],
            );
        }

        if prefix_trimmed == "/subagent" {
            return vec![("/subagent ".into(), "Launch a subagent with a prompt")];
        }

        // /model opens the interactive picker, and `/model <name>` supports direct completion.
        if prefix_trimmed == "/model" || prefix_trimmed == "/models" {
            return vec![("/model".into(), "Open model picker or type `/model <name>`")];
        }

        if prefix_trimmed == "/agents" {
            return vec![("/agents".into(), "Open agent model config picker")];
        }

        if prefix.starts_with("/help ") || prefix.starts_with("/? ") {
            let base = if prefix.starts_with("/? ") {
                "/?"
            } else {
                "/help"
            };
            let topics = self
                .command_candidates()
                .into_iter()
                .map(|(cmd, help)| (format!("{} {}", base, cmd.trim_start_matches('/')), help))
                .collect();
            return self.rank_suggestions(input, topics);
        }

        if prefix.starts_with("/effort ") {
            let efforts = ["none", "low", "medium", "high", "xhigh"];
            return self.rank_suggestions(
                input,
                efforts
                    .iter()
                    .map(|e| (format!("/effort {}", e), effort_display_label(e)))
                    .collect(),
            );
        }

        if prefix.starts_with("/fast ") {
            let modes = [
                "on",
                "off",
                "status",
                "default on",
                "default off",
                "default status",
            ];
            return self.rank_suggestions(
                input,
                modes.iter().map(|m| (format!("/fast {}", m), *m)).collect(),
            );
        }

        if prefix.starts_with("/transport ") {
            let transports = ["auto", "https", "websocket"];
            return self.rank_suggestions(
                input,
                transports
                    .iter()
                    .map(|t| (format!("/transport {}", t), *t))
                    .collect(),
            );
        }

        if prefix.starts_with("/compact ") {
            let suggestions = vec![
                ("/compact mode".into(), "Show/change compaction mode"),
                (
                    "/compact mode status".into(),
                    "Show the current compaction mode",
                ),
                ("/compact mode reactive".into(), "Use reactive compaction"),
                ("/compact mode proactive".into(), "Use proactive compaction"),
                ("/compact mode semantic".into(), "Use semantic compaction"),
            ];
            return self.rank_suggestions(input, suggestions);
        }

        if prefix.starts_with("/compact mode ") {
            let modes = ["reactive", "proactive", "semantic"];
            let mut suggestions: Vec<(String, &'static str)> = vec![(
                "/compact mode status".into(),
                "Show the current compaction mode",
            )];
            suggestions.extend(
                modes
                    .iter()
                    .map(|mode| (format!("/compact mode {}", mode), *mode)),
            );
            return self.rank_suggestions(input, suggestions);
        }

        if prefix.starts_with("/login ") || prefix.starts_with("/auth ") {
            let base = if prefix.starts_with("/auth ") {
                "/auth"
            } else {
                "/login"
            };
            let suggestions = crate::provider_catalog::tui_login_providers()
                .iter()
                .map(|provider| (format!("{} {}", base, provider.id), provider.menu_detail))
                .collect();
            return self.rank_suggestions(input, suggestions);
        }

        if prefix.starts_with("/account ") || prefix.starts_with("/accounts ") {
            let mut suggestions = vec![
                ("/account list".into(), "Open all provider/account actions"),
                ("/account switch".into(), "Switch active account by label"),
                (
                    "/account default-provider".into(),
                    "Set preferred default provider",
                ),
                (
                    "/account default-model".into(),
                    "Set preferred default model",
                ),
                (
                    "/account openai-compatible settings".into(),
                    "Inspect custom OpenAI-compatible settings",
                ),
                (
                    "/account openai-compatible api-base".into(),
                    "Set custom OpenAI-compatible API base",
                ),
            ];
            for provider in crate::provider_catalog::login_providers() {
                suggestions.push((
                    format!("/account {}", provider.id),
                    "Open this provider's account/settings actions",
                ));
                suggestions.push((
                    format!("/account {} settings", provider.id),
                    "Show provider-specific settings",
                ));
                suggestions.push((
                    format!("/account {} login", provider.id),
                    "Start or refresh login for this provider",
                ));
            }
            suggestions.push(("/account claude add".into(), "Add a new Claude account"));
            suggestions.push(("/account openai add".into(), "Add a new OpenAI account"));
            suggestions.push((
                "/account openai transport".into(),
                "Set OpenAI transport preference",
            ));
            suggestions.push((
                "/account openai effort".into(),
                "Set OpenAI reasoning effort preference",
            ));
            if let Ok(accounts) = crate::auth::claude::list_accounts() {
                for account in accounts {
                    suggestions.push((
                        format!("/account claude switch {}", account.label),
                        "Switch to this Claude account",
                    ));
                }
            }
            if let Ok(accounts) = crate::auth::codex::list_accounts() {
                for account in accounts {
                    suggestions.push((
                        format!("/account openai switch {}", account.label),
                        "Switch to this OpenAI account",
                    ));
                }
            }
            return self.rank_suggestions(input, suggestions);
        }

        if prefix.starts_with("/memory ") {
            return self.rank_suggestions(
                input,
                vec![
                    ("/memory on".into(), "Enable memory for this session"),
                    ("/memory off".into(), "Disable memory for this session"),
                    ("/memory status".into(), "Show memory feature status"),
                ],
            );
        }

        if prefix.starts_with("/improve ") {
            return self.rank_suggestions(
                input,
                vec![
                    (
                        "/improve plan".into(),
                        "Generate a ranked improve todo list without editing",
                    ),
                    (
                        "/improve resume".into(),
                        "Resume the last saved improve mode for this session",
                    ),
                    (
                        "/improve status".into(),
                        "Show current improve batch and inferred status",
                    ),
                    (
                        "/improve stop".into(),
                        "Stop improvement mode after the next safe point",
                    ),
                ],
            );
        }

        if prefix.starts_with("/refactor ") {
            return self.rank_suggestions(
                input,
                vec![
                    (
                        "/refactor plan".into(),
                        "Generate a ranked refactor todo list without editing",
                    ),
                    (
                        "/refactor resume".into(),
                        "Resume the last saved refactor mode for this session",
                    ),
                    (
                        "/refactor status".into(),
                        "Show current refactor batch and inferred status",
                    ),
                    (
                        "/refactor stop".into(),
                        "Stop refactor mode after the next safe point",
                    ),
                ],
            );
        }

        if prefix.starts_with("/swarm ") {
            return self.rank_suggestions(
                input,
                vec![
                    ("/swarm on".into(), "Enable swarm for this session"),
                    ("/swarm off".into(), "Disable swarm for this session"),
                    ("/swarm status".into(), "Show swarm feature status"),
                ],
            );
        }

        if prefix.starts_with("/subscription ") {
            return self.rank_suggestions(
                input,
                vec![("/subscription status".into(), "Show subscription status")],
            );
        }

        if prefix.starts_with("/alignment ") {
            return self.rank_suggestions(
                input,
                vec![
                    (
                        "/alignment status".into(),
                        "Show current and saved alignment",
                    ),
                    (
                        "/alignment centered".into(),
                        "Save centered alignment and apply it now",
                    ),
                    (
                        "/alignment left".into(),
                        "Save left-aligned layout and apply it now",
                    ),
                ],
            );
        }

        if prefix.starts_with("/config ") {
            return self.rank_suggestions(
                input,
                vec![
                    ("/config init".into(), "Create a default config file"),
                    ("/config create".into(), "Alias for /config init"),
                    ("/config edit".into(), "Open the config file in $EDITOR"),
                ],
            );
        }

        if prefix.starts_with("/goals show ") {
            let relevant_goals = crate::goal::list_relevant_goals(
                self.session
                    .working_dir
                    .as_deref()
                    .map(std::path::Path::new),
            )
            .unwrap_or_default();
            let suggestions = relevant_goals
                .into_iter()
                .map(|goal| (format!("/goals show {}", goal.id), "Open this goal"))
                .collect();
            return self.rank_suggestions(input, suggestions);
        }

        if prefix.starts_with("/goals ") {
            return self.rank_suggestions(
                input,
                vec![
                    ("/goals resume".into(), "Resume the current goal"),
                    ("/goals show".into(), "Open a specific goal by id"),
                ],
            );
        }

        if prefix.starts_with("/selfdev ") {
            return self.rank_suggestions(
                input,
                vec![
                    (
                        "/selfdev status".into(),
                        "Show current self-dev/build status",
                    ),
                    ("/selfdev enter".into(), "Open a blank self-dev session"),
                    (
                        "/selfdev enter ".into(),
                        "Open a self-dev session with a prompt",
                    ),
                ],
            );
        }

        if prefix.starts_with("/rewind ") {
            let suggestions = (1..=self.session.messages.len())
                .map(|n| (format!("/rewind {}", n), "Rewind to this message"))
                .collect();
            return self.rank_suggestions(input, suggestions);
        }

        self.rank_suggestions(&prefix, self.command_candidates())
    }

    /// Get command suggestions based on current input
    pub fn command_suggestions(&self) -> Vec<(String, &'static str)> {
        self.get_suggestions_for(&self.input)
    }

    /// Get suggestion prompts for new users on the initial empty screen.
    /// Returns (label, prompt_text) pairs. Empty once user is experienced or not authenticated.
    pub fn suggestion_prompts(&self) -> Vec<(String, String)> {
        let is_canary = if self.is_remote {
            self.remote_is_canary.unwrap_or(self.session.is_canary)
        } else {
            self.session.is_canary
        };
        if is_canary {
            return Vec::new();
        }

        let auth = crate::auth::AuthStatus::check_fast();
        if !auth.has_any_available() {
            return vec![("Log in to get started".to_string(), "/login".to_string())];
        }

        if !self.display_messages.is_empty() || self.is_processing {
            return Vec::new();
        }

        let is_new_user = crate::storage::jcode_dir()
            .ok()
            .and_then(|dir| {
                let path = dir.join("setup_hints.json");
                std::fs::read_to_string(&path).ok()
            })
            .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
            .and_then(|v| v.get("launch_count")?.as_u64())
            .map(|count| count <= 5)
            .unwrap_or(true);

        if !is_new_user {
            return Vec::new();
        }

        vec![
            (
                "Customize my terminal theme".to_string(),
                "Find what terminal I'm using, then change its background color to pitch black and make it slightly transparent. Apply the changes for me.".to_string(),
            ),
            (
                "Review something I've been working on".to_string(),
                "Find a recent file or project I've been working on, read through it, and give me concrete suggestions on how I could improve it.".to_string(),
            ),
            (
                "Find my social media and roast me".to_string(),
                "Find a social media platform I use, look around at my profile and posts, then give me a brutally honest roast based on what you see.".to_string(),
            ),
        ]
    }

    /// Autocomplete current input - cycles through suggestions on repeated Tab
    pub fn autocomplete(&mut self) -> bool {
        // Get suggestions for current input
        let current_suggestions = self.get_suggestions_for(&self.input);

        // Check if we're continuing a tab cycle from a previous base
        if let Some((ref base, idx)) = self.tab_completion_state.clone() {
            let base_suggestions = self.get_suggestions_for(&base);

            // If current input is in base suggestions AND there are multiple options, continue cycling
            if base_suggestions.len() > 1
                && base_suggestions.iter().any(|(cmd, _)| cmd == &self.input)
            {
                let next_index = (idx + 1) % base_suggestions.len();
                let (cmd, _) = &base_suggestions[next_index];
                self.remember_input_undo_state();
                self.input = cmd.clone();
                self.cursor_pos = self.input.len();
                self.tab_completion_state = Some((base.clone(), next_index));
                return true;
            }
            // Otherwise, fall through to start a new cycle with current input
        }

        // Start fresh cycle with current input
        if current_suggestions.is_empty() {
            self.tab_completion_state = None;
            return false;
        }

        // If only one suggestion and it matches exactly, add trailing space for commands
        // that accept arguments, then we're done
        if current_suggestions.len() == 1 && current_suggestions[0].0 == self.input {
            if !self.input.ends_with(' ') && Self::command_accepts_args(&self.input) {
                self.remember_input_undo_state();
                self.input.push(' ');
                self.cursor_pos = self.input.len();
                return true;
            }
            self.tab_completion_state = None;
            return false;
        }

        // Apply first suggestion and start tracking the cycle
        let (cmd, _) = &current_suggestions[0];
        let base = self.input.clone();
        self.remember_input_undo_state();
        self.input = cmd.clone();
        // If unique match, add trailing space for arg-accepting commands
        if current_suggestions.len() == 1 && Self::command_accepts_args(&self.input) {
            self.input.push(' ');
        }
        self.cursor_pos = self.input.len();
        self.tab_completion_state = Some((base, 0));
        true
    }

    /// Reset tab completion state (call when user types/modifies input)
    pub fn reset_tab_completion(&mut self) {
        self.tab_completion_state = None;
    }

    pub(super) fn remember_input_undo_state(&mut self) {
        let snapshot = (self.input.clone(), self.cursor_pos.min(self.input.len()));
        if self.input_undo_stack.last() == Some(&snapshot) {
            return;
        }
        if self.input_undo_stack.len() >= Self::INPUT_UNDO_LIMIT {
            self.input_undo_stack.remove(0);
        }
        self.input_undo_stack.push(snapshot);
    }

    pub(super) fn clear_input_undo_history(&mut self) {
        self.input_undo_stack.clear();
    }

    pub(super) fn undo_input_change(&mut self) {
        if let Some((input, cursor_pos)) = self.input_undo_stack.pop() {
            self.input = input;
            self.cursor_pos = cursor_pos.min(self.input.len());
            self.reset_tab_completion();
            self.sync_model_picker_preview_from_input();
            self.set_status_notice("↶ Input restored");
        } else {
            self.set_status_notice("Nothing to undo");
        }
    }

    pub(super) fn command_accepts_args(cmd: &str) -> bool {
        matches!(
            cmd.trim(),
            "/help"
                | "/?"
                | "/btw"
                | "/observe"
                | "/model"
                | "/agents"
                | "/effort"
                | "/fast"
                | "/transport"
                | "/login"
                | "/auth"
                | "/account"
                | "/account claude"
                | "/account switch"
                | "/account openai"
                | "/account openai-compatible"
                | "/account default-provider"
                | "/account default-model"
                | "/account claude switch"
                | "/account claude remove"
                | "/account openai switch"
                | "/account openai remove"
                | "/usage"
                | "/subscription"
                | "/memory"
                | "/goals"
                | "/goals show"
                | "/swarm"
                | "/improve"
                | "/refactor"
                | "/rewind"
                | "/compact"
                | "/compact mode"
                | "/alignment"
                | "/config"
                | "/save"
                | "/cache"
        )
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

    pub fn available_skills(&self) -> Vec<&str> {
        self.skills.list().iter().map(|s| s.name.as_str()).collect()
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
            parts.push(format!("{:.1}s", secs));
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

    pub(super) fn push_turn_footer(&mut self, duration: Option<f32>) {
        self.log_cache_miss_if_unexpected();

        self.last_api_completed = Some(Instant::now());
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

        // Unexpected cache miss: on turn 3+, we should no longer be in cache warm-up
        let is_unexpected = is_unexpected_cache_miss(
            user_turn_count,
            self.streaming_cache_read_tokens,
            self.streaming_cache_creation_tokens,
        );

        if is_unexpected {
            // Collect context for debugging
            let session_id = self.session_id().to_string();
            let provider = self.provider.name().to_string();
            let model = self.provider.model();
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
                "CACHE_MISS: unexpected cache miss on turn {} | \
                 cache_creation={} cache_read={} | \
                 input={} output={} | \
                 session={} provider={} model={} | \
                 msgs: user={} assistant={} tool={} other={}",
                user_turn_count,
                cache_creation_dbg,
                cache_read_dbg,
                input_tokens,
                output_tokens,
                session_id,
                provider,
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
            self.streaming_text.push_str(&warning);
            self.context_warning_shown = true;
        } else if self.context_warning_shown && usage_percent >= 80.0 {
            // Reset to show 80% warning
            if usage_percent < 85.0 {
                let warning = format!(
                    "\n⚠️  Context usage: {:.0}% - compaction imminent\n\n",
                    usage_percent
                );
                self.streaming_text.push_str(&warning);
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
        self.last_stream_activity.map(|t| t.elapsed())
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
        self.processing_started.map(|t| t.elapsed()).or_else(|| {
            self.split_launch_in_flight()
                .then(|| self.pending_split_started_at.map(|t| t.elapsed()))
                .flatten()
        })
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

    pub(super) fn save_input_for_reload(&self, session_id: &str) {
        if self.input.is_empty()
            && self.queued_messages.is_empty()
            && self.hidden_queued_system_messages.is_empty()
            && self.interleave_message.is_none()
            && self.pending_soft_interrupts.is_empty()
            && self.rate_limit_pending_message.is_none()
            && !self.observe_mode_enabled
        {
            return;
        }
        if let Ok(jcode_dir) = crate::storage::jcode_dir() {
            let path = jcode_dir.join(format!("client-input-{}", session_id));
            let rate_limit_reset_in_ms = self.rate_limit_reset.and_then(|reset| {
                let now = Instant::now();
                if reset <= now {
                    Some(0)
                } else {
                    Some((reset - now).as_millis().min(u64::MAX as u128) as u64)
                }
            });
            let rate_limit_pending_message =
                self.rate_limit_pending_message.as_ref().map(|pending| {
                    serde_json::json!({
                        "content": pending.content,
                        "images": pending.images,
                        "is_system": pending.is_system,
                        "system_reminder": pending.system_reminder,
                        "auto_retry": pending.auto_retry,
                        "retry_attempts": pending.retry_attempts,
                    })
                });
            let data = serde_json::json!({
                "cursor": self.cursor_pos,
                "input": self.input,
                "queued_messages": self.queued_messages,
                "hidden_queued_system_messages": self.hidden_queued_system_messages,
                "interleave_message": self.interleave_message,
                "pending_soft_interrupts": self.pending_soft_interrupts,
                "rate_limit_pending_message": rate_limit_pending_message,
                "rate_limit_reset_in_ms": rate_limit_reset_in_ms,
                "observe_mode_enabled": self.observe_mode_enabled,
                "observe_page_markdown": self.observe_page_markdown,
                "observe_page_updated_at_ms": self.observe_page_updated_at_ms,
            });
            let _ = std::fs::write(&path, data.to_string());
        }
    }

    pub(crate) fn save_startup_message_for_session(session_id: &str, message: String) {
        if message.trim().is_empty() {
            return;
        }
        if let Ok(jcode_dir) = crate::storage::jcode_dir() {
            let path = jcode_dir.join(format!("client-input-{}", session_id));
            let inferred_hints = infer_spawned_session_startup_hints(&message);
            let data = serde_json::json!({
                "cursor": 0,
                "input": "",
                "queued_messages": [],
                "hidden_queued_system_messages": [message],
                "startup_status_notice": inferred_hints.as_ref().map(|(status, _)| status.clone()),
                "startup_display_message_title": inferred_hints.as_ref().map(|(_, (title, _))| title.clone()),
                "startup_display_message": inferred_hints.as_ref().map(|(_, (_, body))| body.clone()),
                "interleave_message": serde_json::Value::Null,
                "pending_soft_interrupts": [],
                "rate_limit_pending_message": serde_json::Value::Null,
                "rate_limit_reset_in_ms": serde_json::Value::Null,
                "observe_mode_enabled": false,
                "observe_page_markdown": "",
                "observe_page_updated_at_ms": 0,
            });
            let _ = std::fs::write(&path, data.to_string());
        }
    }

    pub(super) fn restore_input_for_reload(session_id: &str) -> Option<RestoredReloadInput> {
        let jcode_dir = crate::storage::jcode_dir().ok()?;
        let path = jcode_dir.join(format!("client-input-{}", session_id));
        if !path.exists() {
            return None;
        }
        let data = std::fs::read_to_string(&path).ok()?;
        let _ = std::fs::remove_file(&path);

        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&data) {
            let input = value
                .get("input")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let cursor = value.get("cursor").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let queued_messages = value
                .get("queued_messages")
                .and_then(|v| v.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(|s| s.to_string()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let hidden_queued_system_messages = value
                .get("hidden_queued_system_messages")
                .and_then(|v| v.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(|s| s.to_string()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let startup_status_notice = value
                .get("startup_status_notice")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty());
            let startup_display_message = value
                .get("startup_display_message")
                .and_then(|v| v.as_str())
                .map(|body| {
                    let title = value
                        .get("startup_display_message_title")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Launch")
                        .to_string();
                    (title, body.to_string())
                })
                .filter(|(_, body)| !body.is_empty());
            let interleave_message = value
                .get("interleave_message")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty());
            let pending_soft_interrupts = value
                .get("pending_soft_interrupts")
                .and_then(|v| v.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(|s| s.to_string()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let rate_limit_pending_message = value
                .get("rate_limit_pending_message")
                .and_then(|pending| pending.as_object())
                .map(|pending| super::PendingRemoteMessage {
                    content: pending
                        .get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    images: pending
                        .get("images")
                        .and_then(|v| v.as_array())
                        .map(|items| {
                            items
                                .iter()
                                .filter_map(|item| {
                                    let pair = item.as_array()?;
                                    let first = pair.first()?.as_str()?;
                                    let second = pair.get(1)?.as_str()?;
                                    Some((first.to_string(), second.to_string()))
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default(),
                    is_system: pending
                        .get("is_system")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false),
                    system_reminder: pending
                        .get("system_reminder")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    auto_retry: pending
                        .get("auto_retry")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false),
                    retry_attempts: pending
                        .get("retry_attempts")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u8,
                    retry_at: None,
                });
            let rate_limit_reset = value
                .get("rate_limit_reset_in_ms")
                .and_then(|v| v.as_u64())
                .map(|delay_ms| Instant::now() + Duration::from_millis(delay_ms));
            let mut rate_limit_pending_message = rate_limit_pending_message;
            if let (Some(pending), Some(reset)) =
                (&mut rate_limit_pending_message, rate_limit_reset)
            {
                pending.retry_at = Some(reset);
            }
            let observe_mode_enabled = value
                .get("observe_mode_enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let observe_page_markdown = value
                .get("observe_page_markdown")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let observe_page_updated_at_ms = value
                .get("observe_page_updated_at_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let cursor = cursor.min(input.len());
            return Some(RestoredReloadInput {
                input,
                cursor,
                queued_messages,
                hidden_queued_system_messages,
                startup_status_notice,
                startup_display_message,
                interleave_message,
                pending_soft_interrupts,
                rate_limit_pending_message,
                rate_limit_reset,
                observe_mode_enabled,
                observe_page_markdown,
                observe_page_updated_at_ms,
            });
        }

        let (cursor_str, input) = data.split_once('\n')?;
        let cursor = cursor_str.parse::<usize>().unwrap_or(0);
        let cursor = cursor.min(input.len());
        Some(RestoredReloadInput {
            input: input.to_string(),
            cursor,
            queued_messages: Vec::new(),
            hidden_queued_system_messages: Vec::new(),
            startup_status_notice: None,
            startup_display_message: None,
            interleave_message: None,
            pending_soft_interrupts: Vec::new(),
            rate_limit_pending_message: None,
            rate_limit_reset: None,
            observe_mode_enabled: false,
            observe_page_markdown: String::new(),
            observe_page_updated_at_ms: 0,
        })
    }

    /// Toggle scroll bookmark: stash current position and jump to bottom,
    /// or restore stashed position if already at bottom.
    pub(super) fn toggle_scroll_bookmark(&mut self) {
        if let Some(saved) = self.scroll_bookmark.take() {
            // We have a bookmark — teleport back to it
            self.scroll_offset = saved;
            self.auto_scroll_paused = saved > 0;
            self.set_status_notice("📌 Returned to bookmark");
        } else if self.auto_scroll_paused && self.scroll_offset > 0 {
            // We're scrolled up — save position and jump to bottom
            self.scroll_bookmark = Some(self.scroll_offset);
            self.follow_chat_bottom();
            self.set_status_notice("📌 Bookmark set — press again to return");
        }
        // If already at bottom with no bookmark, do nothing
    }

    pub(super) fn follow_chat_bottom_for_typing(&mut self) {
        if !self.typing_scroll_lock {
            self.follow_chat_bottom();
        }
    }

    pub(super) fn set_side_panel_snapshot(
        &mut self,
        snapshot: crate::side_panel::SidePanelSnapshot,
    ) {
        let focus_observe = self.observe_mode_enabled
            && self.side_panel.focused_page_id.as_deref() == Some(super::observe::OBSERVE_PAGE_ID);
        let snapshot = if self.observe_mode_enabled {
            self.decorate_side_panel_with_observe(snapshot, focus_observe)
        } else {
            snapshot
        };
        self.apply_side_panel_snapshot(snapshot);
    }

    pub(super) fn apply_side_panel_snapshot(
        &mut self,
        snapshot: crate::side_panel::SidePanelSnapshot,
    ) {
        let focused_before = self.side_panel.focused_page_id.clone();
        let focused_after = snapshot.focused_page_id.clone();
        let focused_changed = focused_before != focused_after;
        let focused_title_after = snapshot.focused_page().map(|page| page.title.clone());
        if let Some(focused_after) = focused_after.as_deref() {
            if focused_after != super::observe::OBSERVE_PAGE_ID {
                self.last_side_panel_focus_id = Some(focused_after.to_string());
            }
        } else if snapshot.pages.is_empty() {
            self.last_side_panel_focus_id = None;
        }
        self.last_side_panel_refresh = None;
        self.side_panel = snapshot;
        if focused_changed {
            self.diff_pane_scroll = 0;
            self.diff_pane_scroll_x = 0;
            self.diff_pane_auto_scroll = true;
        }
        if focused_changed {
            match (focused_after.as_deref(), focused_title_after.as_deref()) {
                (Some(super::observe::OBSERVE_PAGE_ID), _) => self.set_status_notice("Observe"),
                (Some("goals"), _) => self.set_status_notice("Goals"),
                (Some(id), Some(title)) if id.starts_with("goal.") => self.set_status_notice(title),
                _ => {}
            }
        }
        self.sync_diagram_fit_context();
        self.prewarm_focused_side_panel();
    }

    pub(super) fn refresh_side_panel_linked_content_if_due(&mut self) {
        const SIDE_PANEL_REFRESH_INTERVAL: Duration = Duration::from_millis(250);

        let should_refresh = self
            .side_panel
            .focused_page()
            .map(|page| page.source == crate::side_panel::SidePanelPageSource::LinkedFile)
            .unwrap_or(false);

        if !should_refresh {
            self.last_side_panel_refresh = None;
            return;
        }

        let now = Instant::now();
        if self
            .last_side_panel_refresh
            .is_some_and(|last| now.duration_since(last) < SIDE_PANEL_REFRESH_INTERVAL)
        {
            return;
        }

        self.last_side_panel_refresh = Some(now);
        if crate::side_panel::refresh_linked_page_content(&mut self.side_panel, None) {
            self.sync_diagram_fit_context();
            self.prewarm_focused_side_panel();
        }
    }

    pub(super) fn toggle_typing_scroll_lock(&mut self) {
        self.typing_scroll_lock = !self.typing_scroll_lock;
        let status = if self.typing_scroll_lock {
            "Typing scroll lock: ON — typing stays at current chat position"
        } else {
            "Typing scroll lock: OFF — typing follows chat bottom"
        };
        self.set_status_notice(status);
    }

    pub(super) fn toggle_centered_mode(&mut self) {
        self.centered = !self.centered;
        let mode = if self.centered {
            "Centered"
        } else {
            "Left-aligned"
        };
        self.set_status_notice(format!("Layout: {}", mode));
        self.prewarm_focused_side_panel();
    }

    pub fn set_centered(&mut self, centered: bool) {
        self.centered = centered;
        self.prewarm_focused_side_panel();
    }

    fn prewarm_focused_side_panel(&self) {
        let Ok((terminal_width, terminal_height)) = crossterm::terminal::size() else {
            return;
        };
        let has_protocol = crate::tui::mermaid::protocol_type().is_some();
        let _ = crate::tui::prewarm_focused_side_panel(
            &self.side_panel,
            terminal_width,
            terminal_height,
            self.diagram_pane_ratio,
            has_protocol,
            self.centered,
        );
    }

    // ==================== Debug Socket Methods ====================

    /// Enable debug socket and return the broadcast receiver
    /// Call this before run() to enable debug event broadcasting
    pub fn enable_debug_socket(&mut self) -> tokio::sync::broadcast::Receiver<backend::DebugEvent> {
        let (tx, rx) = tokio::sync::broadcast::channel(256);
        self.debug_tx = Some(tx);
        rx
    }

    /// Broadcast a debug event to connected clients (if debug socket enabled)
    pub(super) fn broadcast_debug(&self, event: backend::DebugEvent) {
        if let Some(ref tx) = self.debug_tx {
            let _ = tx.send(event); // Ignore errors (no receivers)
        }
    }

    /// Create a full state snapshot for debug socket
    pub fn create_debug_snapshot(&self) -> backend::DebugEvent {
        use backend::{DebugEvent, DebugMessage};

        DebugEvent::StateSnapshot {
            display_messages: self
                .display_messages
                .iter()
                .map(|m| DebugMessage {
                    role: m.role.clone(),
                    content: m.content.clone(),
                    tool_calls: m.tool_calls.clone(),
                    duration_secs: m.duration_secs,
                    title: m.title.clone(),
                    tool_data: m.tool_data.clone(),
                })
                .collect(),
            streaming_text: self.streaming_text.clone(),
            streaming_tool_calls: self.streaming_tool_calls.clone(),
            input: self.input.clone(),
            cursor_pos: self.cursor_pos,
            is_processing: self.is_processing,
            scroll_offset: self.scroll_offset,
            status: format!("{:?}", self.status),
            provider_name: self.provider.name().to_string(),
            provider_model: self.provider.model().to_string(),
            mcp_servers: self
                .mcp_server_names
                .iter()
                .map(|(name, _)| name.clone())
                .collect(),
            skills: self.skills.list().iter().map(|s| s.name.clone()).collect(),
            session_id: self.provider_session_id.clone(),
            input_tokens: self.streaming_input_tokens,
            output_tokens: self.streaming_output_tokens,
            cache_read_input_tokens: self.streaming_cache_read_tokens,
            cache_creation_input_tokens: self.streaming_cache_creation_tokens,
            queued_messages: self.queued_messages.clone(),
        }
    }

    /// Start debug socket listener task
    /// Returns a JoinHandle for the listener task
    pub fn start_debug_socket_listener(
        &self,
        mut rx: tokio::sync::broadcast::Receiver<backend::DebugEvent>,
    ) -> tokio::task::JoinHandle<()> {
        use crate::transport::Listener;
        use tokio::io::AsyncWriteExt;

        let socket_path = Self::debug_socket_path();
        let initial_snapshot = self.create_debug_snapshot();

        tokio::spawn(async move {
            // Clean up old socket
            let _ = std::fs::remove_file(&socket_path);

            #[allow(unused_mut)]
            let mut listener = match Listener::bind(&socket_path) {
                Ok(l) => l,
                Err(e) => {
                    crate::logging::error(&format!("Failed to bind debug socket: {}", e));
                    return;
                }
            };

            // Restrict TUI debug socket to owner-only.
            let _ = crate::platform::set_permissions_owner_only(&socket_path);

            // Accept connections and forward events
            let clients: std::sync::Arc<tokio::sync::Mutex<Vec<crate::transport::WriteHalf>>> =
                std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));

            let clients_clone = clients.clone();

            // Spawn event broadcaster
            let broadcast_handle = tokio::spawn(async move {
                while let Ok(event) = rx.recv().await {
                    let json = match serde_json::to_string(&event) {
                        Ok(j) => j + "\n",
                        Err(_) => continue,
                    };
                    let bytes = json.as_bytes();

                    let mut clients = clients_clone.lock().await;
                    let mut to_remove = Vec::new();

                    for (i, writer) in clients.iter_mut().enumerate() {
                        if writer.write_all(bytes).await.is_err() {
                            to_remove.push(i);
                        }
                    }

                    // Remove disconnected clients (reverse order to preserve indices)
                    for i in to_remove.into_iter().rev() {
                        clients.swap_remove(i);
                    }
                }
            });

            // Accept new connections
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        let (_, writer) = stream.into_split();
                        let mut writer = writer;

                        // Send initial snapshot
                        let snapshot_json =
                            serde_json::to_string(&initial_snapshot).unwrap_or_default() + "\n";
                        if writer.write_all(snapshot_json.as_bytes()).await.is_ok() {
                            clients.lock().await.push(writer);
                        }
                    }
                    Err(_) => break,
                }
            }

            broadcast_handle.abort();
            let _ = std::fs::remove_file(&socket_path);
        })
    }

    /// Get the debug socket path
    pub fn debug_socket_path() -> std::path::PathBuf {
        crate::storage::runtime_dir().join("jcode-debug.sock")
    }
}

pub(super) fn handle_info_command(app: &mut App, trimmed: &str) -> bool {
    if trimmed == "/version" {
        let version = env!("JCODE_VERSION");
        let is_canary = if app.session.is_canary {
            " (canary/self-dev)"
        } else {
            ""
        };
        app.push_display_message(DisplayMessage {
            role: "system".to_string(),
            content: format!("jcode {}{}", version, is_canary),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        });
        return true;
    }

    if trimmed == "/changelog" {
        app.changelog_scroll = Some(0);
        return true;
    }

    if trimmed == "/cache" || trimmed.starts_with("/cache ") {
        let arg = trimmed.strip_prefix("/cache").unwrap_or("").trim();
        match arg {
            "1h" | "1hour" | "extended" => {
                crate::provider::anthropic::set_cache_ttl_1h(true);
                app.push_display_message(DisplayMessage::system(
                    "Cache TTL set to 1 hour. Cache writes cost 2x base input tokens.".to_string(),
                ));
            }
            "5m" | "5min" | "default" | "reset" => {
                crate::provider::anthropic::set_cache_ttl_1h(false);
                app.push_display_message(DisplayMessage::system(
                    "Cache TTL set to 5 minutes (default).".to_string(),
                ));
            }
            "" => {
                let current = crate::provider::anthropic::is_cache_ttl_1h();
                let new_state = !current;
                crate::provider::anthropic::set_cache_ttl_1h(new_state);
                let msg = if new_state {
                    "Cache TTL toggled to 1 hour. Cache writes cost 2x base input tokens.\nUse `/cache 5m` to revert."
                } else {
                    "Cache TTL toggled to 5 minutes (default).\nUse `/cache 1h` to extend."
                };
                app.push_display_message(DisplayMessage::system(msg.to_string()));
            }
            _ => {
                app.push_display_message(DisplayMessage::error(
                    "Usage: `/cache` (toggle), `/cache 1h` (1 hour), `/cache 5m` (default)"
                        .to_string(),
                ));
            }
        }
        return true;
    }

    if trimmed == "/info" {
        let version = env!("JCODE_VERSION");
        let terminal_size = crossterm::terminal::size()
            .map(|(w, h)| format!("{}x{}", w, h))
            .unwrap_or_else(|_| "unknown".to_string());
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "unknown".to_string());

        let turn_count = app
            .display_messages
            .iter()
            .filter(|m| m.role == "user")
            .count();

        let session_duration = chrono::Utc::now().signed_duration_since(app.session.created_at);
        let duration_str = if session_duration.num_hours() > 0 {
            format!(
                "{}h {}m",
                session_duration.num_hours(),
                session_duration.num_minutes() % 60
            )
        } else if session_duration.num_minutes() > 0 {
            format!("{}m", session_duration.num_minutes())
        } else {
            format!("{}s", session_duration.num_seconds())
        };

        let mut info = String::new();
        info.push_str(&format!("**Version:** {}\n", version));
        info.push_str(&format!(
            "**Session:** {} ({})\n",
            app.session.short_name.as_deref().unwrap_or("unnamed"),
            &app.session.id[..8]
        ));
        info.push_str(&format!(
            "**Duration:** {} ({} turns)\n",
            duration_str, turn_count
        ));
        info.push_str(&format!(
            "**Tokens:** ↑{} ↓{}\n",
            app.total_input_tokens, app.total_output_tokens
        ));
        info.push_str(&format!("**Terminal:** {}\n", terminal_size));
        info.push_str(&format!("**CWD:** {}\n", cwd));
        info.push_str(&format!(
            "**Features:** memory={}, swarm={}\n",
            if app.memory_enabled { "on" } else { "off" },
            if app.swarm_enabled { "on" } else { "off" }
        ));

        if let Some(ref model) = app.remote_provider_model {
            info.push_str(&format!("**Model:** {}\n", model));
        }
        if let Some(ref provider_id) = app.provider_session_id {
            info.push_str(&format!(
                "**Provider Session:** {}...\n",
                &provider_id[..provider_id.len().min(16)]
            ));
        }

        if app.session.is_canary {
            info.push_str("\n**Self-Dev Mode:** enabled\n");
            if let Some(ref build) = app.session.testing_build {
                info.push_str(&format!("**Testing Build:** {}\n", build));
            }
        }

        if app.is_remote {
            info.push_str("\n**Remote Mode:** connected\n");
            if let Some(count) = app.remote_client_count {
                info.push_str(&format!("**Connected Clients:** {}\n", count));
            }
        }

        app.push_display_message(DisplayMessage {
            role: "system".to_string(),
            content: info,
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        });
        return true;
    }

    if trimmed == "/context" {
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "unknown".to_string());
        let terminal_size = crossterm::terminal::size()
            .map(|(w, h)| format!("{}x{}", w, h))
            .unwrap_or_else(|_| "unknown".to_string());
        let active_session_id = app
            .active_client_session_id()
            .unwrap_or(app.session.id.as_str())
            .to_string();
        let context = app.context_info();
        let todos = super::helpers::gather_todos_for_session(Some(active_session_id.as_str()));

        let (provider_name, model_name, reasoning_effort, service_tier, transport, total_tokens) =
            if app.is_remote {
                (
                    app.remote_provider_name
                        .clone()
                        .unwrap_or_else(|| app.provider.name().to_string()),
                    app.remote_provider_model
                        .clone()
                        .unwrap_or_else(|| app.provider.model()),
                    app.remote_reasoning_effort.clone(),
                    app.remote_service_tier.clone(),
                    app.remote_transport.clone(),
                    app.remote_total_tokens,
                )
            } else {
                (
                    app.provider.name().to_string(),
                    app.provider.model(),
                    app.provider.reasoning_effort(),
                    app.provider.service_tier(),
                    app.provider.transport(),
                    Some((app.total_input_tokens, app.total_output_tokens)),
                )
            };

        let compaction_summary = if app.provider.supports_compaction() {
            let manager = app.registry.compaction();
            if let Ok(manager) = manager.try_read() {
                let stats = manager.stats_with(&app.messages);
                let mode = if app.is_remote {
                    app.remote_compaction_mode
                        .as_ref()
                        .map(|mode| mode.as_str().to_string())
                        .unwrap_or_else(|| "unknown".to_string())
                } else {
                    manager.mode().as_str().to_string()
                };
                let summary_kind = match app.session.compaction.as_ref() {
                    Some(state) if state.openai_encrypted_content.is_some() => {
                        "native/openai-encrypted"
                    }
                    Some(_) => "summary-text",
                    None => "none",
                };
                format!(
                    "- supported: yes\n- mode: {}\n- jcode-managed: {}\n- active summary: {} ({})\n- compacted messages: {}\n- active messages: {}\n- summary chars: {}\n- estimated tokens: {}\n- effective tokens: {}\n- observed tokens: {}\n- usage: {:.1}%\n- compacting now: {}\n- budget: {}",
                    mode,
                    if app.provider.uses_jcode_compaction() {
                        "yes"
                    } else {
                        "no"
                    },
                    if stats.has_summary { "yes" } else { "no" },
                    summary_kind,
                    manager.compacted_count(),
                    stats.active_messages,
                    manager.summary_chars(),
                    stats.token_estimate,
                    stats.effective_tokens,
                    stats
                        .observed_input_tokens
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "n/a".to_string()),
                    stats.context_usage * 100.0,
                    if stats.is_compacting { "yes" } else { "no" },
                    manager.token_budget(),
                )
            } else {
                "- supported: yes\n- state: unavailable (compaction manager busy)".to_string()
            }
        } else {
            "- supported: no".to_string()
        };

        let pending_images = app.pending_images.len();
        let queued_messages = app.queued_messages.len();
        let soft_interrupts = app.pending_soft_interrupts.len();
        let side_panel_pages = app.side_panel.pages.len();
        let focused_side_panel = app.side_panel.focused_page_id.as_deref().unwrap_or("none");

        let mut todo_lines = String::new();
        if todos.is_empty() {
            todo_lines.push_str("- none\n");
        } else {
            for todo in todos.iter().take(8) {
                todo_lines.push_str(&format!(
                    "- [{}|{}] {}\n",
                    todo.status, todo.priority, todo.content
                ));
            }
            if todos.len() > 8 {
                todo_lines.push_str(&format!("- … {} more\n", todos.len() - 8));
            }
        }

        let mut context_report = String::new();
        context_report.push_str("# Session Context\n\n");
        context_report.push_str("## Runtime\n");
        context_report.push_str(&format!("- session id: `{}`\n", active_session_id));
        context_report.push_str(&format!("- session name: {}\n", app.session.display_name()));
        context_report.push_str(&format!(
            "- mode: {}{}{}\n",
            if app.is_remote { "remote" } else { "local" },
            if app.is_replay { ", replay" } else { "" },
            if app.session.is_canary {
                ", self-dev"
            } else {
                ""
            }
        ));
        context_report.push_str(&format!("- provider: {}\n", provider_name));
        context_report.push_str(&format!("- model: {}\n", model_name));
        context_report.push_str(&format!(
            "- reasoning effort: {}\n",
            reasoning_effort.as_deref().unwrap_or("default")
        ));
        context_report.push_str(&format!(
            "- service tier: {}\n",
            service_tier.as_deref().unwrap_or("default")
        ));
        context_report.push_str(&format!(
            "- transport: {}\n",
            transport.as_deref().unwrap_or("default")
        ));
        context_report.push_str(&format!("- cwd: {}\n", cwd));
        context_report.push_str(&format!("- terminal: {}\n", terminal_size));
        context_report.push_str(&format!(
            "- features: memory={}, swarm={}\n",
            if app.memory_enabled { "on" } else { "off" },
            if app.swarm_enabled { "on" } else { "off" }
        ));
        context_report.push_str(&format!(
            "- processing: {}\n",
            match &app.status {
                ProcessingStatus::Idle => "idle".to_string(),
                ProcessingStatus::Sending => "sending".to_string(),
                ProcessingStatus::Connecting(phase) => format!("connecting ({})", phase),
                ProcessingStatus::Thinking(_) => "thinking".to_string(),
                ProcessingStatus::Streaming => "streaming".to_string(),
                ProcessingStatus::RunningTool(name) => format!("running tool ({})", name),
            }
        ));
        if let Some((input, output)) = total_tokens {
            context_report.push_str(&format!("- session tokens: ↑{} ↓{}\n", input, output));
        }
        context_report.push_str("\n## Prompt / Context Composition\n");
        context_report.push_str(&format!(
            "- total chars: {} (~{} tokens)\n",
            context.total_chars,
            context.estimated_tokens()
        ));
        context_report.push_str(&format!(
            "- system prompt: {} chars\n- env context: {} chars\n- project AGENTS.md: {} ({})\n- global ~/.AGENTS.md: {} ({})\n- prompt overlays: {} chars\n- skills section: {} chars\n- self-dev section: {} chars\n- memory section: {} chars\n- tool definitions: {} chars across {} tools\n- user messages: {} chars across {} messages\n- assistant messages: {} chars across {} messages\n- tool calls: {} chars across {} calls\n- tool results: {} chars across {} results\n",
            context.system_prompt_chars,
            context.env_context_chars,
            if context.has_project_agents_md { "loaded" } else { "not loaded" },
            context.project_agents_md_chars,
            if context.has_global_agents_md { "loaded" } else { "not loaded" },
            context.global_agents_md_chars,
            context.prompt_overlay_chars,
            context.skills_chars,
            context.selfdev_chars,
            context.memory_chars,
            context.tool_defs_chars,
            context.tool_defs_count,
            context.user_messages_chars,
            context.user_messages_count,
            context.assistant_messages_chars,
            context.assistant_messages_count,
            context.tool_calls_chars,
            context.tool_calls_count,
            context.tool_results_chars,
            context.tool_results_count,
        ));
        context_report.push_str("\n## Compaction\n");
        context_report.push_str(&compaction_summary);
        context_report.push_str("\n\n## Session State\n");
        context_report.push_str(&format!(
            "- queue mode: {}\n- queued messages: {}\n- interleave pending: {}\n- soft interrupts pending: {}\n- pasted snippets buffered: {}\n- pending images: {}\n- active skill: {}\n- autonomy mode: {}\n- subagent status: {}\n- provider session id: {}\n- status notice: {}\n- last stream error: {}\n- stashed input: {}\n",
            if app.queue_mode { "on" } else { "off" },
            queued_messages,
            if app.interleave_message.is_some() { "yes" } else { "no" },
            soft_interrupts,
            app.pasted_contents.len(),
            pending_images,
            app.active_skill.as_deref().unwrap_or("none"),
            app.improve_mode
                .map(|mode| mode.status_label())
                .unwrap_or("inactive"),
            app.subagent_status.as_deref().unwrap_or("idle"),
            app.provider_session_id.as_deref().unwrap_or("none"),
            app.status_notice()
                .as_deref()
                .unwrap_or("none"),
            app.last_stream_error.as_deref().unwrap_or("none"),
            if app.stashed_input.is_some() { "yes" } else { "no" },
        ));
        context_report.push_str("\n## Todos\n");
        context_report.push_str(&todo_lines);
        context_report.push_str("\n## Side Panel\n");
        context_report.push_str(&format!(
            "- pages: {}\n- focused page: {}\n",
            side_panel_pages, focused_side_panel
        ));

        if let Some(page) = app.side_panel.focused_page() {
            context_report.push_str(&format!(
                "- focused title: {}\n- focused source: {} ({})\n- focused content chars: {}\n",
                page.title,
                page.source.as_str(),
                page.format.as_str(),
                page.content.len(),
            ));
        }

        if app.swarm_enabled {
            context_report.push_str("\n## Swarm\n");
            context_report.push_str(&format!(
                "- plan items: {}\n- remote members: {}\n- connected clients: {}\n",
                app.swarm_plan_items.len(),
                app.remote_swarm_members.len(),
                app.remote_client_count
                    .map(|count| count.to_string())
                    .unwrap_or_else(|| "n/a".to_string()),
            ));
        }

        app.push_display_message(DisplayMessage::system(context_report).with_title("Context"));
        return true;
    }

    false
}
