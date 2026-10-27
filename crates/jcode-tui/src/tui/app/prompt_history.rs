//! Session-local prompt recall and the Ctrl+R reverse search overlay.
//!
//! Every submitted prompt is recorded to a session-specific JSONL journal
//! (JSONL, one JSON-encoded string per line, append-only with periodic
//! compaction). Recording dedupes: resubmitting an existing prompt moves it to
//! the most-recent slot instead of storing a second copy. Up/Down prompt
//! recall and Ctrl+R search the same journal, independent of the transcript.
//! Restoring a checkpoint never erases recall, and switching sessions never
//! exposes another session's prompts. The legacy global history is not loaded.

use super::App;
use crossterm::event::{KeyCode, KeyModifiers};
use std::path::{Path, PathBuf};

/// Hard cap on persisted history entries (after dedupe, newest kept).
pub(crate) const MAX_PERSISTED_PROMPTS: usize = 1000;
/// Compact (dedupe + cap + rewrite) the append-only file once it exceeds this
/// many lines.
const COMPACT_THRESHOLD_LINES: usize = MAX_PERSISTED_PROMPTS * 2;
/// Cap on search matches kept in the overlay state.
const MAX_SEARCH_MATCHES: usize = 50;

/// State for the Ctrl+R reverse history search overlay.
#[derive(Debug, Clone, Default)]
pub(crate) struct PromptHistorySearchState {
    /// Current filter text typed into the overlay.
    pub(crate) query: String,
    /// Selected index into `matches` (0 = newest match).
    pub(crate) selected: usize,
    /// Matching prompts, newest first, refreshed on every query change.
    /// Empty until the user types a query (readline-style: no results are
    /// shown for an empty query).
    pub(crate) matches: Vec<String>,
    /// Input-line draft captured when the search opened, restored on cancel
    /// (Esc) or when the query stops matching anything.
    pub(crate) original_input: String,
    /// Cursor position matching `original_input`.
    pub(crate) original_cursor: usize,
}

/// Ownership is separate from the transcript because SessionId can arrive
/// before History replaces the previous session's displayed messages.
#[derive(Default)]
pub(super) struct PromptHistoryScope {
    session_id: Option<String>,
    display_session_id: Option<String>,
    display_is_backfill: bool,
    sessions: std::collections::HashMap<String, Vec<String>>,
    switch_pending: bool,
}

pub(crate) fn history_file_path(session_id: &str) -> Option<PathBuf> {
    if crate::tui::is_ssh_remote() {
        // Keep recall in-memory for this attachment, not mixed with local hosts.
        return None;
    }
    crate::storage::jcode_dir()
        .ok()
        // Encode the id instead of trusting it as a path component.
        .map(|dir| {
            dir.join("prompt-history")
                .join(format!("{}.jsonl", hex::encode(session_id)))
        })
}

/// Load persisted prompts (oldest → newest), deduped keeping the most recent
/// occurrence, capped to [`MAX_PERSISTED_PROMPTS`].
pub(crate) fn load_from_path(path: &Path) -> Vec<String> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let entries: Vec<String> = content
        .lines()
        .filter_map(|line| serde_json::from_str::<String>(line).ok())
        .filter(|entry| !entry.trim().is_empty())
        .collect();
    let mut deduped = dedupe_keep_last(entries);
    if deduped.len() > MAX_PERSISTED_PROMPTS {
        let overflow = deduped.len() - MAX_PERSISTED_PROMPTS;
        deduped.drain(..overflow);
    }
    deduped
}

/// Append one prompt to the history file, compacting when the append-only
/// file grows past [`COMPACT_THRESHOLD_LINES`]. Appends (rather than
/// rewriting) so concurrent jcode processes do not clobber each other;
/// dedupe happens at load and compaction time.
pub(crate) fn append_to_path(path: &Path, text: &str) {
    append_prompts_to_path(path, std::iter::once(text));
}

/// Re-emit an ordered snapshot after historical backfill. Loading dedupes by
/// last occurrence, so disk order matches memory without overwriting another
/// client's appended prompts.
pub(super) fn append_snapshot_to_path(path: &Path, entries: &[String]) {
    append_prompts_to_path(path, entries.iter().map(String::as_str));
}

