//! Persistent cross-session prompt history plus the Ctrl+R reverse search
//! overlay.
//!
//! Every submitted prompt is recorded to `~/.jcode/prompt-history.jsonl`
//! (JSONL, one JSON-encoded string per line, append-only with periodic
//! compaction). Recording dedupes: resubmitting an existing prompt moves it to
//! the most-recent slot instead of storing a second copy. Up/Down prompt
//! recall (`input::handle_prompt_history_navigation`) walks the merged
//! history, so prompts from previous sessions are reachable after the current
//! session's own prompts. Ctrl+R (or Cmd+R) opens a fuzzy reverse search over
//! the same merged history.

use super::App;
use crossterm::event::{KeyCode, KeyModifiers};
use std::path::{Path, PathBuf};

/// Hard cap on persisted history entries (after dedupe, newest kept).
pub(crate) const MAX_PERSISTED_PROMPTS: usize = 1000;
/// Prompts longer than this are not recorded (giant pastes are not useful as
/// recallable history and bloat the file).
const MAX_RECORDED_PROMPT_LEN: usize = 10_000;
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

pub(crate) fn history_file_path() -> Option<PathBuf> {
    crate::storage::jcode_dir()
        .ok()
        .map(|dir| dir.join("prompt-history.jsonl"))
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
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_RECORDED_PROMPT_LEN {
        return;
    }
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
    let Ok(line) = serde_json::to_string(trimmed) else {
        return;
    };
    use std::io::Write;
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        Ok(mut file) => {
            if let Err(error) = writeln!(file, "{}", line) {
                crate::logging::warn(&format!("prompt history: append failed: {}", error));
                return;
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
    /// Lazily load the persisted prompt history. Under `cfg(test)` the load is
    /// skipped (tests inject `persisted_prompt_history` directly) so unit tests
    /// sharing one `JCODE_HOME` stay deterministic.
    fn ensure_persisted_prompt_history_loaded(&mut self) {
        if self.persisted_prompt_history.is_some() {
            return;
        }
        let loaded = if cfg!(test) {
            Vec::new()
        } else {
            history_file_path()
                .map(|path| load_from_path(&path))
                .unwrap_or_default()
        };
        self.persisted_prompt_history = Some(loaded);
    }

    /// Full recall history: persisted prompts from previous sessions (oldest
    /// first) followed by this session's visible prompts, deduped keeping the
    /// most recent occurrence so each prompt appears exactly once.
    pub(super) fn merged_prompt_history(&mut self) -> Vec<String> {
        self.ensure_persisted_prompt_history_loaded();
        let mut combined: Vec<String> = self
            .persisted_prompt_history
            .as_deref()
            .unwrap_or_default()
            .to_vec();
        combined.extend(
            self.display_messages
                .iter()
                .filter(|message| message.role == "user")
                .map(|message| message.content.trim().to_string())
                .filter(|content| !content.is_empty()),
        );
        dedupe_keep_last(combined)
    }

    /// Record a submitted prompt into the persistent history. Only new
    /// content is stored: resubmitting an existing prompt moves it to the
    /// most-recent slot instead of adding a duplicate. No-op for empty/huge
    /// prompts and while a login/account/ssh input interception is pending
    /// (those inputs can contain secrets).
    pub(super) fn record_prompt_history(&mut self, text: &str) {
        if self.pending_login.is_some()
            || self.pending_account_input.is_some()
            || self.pending_ssh_remote_name.is_some()
        {
            return;
        }
        let trimmed = text.trim();
        if trimmed.is_empty() || trimmed.len() > MAX_RECORDED_PROMPT_LEN {
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
            && let Some(path) = history_file_path()
        {
            append_to_path(&path, trimmed);
        }
    }

    /// Open the Ctrl+R reverse history search overlay. No matches are shown
    /// until the user types a query; the current input-line draft is saved so
    /// Esc can restore it after the live preview overwrites the input.
    pub(super) fn open_prompt_history_search(&mut self) {
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