fn append_prompts_to_path<'a>(path: &Path, entries: impl IntoIterator<Item = &'a str>) {
    let mut entries = entries
        .into_iter()
        .map(str::trim)
        .filter(|text| !text.is_empty());
    let Some(first) = entries.next() else {
        return;
    };
    if let Some(parent) = path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        crate::logging::warn(&format!(
            "prompt history: failed to create {}: {}",
            parent.display(),
            error
        ));
        return;
    }
    use std::io::Write;
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        Ok(mut file) => {
            for text in std::iter::once(first).chain(entries) {
                let Ok(line) = serde_json::to_string(text) else {
                    continue;
                };
                if let Err(error) = writeln!(file, "{}", line) {
                    crate::logging::warn(&format!("prompt history: append failed: {}", error));
                    return;
                }
            }
        }
        Err(error) => {
            crate::logging::warn(&format!(
                "prompt history: cannot open {}: {}",
                path.display(),
                error
            ));
            return;
        }
    }
    maybe_compact(path);
}

fn maybe_compact(path: &Path) {
    let Ok(content) = std::fs::read_to_string(path) else {
        return;
    };
    if content.lines().count() <= COMPACT_THRESHOLD_LINES {
        return;
    }
    let entries = load_from_path(path);
    let mut out = String::new();
    for entry in &entries {
        if let Ok(line) = serde_json::to_string(entry) {
            out.push_str(&line);
            out.push('\n');
        }
    }
    let tmp = path.with_extension("jsonl.tmp");
    if let Err(error) = std::fs::write(&tmp, out) {
        crate::logging::warn(&format!("prompt history: compact write failed: {}", error));
        return;
    }
    if let Err(error) = std::fs::rename(&tmp, path) {
        crate::logging::warn(&format!("prompt history: compact rename failed: {}", error));
    }
}

/// Dedupe keeping the last occurrence of each entry, preserving order.
fn dedupe_keep_last(entries: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut reversed: Vec<String> = Vec::with_capacity(entries.len());
    for entry in entries.into_iter().rev() {
        if seen.insert(entry.clone()) {
            reversed.push(entry);
        }
    }
    reversed.reverse();
    reversed
}

/// Seed a restored transcript without treating reconstructed historical text
/// as newly submitted. Known journal entries keep their submission recency;
/// unknown runs are placed beside their chronological transcript anchors.
fn backfill_history(history: &[String], transcript: Vec<String>) -> Vec<String> {
    let positions: std::collections::HashMap<&str, usize> = history
        .iter()
        .enumerate()
        .map(|(index, prompt)| (prompt.as_str(), index))
        .collect();
    let mut insertions = vec![Vec::new(); history.len() + 1];
    let mut pending = Vec::new();
    let mut previous = None;
    for prompt in dedupe_keep_last(transcript) {
        if let Some(&index) = positions.get(prompt.as_str()) {
            insertions[index].append(&mut pending);
            previous = Some(index);
        } else {
            pending.push(prompt);
        }
    }
    // A shortened rewind snapshot may have no known anchor at all. Its
    // unknown text predates, rather than supersedes, submitted journal entries.
    insertions[previous.map_or(0, |index| index + 1)].append(&mut pending);
    let mut merged = Vec::new();
    for (index, entries) in insertions.into_iter().enumerate() {
        merged.extend(entries);
        if let Some(prompt) = history.get(index) {
            merged.push(prompt.clone());
        }
    }
    if merged.len() > MAX_PERSISTED_PROMPTS {
        merged.drain(..merged.len() - MAX_PERSISTED_PROMPTS);
    }
    merged
}

/// Collapse a prompt to a single display line for the search overlay.
fn single_line_preview(text: &str) -> String {
    let mut line: String = text
        .split('\n')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ⏎ ");
    if line.chars().count() > 160 {
        line = line.chars().take(159).collect();
        line.push('…');
    }
    line
}

impl App {
    fn prompt_history_session_id(&self) -> String {
        if self.is_remote {
            self.remote_session_id
                .as_ref()
                .or(self.resume_session_id.as_ref())
                .unwrap_or(&self.session.id)
                .clone()
        } else {
            self.session.id.clone()
        }
    }

    /// Bind recall before changing the remote session id. The displayed
    /// transcript remains owned by the previous session until replaced.
    pub(super) fn activate_prompt_history(&mut self, session_id: &str) {
        if self.prompt_history_scope.session_id.as_deref() == Some(session_id) {
            return;
        }
        if let Some(previous) = self.prompt_history_scope.session_id.take() {
            if let Some(history) = self.persisted_prompt_history.take() {
                self.prompt_history_scope.sessions.insert(previous, history);
            }
            // A search preview or saved history draft must never cross sessions.
            self.cancel_prompt_history_search();
            self.history_draft = None;
        } else {
            // The first binding owns the initial transcript and any test fixture.
            self.prompt_history_scope.display_session_id = Some(session_id.to_string());
        }
        let history = self
            .prompt_history_scope
            .sessions
            .remove(session_id)
            .or_else(|| self.persisted_prompt_history.take())
            .unwrap_or_else(|| {
                if cfg!(test) {
                    Vec::new()
                } else {
                    history_file_path(session_id)
                        .map(|path| load_from_path(&path))
                        .unwrap_or_default()
                }
            });
        self.persisted_prompt_history = Some(history);
        self.prompt_history_scope.session_id = Some(session_id.to_string());
    }

    fn ensure_persisted_prompt_history_loaded(&mut self) {
        let session_id = self.prompt_history_session_id();
        self.activate_prompt_history(&session_id);
    }

    /// The first server id names the provisional startup session. Subsequent
    /// ids are real switches, even when the old transcript is still on screen.
    pub(super) fn set_remote_prompt_history_session(&mut self, session_id: &str) {
        self.capture_visible_prompt_history();
        let adopting_startup = self.remote_session_id.is_none() && self.resume_session_id.is_none();
        let startup_prompts = adopting_startup
            .then(|| self.persisted_prompt_history.clone().unwrap_or_default())
            .unwrap_or_default();
        self.activate_prompt_history(session_id);
        self.remote_session_id = Some(session_id.to_string());
        self.prompt_history_scope.switch_pending = false;
        if adopting_startup {
            self.prompt_history_scope.display_session_id = Some(session_id.to_string());
            for prompt in startup_prompts {
                self.record_prompt_history(&prompt);
            }
        }
    }

    /// A requested switch has not received SessionId/History yet. Do not let
    /// Up or Ctrl+R reveal the source session during that gap.
    pub(super) fn suspend_prompt_history_for_switch(&mut self) {
        self.capture_visible_prompt_history();
        self.cancel_prompt_history_search();
        self.history_draft = None;
        self.prompt_history_scope.switch_pending = true;
    }

    pub(super) fn cancel_prompt_history_switch(&mut self) {
        self.prompt_history_scope.switch_pending = false;
    }

    /// Import restored/remote prompts once, without replaying older transcript
    /// entries over the journal's more recent (possibly rewound) submissions.
    pub(super) fn capture_visible_prompt_history(&mut self) {
        self.ensure_persisted_prompt_history_loaded();
        if self.prompt_history_scope.display_session_id != self.prompt_history_scope.session_id {
            return;
        }
        if self.prompt_history_scope.display_is_backfill {
            self.backfill_visible_prompt_history();
            return;
        }
        let prompts: Vec<String> = self
            .display_messages
            .iter()
            .filter(|message| message.role == "user")
            .map(|message| message.content.clone())
            .collect();
        for prompt in prompts {
            self.import_prompt_history(&prompt);
        }
    }

    /// Re-rendering retained messages (notably local checkpoint restore) is
    /// not a resubmission and must not move them ahead of removed prompts.
    pub(super) fn import_prompt_history(&mut self, text: &str) {
        self.ensure_persisted_prompt_history_loaded();
        if !self
            .persisted_prompt_history
            .as_ref()
            .is_some_and(|history| history.iter().any(|entry| entry == text.trim()))
        {
            self.store_prompt_history(text);
        }
    }

    /// Called only after the old transcript has been cleared/replaced.
    pub(super) fn adopt_prompt_history_display(&mut self) {
        self.ensure_persisted_prompt_history_loaded();
        self.prompt_history_scope.display_session_id = self.prompt_history_scope.session_id.clone();
        self.prompt_history_scope.display_is_backfill = true;
        self.backfill_visible_prompt_history();
    }

    /// Replacement/restoration is historical seeding, unlike a live user row
    /// passed through push_display_message or an explicit submitted prompt.
    fn backfill_visible_prompt_history(&mut self) {
        if self.pending_login.is_some()
            || self.pending_account_input.is_some()
            || self.pending_ssh_remote_name.is_some()
            || self.prompt_history_scope.display_session_id != self.prompt_history_scope.session_id
        {
            return;
        }
        let transcript = self
            .display_messages
            .iter()
            .filter(|message| message.role == "user")
            .map(|message| message.content.trim().to_string())
            .filter(|text| !text.is_empty() && !text.starts_with('/') && !text.starts_with('!'))
            .collect();
        let history = self.persisted_prompt_history.as_deref().unwrap_or_default();
        let merged = backfill_history(history, transcript);
        if merged == history {
            return;
        }
        self.persisted_prompt_history = Some(merged);
        if !cfg!(test)
            && let Some(path) = history_file_path(&self.prompt_history_session_id())
        {
            append_snapshot_to_path(
                &path,
                self.persisted_prompt_history.as_deref().unwrap_or_default(),
            );
        }
    }

    /// Recall is independent of transcript truncation and strictly session-local.
    pub(super) fn merged_prompt_history(&mut self) -> Vec<String> {
        if self.prompt_history_scope.switch_pending {
            return Vec::new();
        }
        self.capture_visible_prompt_history();
        self.persisted_prompt_history.clone().unwrap_or_default()
    }

    /// Record a submitted prompt into the persistent history. Only new
    /// content is stored: resubmitting an existing prompt moves it to the
    /// most-recent slot instead of adding a duplicate. No-op for empty
    /// prompts and while a login/account/ssh input interception is pending
    /// (those inputs can contain secrets).
    pub(super) fn record_prompt_history(&mut self, text: &str) {
        // Skill/slash expansion can update an existing user display message
        // after it was inserted. Seed that older text before this submission,
        // not later during rewind when it would incorrectly become newest.
        self.capture_visible_prompt_history();
        self.store_prompt_history(text);
    }

    /// Low-level recording for imports. Must not scan the transcript again.
    fn store_prompt_history(&mut self, text: &str) {
        if self.pending_login.is_some()
            || self.pending_account_input.is_some()
            || self.pending_ssh_remote_name.is_some()
        {
            return;
        }
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        // Slash commands and input-line shell commands are not conversational
        // prompts; keep them out so Up-arrow recall semantics stay unchanged
        // (session recall only ever showed user display messages).
        if trimmed.starts_with('/') || trimmed.starts_with('!') {
            return;
        }
        self.ensure_persisted_prompt_history_loaded();
        let Some(history) = self.persisted_prompt_history.as_mut() else {
            return;
        };
        let already_most_recent = history.last().is_some_and(|last| last == trimmed);
        history.retain(|prompt| prompt != trimmed);
        history.push(trimmed.to_string());
        if history.len() > MAX_PERSISTED_PROMPTS {
            let overflow = history.len() - MAX_PERSISTED_PROMPTS;
            history.drain(..overflow);
        }
        // Disk writes are skipped in unit tests (shared JCODE_HOME) and when
        // the prompt is already the newest entry (no state change to persist).
        if !already_most_recent
            && !cfg!(test)
            && let Some(path) = history_file_path(&self.prompt_history_session_id())
        {
            append_to_path(&path, trimmed);
        }
    }

    /// Open the Ctrl+R reverse history search overlay. No matches are shown
    /// until the user types a query; the current input-line draft is saved so
    /// Esc can restore it after the live preview overwrites the input.
    pub(super) fn open_prompt_history_search(&mut self) {
        self.capture_visible_prompt_history();
        self.prompt_history_search = Some(PromptHistorySearchState {
            original_input: self.input.clone(),
            original_cursor: self.cursor_pos,
            ..PromptHistorySearchState::default()
        });
    }

    fn refresh_prompt_history_search_matches(&mut self) {
        let history = self.merged_prompt_history();
        let Some(state) = self.prompt_history_search.as_mut() else {
            return;
        };
        let query = state.query.trim().to_string();
        // Readline-style: an empty query matches nothing (the overlay only
        // shows results once the user starts typing).
        let mut matches: Vec<String> = if query.is_empty() {
            Vec::new()
        } else {
            let mut scored: Vec<(i32, usize, String)> = history
                .into_iter()
                .enumerate()
                .filter_map(|(recency, prompt)| {
                    // Free-form matcher: the query may match anywhere in the
                    // prompt (the slash-command scorer requires an anchored
                    // start and misses mid-prompt words).
                    jcode_fuzzy::fuzzy_score(&query, &prompt).map(|score| (score, recency, prompt))
                })
                .collect();
            // Best fuzzy score first; ties broken by recency (newest first).
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
            scored.into_iter().map(|(_, _, prompt)| prompt).collect()
        };
        matches.truncate(MAX_SEARCH_MATCHES);
        state.selected = state.selected.min(matches.len().saturating_sub(1));
        state.matches = matches;
        self.apply_prompt_history_search_preview();
    }

    /// Live-preview the selected match in the input line (readline-style).
    /// Falls back to the saved draft when nothing matches.
    fn apply_prompt_history_search_preview(&mut self) {
        let Some(state) = self.prompt_history_search.as_ref() else {
            return;
        };
        match state.matches.get(state.selected) {
            Some(prompt) => {
                self.input = prompt.clone();
                self.cursor_pos = self.input.len();
            }
            None => {
                self.input = state.original_input.clone();
                self.cursor_pos = state.original_cursor;
            }
        }
    }

    /// Close the search overlay and restore the input-line draft that was
    /// active when it opened.
    fn cancel_prompt_history_search(&mut self) {
        if let Some(state) = self.prompt_history_search.take() {
            self.input = state.original_input;
            self.cursor_pos = state.original_cursor;
        }
    }

    /// Handle a key event while the history search overlay is open.
    /// Consumes every key until the overlay closes (Enter/Esc).
    pub(super) fn handle_prompt_history_search_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) {
        let ctrl = modifiers.contains(KeyModifiers::CONTROL);
        match code {
            KeyCode::Esc => {
                self.cancel_prompt_history_search();
            }
            KeyCode::Char('g') | KeyCode::Char('c') | KeyCode::Char('d') if ctrl => {
                self.cancel_prompt_history_search();
            }
            KeyCode::Enter => {
                let selected = self
                    .prompt_history_search
                    .as_ref()
                    .and_then(|state| state.matches.get(state.selected).cloned());
                self.prompt_history_search = None;
                if let Some(prompt) = selected {
                    self.input = prompt;
                    self.cursor_pos = self.input.len();
                    self.reset_tab_completion();
                    self.sync_model_picker_preview_from_input();
                }
            }
            // Up (or Ctrl+R again, readline-style) steps to an older match.
            KeyCode::Up => self.step_prompt_history_search(1),
            KeyCode::Char('r') if ctrl => self.step_prompt_history_search(1),
            KeyCode::Down => self.step_prompt_history_search(-1),
            KeyCode::Backspace => {
                if let Some(state) = self.prompt_history_search.as_mut() {
                    state.query.pop();
                }
                self.refresh_prompt_history_search_matches();
            }
            _ => {
                if let Some(text) = super::input::text_input_for_key(code, modifiers) {
                    if let Some(state) = self.prompt_history_search.as_mut() {
                        state.query.push_str(&text);
                        state.selected = 0;
                    }
                    self.refresh_prompt_history_search_matches();
                }
            }
        }
    }

    fn step_prompt_history_search(&mut self, delta: i64) {
        let Some(state) = self.prompt_history_search.as_mut() else {
            return;
        };
        if state.matches.is_empty() {
            return;
        }
        let max = state.matches.len() as i64 - 1;
        state.selected = (state.selected as i64 + delta).clamp(0, max) as usize;
        self.apply_prompt_history_search_preview();
    }

    /// Render-friendly snapshot of the search overlay for the UI layer.
    pub(crate) fn prompt_history_search_view(&self) -> Option<crate::tui::PromptHistorySearchView> {
        let state = self.prompt_history_search.as_ref()?;
        Some(crate::tui::PromptHistorySearchView {
            query: state.query.clone(),
            matches: state
                .matches
                .iter()
                .map(|prompt| single_line_preview(prompt))
                .collect(),
            selected: state.selected,
        })
    }
}
