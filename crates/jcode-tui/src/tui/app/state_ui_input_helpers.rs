use super::*;
use crate::tui::core;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Clone, Copy)]
pub(super) struct RegisteredCommand {
    name: &'static str,
    help: &'static str,
    hidden: bool,
}

impl RegisteredCommand {
    /// Slash command name including the leading `/`.
    pub(super) fn name(&self) -> &'static str {
        self.name
    }

    const fn public(name: &'static str, help: &'static str) -> Self {
        Self {
            name,
            help,
            hidden: false,
        }
    }

    const fn remote(name: &'static str, help: &'static str) -> Self {
        Self {
            name,
            help,
            hidden: false,
        }
    }

    const fn hidden(name: &'static str, help: &'static str) -> Self {
        Self {
            name,
            help,
            hidden: true,
        }
    }
}

pub(super) const REGISTERED_COMMANDS: &[RegisteredCommand] = &[
    RegisteredCommand::public("/help", "Show help and keyboard shortcuts"),
    RegisteredCommand::public("/?", "Show help and keyboard shortcuts"),
    RegisteredCommand::public("/commands", "Alias for /help"),
    RegisteredCommand::public("/model", "List or switch models"),
    RegisteredCommand::public("/models", "Alias for /model"),
    RegisteredCommand::public(
        "/provider-test-coverage",
        "Show live-test evidence for the current provider/model",
    ),
    RegisteredCommand::hidden("/model-status", "Alias for /provider-test-coverage"),
    RegisteredCommand::public("/refresh-model-list", "Refresh provider model catalogs"),
    RegisteredCommand::public("/agents", "Configure models for agent roles"),
    RegisteredCommand::public("/subagent", "Launch a subagent manually"),
    RegisteredCommand::public("/observe", "Show the latest tool context in the side panel"),
    RegisteredCommand::public(
        "/todos",
        "Show the current session todo list in the side panel",
    ),
    RegisteredCommand::public("/splitview", "Mirror the current chat in the side panel"),
    RegisteredCommand::public("/split-view", "Alias for /splitview"),
    RegisteredCommand::public("/btw", "Ask a side question in the side panel"),
    RegisteredCommand::public("/ssh", "Connect to a remote machine using system SSH"),
    RegisteredCommand::public("/git", "Show git status for the session working directory"),
    RegisteredCommand::public("/commit", "Make logical commits from current changes"),
    RegisteredCommand::public("/transcript", "Open the current session transcript file"),
    RegisteredCommand::public("/subagent-model", "Show/change subagent model policy"),
    RegisteredCommand::public("/autoreview", "Show/toggle automatic end-of-turn review"),
    RegisteredCommand::public("/autojudge", "Show/toggle automatic end-of-turn judging"),
    RegisteredCommand::public("/review", "Launch a one-shot headed review session"),
    RegisteredCommand::public("/judge", "Launch a one-shot headed judge session"),
    RegisteredCommand::public("/effort", "Show/change reasoning effort (Alt+left/right)"),
    RegisteredCommand::public("/fast", "Toggle fast mode"),
    RegisteredCommand::public("/transport", "Show/change connection transport"),
    RegisteredCommand::public("/alignment", "Show/change default text alignment"),
    RegisteredCommand::public("/clear", "Clear conversation history"),
    RegisteredCommand::public("/rewind", "Rewind conversation to previous message"),
    RegisteredCommand::public(
        "/history",
        "Input history: list, load N, search, delete N, clear",
    ),
    RegisteredCommand::public("/poke", "Poke model to resume with incomplete todos"),
    RegisteredCommand::public("/plan", "Create a plan-only response in the side panel"),
    RegisteredCommand::public("/improve", "Autonomously improve the repository"),
    RegisteredCommand::public("/refactor", "Run a safe refactor loop"),
    RegisteredCommand::public("/compact", "Compact context"),
    RegisteredCommand::public("/dcp", "DCP: context, stats, sweep, manual on|off"),
    RegisteredCommand::public("/fix", "Recover when the model cannot continue"),
    RegisteredCommand::public("/dictate", "Run configured external dictation command"),
    RegisteredCommand::public("/dictation", "Alias for /dictate"),
    RegisteredCommand::public("/memory", "Toggle memory feature"),
    RegisteredCommand::public("/test", "Verify a claim/current changes with layered tests"),
    RegisteredCommand::public(
        "/initiatives",
        "Open initiatives overview / resume tracked initiatives",
    ),
    RegisteredCommand::public("/goals", "Legacy alias for /initiatives"),
    RegisteredCommand::public("/swarm", "Toggle swarm feature"),
    RegisteredCommand::public("/overnight", "Run a supervised overnight coordinator"),
    RegisteredCommand::public("/context", "Show the full session context snapshot"),
    RegisteredCommand::public("/version", "Show current version"),
    RegisteredCommand::public("/changelog", "Show recent changes in this build"),
    RegisteredCommand::public("/info", "Show session info and tokens"),
    RegisteredCommand::public("/usage", "Show connected provider usage limits"),
    RegisteredCommand::public("/feedback", "Send feedback about jcode"),
    RegisteredCommand::public("/subscription", "Show jcode subscription status"),
    RegisteredCommand::public("/config", "Show or edit configuration"),
    RegisteredCommand::public("/log", "Mark the current location in the jcode logs"),
    RegisteredCommand::public(
        "/diff",
        "Cycle or set diff display mode (off/inline/full/pinned/file)",
    ),
    RegisteredCommand::public(
        "/onboarding-preview",
        "Preview the first-run onboarding screen",
    ),
    RegisteredCommand::public("/reload", "Reload into newest available binary"),
    RegisteredCommand::public("/restart", "Restart with current binary"),
    RegisteredCommand::public("/rebuild", "Background rebuild and auto reload"),
    RegisteredCommand::public("/selfdev", "Open a new self-dev jcode session"),
    RegisteredCommand::public("/update", "Background update and auto reload"),
    RegisteredCommand::public("/resume", "Open session picker"),
    RegisteredCommand::public("/sessions", "Alias for /resume"),
    RegisteredCommand::public("/session", "Alias for /resume"),
    RegisteredCommand::public("/catchup", "Open Catch Up picker"),
    RegisteredCommand::public("/back", "Return to the previous Catch Up session"),
    RegisteredCommand::public("/save", "Bookmark session for easy access"),
    RegisteredCommand::public("/unsave", "Remove bookmark from session"),
    RegisteredCommand::public("/rename", "Rename current session"),
    RegisteredCommand::public("/export", "Export this session to a Markdown or JSON file"),
    RegisteredCommand::public(
        "/share",
        "Upload this session as a private GitHub gist (requires gh CLI)",
    ),
    RegisteredCommand::public(
        "/plan",
        "Toggle plan mode (read-only — agent drafts a plan instead of executing)",
    ),
    RegisteredCommand::public(
        "/fork",
        "Fork this session into a new branch with the same history",
    ),
    RegisteredCommand::public(
        "/settings",
        "Show active jcode config (provider, features, tools, flags)",
    ),
    RegisteredCommand::public(
        "/doctor",
        "Print a quick environment diagnostic (build, flags, providers, health)",
    ),
    RegisteredCommand::public("/split", "Split session into a new window"),
    RegisteredCommand::public("/transfer", "Compact context into a fresh handoff session"),
    RegisteredCommand::public("/workspace", "Niri-style session workspace"),
    RegisteredCommand::public("/quit", "Exit jcode"),
    RegisteredCommand::public("/auth", "Show authentication status"),
    RegisteredCommand::public("/login", "Login to a provider"),
    RegisteredCommand::public("/logout", "Log out of a provider"),
    RegisteredCommand::public("/account", "Open the combined account picker"),
    RegisteredCommand::public("/accounts", "Alias for /account"),
    RegisteredCommand::public("/cache", "Show cache stats or set cache TTL"),
    RegisteredCommand::public("/debug-visual", "Toggle visual debug overlay"),
    RegisteredCommand::public("/screenshot-mode", "Toggle screenshot capture mode"),
    RegisteredCommand::public("/screenshot", "Capture a screenshot debug state"),
    RegisteredCommand::public("/record", "Record a demo capture"),
    RegisteredCommand::remote("/client-reload", "Force reload client binary"),
    RegisteredCommand::remote("/server-reload", "Force reload server binary"),
    RegisteredCommand::hidden("/z", "Secret premium-mode command"),
    RegisteredCommand::hidden("/zz", "Secret premium-mode command"),
    RegisteredCommand::hidden("/zzz", "Secret premium-mode command"),
    RegisteredCommand::hidden("/zstatus", "Secret premium-mode status command"),
];

/// Detect whether the input ends in an "active `$` token" — i.e. the
/// user typed `$` then alphanumeric/dash/underscore characters, and is
/// still on that token (no whitespace yet after `$`). Returns the
/// substring starting from the `$` (e.g. `"$gri"`) so the caller can
/// rank skill candidates against it.
///
/// Returns `None` if:
/// - There's no `$` in the input
/// - The most recent `$` is not at a token start (preceded by alphanumeric)
/// - There's whitespace between the `$` and the end of input
///
/// Examples:
///
/// ```ignore
/// active_dollar_token("$grill-me")          // Some("$grill-me")
/// active_dollar_token("fix the auth $gri")  // Some("$gri")
/// active_dollar_token("xxx $")              // Some("$")
/// active_dollar_token("xxx $a $b")          // Some("$b")  — last token wins
/// active_dollar_token("xxx $a hello")       // None       — token ended
/// active_dollar_token("price = $100")       // None       — preceded by '=' which is OK,
///                                                          // but we accept it
/// active_dollar_token("hello world")        // None
/// ```
pub(super) fn active_dollar_token(input: &str) -> Option<&str> {
    // Walk from the END backwards to find the most recent '$'. While
    // walking we must NOT cross whitespace (whitespace = token boundary
    // and we'd be in a different token already).
    let bytes = input.as_bytes();
    let mut i = bytes.len();
    while i > 0 {
        let prev = bytes[i - 1];
        if prev == b'$' {
            // Found a `$`. Verify it starts a token — i.e. char before
            // is start-of-input or whitespace.
            if i == 1 || (bytes[i - 2] as char).is_whitespace() {
                return Some(&input[i - 1..]);
            }
            // `$` is in the middle of an identifier (e.g. "abc$xyz") —
            // not a skill token.
            return None;
        }
        if (prev as char).is_whitespace() {
            // Crossed whitespace before finding `$` — no active token.
            return None;
        }
        i -= 1;
    }
    None
}

/// Result of `extract_at_token_at_cursor`: a single `@<path>` token under
/// the cursor, with byte offsets so callers can do exact replacement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AtTokenMatch<'a> {
    /// The text after `@`, with surrounding quotes stripped if quoted.
    /// Example: `@"foo bar"` → `"foo bar"`.
    pub query: &'a str,
    /// Byte offset of the `@` symbol in `input`.
    pub start: usize,
    /// Exclusive byte offset where the token ends. For unquoted tokens this
    /// is the first whitespace (or end of input). For quoted tokens it
    /// includes the closing `"` if present.
    pub end: usize,
    /// `true` if the token uses the `@"..."` quoted form.
    pub is_quoted: bool,
}

/// Detect if `cursor` (byte offset) is inside an active `@<path>` token at
/// any position in `input` (start, middle, or end).
///
/// Rules:
///   - The `@` must be at the start of input or preceded by whitespace.
///   - The cursor must lie within `[at_pos .. token_end]` inclusive on the
///     left, exclusive on the right (cursor in trailing whitespace → None).
///   - Quoted form `@"..."` is supported: token continues through spaces
///     until the matching closing `"` or end of input. Cursor anywhere
///     inside the quotes (or on the `@`) is "inside the token".
///   - Email-like `user@example.com` → None (the `@` is preceded by an
///     alphanumeric char, not whitespace).
///
/// `cursor` is clamped to `input.len()` if out of range. UTF-8 boundary
/// checks ensure we never split a multibyte char.
pub(super) fn extract_at_token_at_cursor(input: &str, cursor: usize) -> Option<AtTokenMatch<'_>> {
    let cursor = cursor.min(input.len());
    let bytes = input.as_bytes();

    // --- 1. Walk backward from cursor to find the most recent `@`
    //        that satisfies the start-of-token rule. ----------------
    let mut at_idx: Option<usize> = None;
    let mut i = cursor;
    while i > 0 {
        // Step back to the previous char boundary.
        let prev = i - 1;
        // Quick check: only inspect ASCII bytes. UTF-8 multibyte chars
        // never contain a 0x40 ('@'), 0x22 ('"'), or whitespace ASCII byte
        // in continuation positions, so this is safe.
        let b = bytes[prev];
        if b == b'@' {
            // Verify start condition: pos==0 or preceded by whitespace.
            if prev == 0 {
                at_idx = Some(prev);
                break;
            }
            // Look at the char before the `@`.
            let before = bytes[prev - 1];
            if (before as char).is_ascii_whitespace() {
                at_idx = Some(prev);
                break;
            }
            // `@` mid-word (like `user@example.com`) — not a token start.
            return None;
        }
        // For the *unquoted* part of the search (cursor → @), if we hit
        // whitespace we know there's no active token — the cursor is in a
        // gap or in a non-@ word.
        //
        // BUT: we still need to handle quoted tokens that contain spaces.
        // We can't tell from this side whether a space is inside `@"..."`
        // or outside. So a hit here doesn't immediately disqualify; we
        // also try the quoted-detection path below.
        if (b as char).is_ascii_whitespace() {
            // Save where we hit whitespace; quoted detection picks up
            // from before this point.
            break;
        }
        i = prev;
    }

    // --- 2. If we found an unquoted-style `@`, parse forward. -----
    if let Some(start) = at_idx {
        // Determine if this is a quoted token: `@` followed by `"`.
        let after_at = start + 1;
        if bytes.get(after_at) == Some(&b'"') {
            return parse_quoted_token(input, start, cursor);
        }
        // Plain unquoted token: include path-like chars until whitespace.
        let mut end = after_at;
        while end < bytes.len() && !(bytes[end] as char).is_ascii_whitespace() {
            end += 1;
        }
        if cursor <= end {
            // SAFETY: end is at a whitespace boundary or end-of-input;
            // both are UTF-8 safe split points.
            return Some(AtTokenMatch {
                query: &input[after_at..end],
                start,
                end,
                is_quoted: false,
            });
        }
        return None;
    }

    // --- 3. No unquoted match. Try quoted: scan back for the most -
    //        recent unmatched `@"` whose `@` qualifies as token start.
    //        This handles cursor inside a quoted token with spaces.
    let mut j = cursor;
    while j > 0 {
        let prev = j - 1;
        let b = bytes[prev];
        if b == b'@' {
            // Must be followed by `"` and pass the start-of-token rule.
            let starts_quote = bytes.get(prev + 1) == Some(&b'"');
            let valid_start = prev == 0 || (bytes[prev - 1] as char).is_ascii_whitespace();
            if starts_quote && valid_start {
                return parse_quoted_token(input, prev, cursor);
            }
            // Hit a non-quoted `@` while scanning backward — abort.
            return None;
        }
        j = prev;
    }

    None
}

/// Parse `@"..."` starting at `start` (which points at the `@`). Returns
/// `Some` only if the cursor is positioned inside the token (after the
/// `@` and not past the closing quote).
fn parse_quoted_token(input: &str, start: usize, cursor: usize) -> Option<AtTokenMatch<'_>> {
    let bytes = input.as_bytes();
    debug_assert!(bytes.get(start) == Some(&b'@'));
    debug_assert!(bytes.get(start + 1) == Some(&b'"'));

    let inner_start = start + 2;
    // Find the closing `"`, scanning forward over arbitrary bytes
    // (including spaces and UTF-8 sequences — `"` is never a continuation).
    let mut end = inner_start;
    let mut closed = false;
    while end < bytes.len() {
        if bytes[end] == b'"' {
            closed = true;
            break;
        }
        end += 1;
    }
    let token_end = if closed { end + 1 } else { end };
    if cursor > token_end {
        return None;
    }
    let inner_end = end;
    Some(AtTokenMatch {
        query: &input[inner_start..inner_end],
        start,
        end: token_end,
        is_quoted: true,
    })
}

/// Issue #11: detect an active `@<path>` token at the END of the input.
///
/// Returns the substring beginning at the most recent `@` that:
///   - starts the input, OR
///   - is preceded by whitespace
///
/// and that has not yet been terminated by whitespace.
///
/// ```text
/// active_at_token("@src/main.rs")           // Some("@src/main.rs")
/// active_at_token("look at @docs/READ")     // Some("@docs/READ")
/// active_at_token("look at @docs/RM done")  // None
/// active_at_token("email@example.com")      // None — middle of token
/// ```
#[cfg(test)]
pub(super) fn active_at_token(input: &str) -> Option<&str> {
    let bytes = input.as_bytes();
    let mut i = bytes.len();
    while i > 0 {
        let prev = bytes[i - 1];
        if prev == b'@' {
            if i == 1 || (bytes[i - 2] as char).is_whitespace() {
                return Some(&input[i - 1..]);
            }
            return None;
        }
        if (prev as char).is_whitespace() {
            return None;
        }
        i -= 1;
    }
    None
}

/// Suggest filesystem entries matching a partial `@<query>` token.
///
/// Walks the working directory shallowly, returning entries whose
/// path (relative to `cwd`) starts with `query`. Limits to `limit`
/// results.
///
/// Implementation note: this is intentionally cheap — it only walks
/// the directory containing the partial path, not the full tree.
/// `@s` looks at `cwd`. `@src/m` looks inside `cwd/src/`.
#[cfg(test)]
pub(super) fn suggest_at_path(cwd: &std::path::Path, query: &str, limit: usize) -> Vec<String> {
    let q = query.trim_start_matches('@');
    let (parent_rel, prefix) = match q.rfind('/') {
        Some(idx) => (&q[..idx + 1], &q[idx + 1..]),
        None => ("", q),
    };

    let parent_abs = if parent_rel.is_empty() {
        cwd.to_path_buf()
    } else {
        cwd.join(parent_rel)
    };

    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&parent_abs) else {
        return out;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with(prefix) {
            continue;
        }
        // Skip hidden entries unless user explicitly types `.`
        if name.starts_with('.') && !prefix.starts_with('.') {
            continue;
        }
        let mut full = format!("@{parent_rel}{name}");
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            full.push('/');
        }
        out.push(full);
        if out.len() >= limit {
            break;
        }
    }
    out.sort();
    out
}

impl App {
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
        if let Some(first_char) = n.chars().next()
            && !h.starts_with(&n[..first_char.len_utf8()])
        {
            return None;
        }
        let mut score = 0usize;
        let mut pos = 0usize;
        for ch in n.chars() {
            let idx = h[pos..].find(ch)?;
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
        if let Some(cache) = self.command_candidates_cache.borrow().as_ref() {
            return cache.candidates.clone();
        }

        // Issue #N (UX): only show BUILT-IN commands under `/`. Skills move
        // to the `$` namespace via skill_candidates() so the / autocomplete
        // dropdown stays navigable when the user has 100+ skills installed.
        // The legacy `/<skill>` invocation form still works at submit time
        // for backwards compatibility — it's just hidden from autocomplete.
        let mut seen = std::collections::HashSet::new();
        let commands: Vec<(String, &'static str)> = REGISTERED_COMMANDS
            .iter()
            .filter(|command| !command.hidden)
            .filter_map(|command| {
                let name = command.name.to_string();
                seen.insert(name.clone()).then_some((name, command.help))
            })
            .collect();

        *self.command_candidates_cache.borrow_mut() = Some(CommandCandidatesCache {
            candidates: commands.clone(),
        });
        commands
    }

    /// Build the autocomplete list for the `$` (skill) namespace.
    ///
    /// Each entry is `($<skill-name>, "Activate skill")`. Includes both
    /// locally-discovered skills (project + user dirs, see SkillRegistry)
    /// and remote-session skills when running as a TUI client against a
    /// shared server.
    fn skill_candidates(&self) -> Vec<(String, &'static str)> {
        let mut seen = std::collections::HashSet::new();
        let mut out: Vec<(String, &'static str)> = Vec::new();

        let skills = self.current_skills_snapshot();
        for skill in skills.list() {
            let entry = format!("${}", skill.name);
            if seen.insert(entry.clone()) {
                out.push((entry, "Activate skill"));
            }
        }

        if self.is_remote && !self.remote_skills.is_empty() {
            for skill in &self.remote_skills {
                let entry = format!("${skill}");
                if seen.insert(entry.clone()) {
                    out.push((entry, "Activate skill"));
                }
            }
        }

        out
    }

    pub(super) fn invalidate_command_candidates_cache(&self) {
        *self.command_candidates_cache.borrow_mut() = None;
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

    fn model_provider_suggestion_candidates(&self, model: &str) -> Vec<(String, &'static str)> {
        fn push_unique(
            seen: &mut std::collections::HashSet<String>,
            entries: &mut Vec<(String, &'static str)>,
            command: String,
            help: &'static str,
        ) {
            if !command.is_empty() && seen.insert(command.clone()) {
                entries.push((command, help));
            }
        }

        let model = model.trim();
        if model.is_empty() {
            return Vec::new();
        }
        let Some(openrouter_model) = crate::provider::openrouter_catalog_model_id(model) else {
            return Vec::new();
        };

        let mut seen = std::collections::HashSet::new();
        let mut suggestions = Vec::new();
        push_unique(
            &mut seen,
            &mut suggestions,
            format!("/model {}@auto", openrouter_model),
            "Use automatic OpenRouter provider routing",
        );

        if self.is_remote {
            let routes = if !self.remote_model_options.is_empty() {
                self.remote_model_options.clone()
            } else {
                self.build_remote_model_routes_fallback()
            };

            for route in routes {
                if route.model == model && route.api_method == "openrouter" {
                    let help = if route.provider == "auto" {
                        "Use automatic OpenRouter provider routing"
                    } else {
                        "Pin OpenRouter provider"
                    };
                    push_unique(
                        &mut seen,
                        &mut suggestions,
                        format!("/model {}@{}", openrouter_model, route.provider),
                        help,
                    );
                }
            }
        } else {
            for provider in self.provider.available_providers_for_model(model) {
                push_unique(
                    &mut seen,
                    &mut suggestions,
                    format!("/model {}@{}", openrouter_model, provider),
                    "Pin OpenRouter provider",
                );
            }
        }

        suggestions
    }

    /// Get command suggestions based on current input (or base input for cycling)
    pub(super) fn get_suggestions_for(&self, input: &str) -> Vec<(String, &'static str)> {
        let input = input.trim_start();

        // Only show suggestions when input starts with `/` (built-in
        // commands) or `$` (skills, see skill_candidates).
        // Issue: skill autocomplete should fire even when `$` appears
        // mid-text, e.g. "fix the auth $gri" → suggest skills matching
        // "gri*". Find the last `$` that starts a token (preceded by
        // start-of-input or whitespace), and if found with no whitespace
        // between it and the cursor, surface skill candidates ranked by
        // the partial after `$`.
        if let Some(token) = active_dollar_token(input) {
            return self.rank_suggestions(&token.to_lowercase(), self.skill_candidates());
        }
        if !input.starts_with('/') {
            return vec![];
        }

        let prefix = input.to_lowercase();
        let prefix_trimmed = prefix.trim_end();

        if prefix.starts_with("/model ") || prefix.starts_with("/models ") {
            if let Some(model_spec) = input
                .strip_prefix("/model ")
                .or_else(|| input.strip_prefix("/models "))
                && let Some((model, _provider_prefix)) = model_spec.rsplit_once('@')
            {
                let suggestions = self.model_provider_suggestion_candidates(model);
                if !suggestions.is_empty() {
                    return self.rank_suggestions(input, suggestions);
                }
            }

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

        if prefix.starts_with("/git ") {
            return self.rank_suggestions(
                input,
                vec![("/git status".into(), "Show branch and working tree status")],
            );
        }

        if prefix_trimmed == "/git" {
            return vec![("/git status".into(), "Show branch and working tree status")];
        }

        if prefix.starts_with("/transcript ") {
            return self.rank_suggestions(
                input,
                vec![(
                    "/transcript path".into(),
                    "Print transcript path without opening",
                )],
            );
        }

        if prefix_trimmed == "/transcript" {
            return vec![(
                "/transcript path".into(),
                "Print transcript path without opening",
            )];
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

        if prefix.starts_with("/cache ") {
            let suggestions = vec![
                ("/cache stats".into(), "Show KV cache stats"),
                ("/cache status".into(), "Alias for /cache stats"),
                ("/cache 1h".into(), "Use 1 hour cache TTL"),
                ("/cache 5m".into(), "Use 5 minute cache TTL"),
            ];
            return self.rank_suggestions(input, suggestions);
        }

        if prefix.starts_with("/login ") || prefix.starts_with("/auth ") {
            let base = if prefix.starts_with("/auth ") {
                "/auth"
            } else {
                "/login"
            };
            let mut suggestions: Vec<(String, &'static str)> = Vec::new();
            if base == "/auth" {
                suggestions.push(("/auth doctor".into(), "Diagnose provider auth issues"));
            }
            suggestions.extend(
                crate::provider_catalog::tui_login_providers()
                    .iter()
                    .map(|provider| (format!("{} {}", base, provider.id), provider.menu_detail)),
            );
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

        if prefix.starts_with("/overnight ") {
            return self.rank_suggestions(
                input,
                vec![
                    (
                        "/overnight 7".into(),
                        "Start a 7-hour supervised overnight run",
                    ),
                    (
                        "/overnight status".into(),
                        "Show latest overnight run status",
                    ),
                    ("/overnight log".into(), "Show recent overnight events"),
                    ("/overnight review".into(), "Open the generated review page"),
                    ("/overnight cancel".into(), "Request overnight cancellation"),
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
            let arg = prefix.strip_prefix("/rewind ").unwrap_or_default().trim();
            let visible_count = self.session.visible_conversation_message_count();

            // Rewind targets are 1-based visible conversation message numbers.
            // Do not fuzzy-rank numeric arguments: `/rewind 10` should never be
            // completed or preview-accepted as `/rewind 1` just because `1` is a
            // fuzzy prefix match. If a complete numeric target is present, only
            // surface the exact valid command.
            if !arg.is_empty() && arg.chars().all(|c| c.is_ascii_digit()) {
                if let Ok(n) = arg.parse::<usize>()
                    && (1..=visible_count).contains(&n)
                {
                    return vec![(format!("/rewind {}", n), "Rewind to this message")];
                }
                return Vec::new();
            }

            let suggestions = (1..=visible_count)
                .map(|n| (format!("/rewind {}", n), "Rewind to this message"))
                .collect();
            return self.rank_suggestions(input, suggestions);
        }

        self.rank_suggestions(&prefix, self.command_candidates())
    }

    /// Get command suggestions based on current input
    /// Compute `@<path>` suggestions if the cursor currently sits inside
    /// an active @-token. Returns `None` when no token is active or the
    /// picker is unavailable.
    ///
    /// Each entry is formatted as `@path` (file) or `@path/` (folder), and
    /// the help text indicates the kind. Folders sort before files within
    /// the same score band — ffs already handles this internally.
    fn at_suggestions(&self) -> Option<Vec<(String, &'static str)>> {
        let m = super::state_ui_input_helpers::extract_at_token_at_cursor(
            &self.input,
            self.cursor_pos,
        )?;

        // Lazy-init: if the picker is missing, try to build one. This
        // briefly takes a mut borrow on the RefCell.
        let picker = {
            let mut slot = self.at_picker.borrow_mut();
            slot.ensure(self.session.working_dir.as_deref())?
        };

        let raw = picker.search(m.query, super::at_picker::AT_PICKER_MAX_SUGGESTIONS);
        if raw.is_empty() {
            // Picker still warming up or query has no matches. Returning
            // an empty Vec here would suppress the dropdown and also block
            // the `/` and `$` fallbacks (which is what we want — once the
            // user types `@`, we own the dropdown).
            return Some(Vec::new());
        }

        let suggestions = raw
            .into_iter()
            .map(|s| {
                let (display, help) = if s.is_directory {
                    (format!("@{}/", s.display_path), "Folder")
                } else {
                    (format!("@{}", s.display_path), "File")
                };
                (display, help)
            })
            .collect();
        Some(suggestions)
    }

    pub fn command_suggestions(&self) -> Vec<(String, &'static str)> {
        if self
            .inline_interactive_state
            .as_ref()
            .is_some_and(|picker| picker.preview && picker.kind == crate::tui::PickerKind::Model)
        {
            let input = self.input.trim_start();
            if input.starts_with("/model") || input.starts_with("/models") {
                return Vec::new();
            }
        }

        // `@<path>` autocomplete: when the cursor sits inside an @-token
        // anywhere in the input, return ffs-search file/folder suggestions
        // formatted as `@path` (file) or `@path/` (folder, drill-in).
        // This must come BEFORE the `/` and `$` checks because `@` tokens
        // can appear mid-input and shouldn't be confused with command-mode.
        if let Some(at) = self.at_suggestions() {
            return at;
        }

        self.get_suggestions_for(&self.input)
    }

    fn clamp_command_suggestion_selection(&mut self) -> Vec<(String, &'static str)> {
        let suggestions = self.command_suggestions();
        if suggestions.is_empty() {
            self.command_suggestion_selected = 0;
        } else {
            self.command_suggestion_selected = self
                .command_suggestion_selected
                .min(suggestions.len().saturating_sub(1));
        }
        suggestions
    }

    pub(super) fn move_command_suggestion_selection(&mut self, delta: i32) -> bool {
        let suggestions = self.clamp_command_suggestion_selection();
        if suggestions.is_empty() {
            return false;
        }

        let len = suggestions.len() as i32;
        let selected = self.command_suggestion_selected as i32;
        self.command_suggestion_selected = (selected + delta).rem_euclid(len) as usize;
        true
    }

    fn arrow_modifiers_allow_command_suggestion_navigation(modifiers: KeyModifiers) -> bool {
        !modifiers.intersects(
            KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER | KeyModifiers::HYPER,
        )
    }

    pub(super) fn handle_command_suggestion_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> bool {
        if self.command_suggestions().is_empty() {
            return false;
        }

        match code {
            KeyCode::Down
                if Self::arrow_modifiers_allow_command_suggestion_navigation(modifiers) =>
            {
                self.move_command_suggestion_selection(1)
            }
            KeyCode::Up if Self::arrow_modifiers_allow_command_suggestion_navigation(modifiers) => {
                self.move_command_suggestion_selection(-1)
            }
            KeyCode::Char('j') if modifiers.contains(KeyModifiers::CONTROL) => {
                self.move_command_suggestion_selection(1)
            }
            KeyCode::Char('k') if modifiers.contains(KeyModifiers::CONTROL) => {
                self.move_command_suggestion_selection(-1)
            }
            KeyCode::Enter if modifiers.is_empty() => self.accept_selected_command_suggestion(),
            _ => false,
        }
    }

    pub(super) fn accept_selected_command_suggestion(&mut self) -> bool {
        let suggestions = self.clamp_command_suggestion_selection();
        let Some((cmd, _)) = suggestions.get(self.command_suggestion_selected).cloned() else {
            return false;
        };
        if cmd == self.input.trim() {
            return false;
        }

        self.remember_input_undo_state();
        self.input = cmd;
        self.cursor_pos = self.input.len();
        self.tab_completion_state = None;
        self.command_suggestion_selected = 0;
        true
    }

    /// Whether to show the dedicated first-run onboarding welcome screen
    /// (gray telemetry header, prominent donut, welcome text, login prompt).
    ///
    /// This is true exactly when the empty screen is showing onboarding
    /// suggestion prompts (brand-new install / unauthenticated / new user) so
    /// the welcome layout and the suggestions stay in sync.
    pub fn onboarding_welcome_active(&self) -> bool {
        if self.onboarding_preview_mode {
            return true;
        }
        // While the guided onboarding flow is driving the pre-suggestion phases
        // (model select / continue prompt), keep the welcome screen up even
        // though the server may have pushed scaffolding messages. The flow
        // renders its own body via `onboarding_welcome_kind()`.
        if self.onboarding_flow_drives_welcome() {
            return true;
        }
        if !self.display_messages.is_empty() || self.is_processing {
            return false;
        }
        !self.suggestion_prompts().is_empty()
    }

    /// What the onboarding welcome screen should render in its body, driven by
    /// the active guided flow phase. Defaults to the starter suggestion cards.
    pub fn onboarding_welcome_kind(&self) -> crate::tui::OnboardingWelcomeKind {
        use crate::tui::OnboardingWelcomeKind;
        use crate::tui::app::onboarding_flow::OnboardingPhase;
        match self.onboarding_phase() {
            Some(OnboardingPhase::Login { import }) => {
                let prompt = import.as_ref().and_then(|review| {
                    review
                        .current()
                        .map(|candidate| crate::tui::LoginImportPrompt {
                            provider_summary: candidate.provider_summary().to_string(),
                            source_name: candidate.source_name().to_string(),
                            position: review.position(),
                            total: review.total(),
                            yes_highlighted: review.yes_highlighted,
                            seconds_left: review.seconds_remaining(),
                        })
                });
                OnboardingWelcomeKind::Login { import: prompt }
            }
            Some(OnboardingPhase::TelemetryConsent {
                yes_highlighted,
                shown_at,
            }) => {
                let total = crate::tui::app::onboarding_flow::DECISION_TIMEOUT.as_secs();
                let seconds_left = total.saturating_sub(shown_at.elapsed().as_secs());
                OnboardingWelcomeKind::TelemetryConsent {
                    yes_highlighted: *yes_highlighted,
                    seconds_left,
                }
            }
            Some(OnboardingPhase::ModelSelect) => OnboardingWelcomeKind::ModelSelect,
            Some(OnboardingPhase::ContinuePrompt {
                cli,
                yes_highlighted,
                shown_at,
            }) => {
                let total = crate::tui::app::onboarding_flow::DECISION_TIMEOUT.as_secs();
                let seconds_left = total.saturating_sub(shown_at.elapsed().as_secs());
                OnboardingWelcomeKind::ContinuePrompt {
                    cli_label: cli.label().to_string(),
                    yes_highlighted: *yes_highlighted,
                    seconds_left,
                }
            }
            _ => OnboardingWelcomeKind::Suggestions,
        }
    }

    /// Whether the guided onboarding flow is in a phase that should take over
    /// the welcome screen body (model select or continue prompt). The
    /// transcript-pick phase uses the session-picker overlay instead, and the
    /// suggestions phase is the default welcome body.
    fn onboarding_flow_drives_welcome(&self) -> bool {
        use crate::tui::app::onboarding_flow::OnboardingPhase;
        matches!(
            self.onboarding_phase(),
            Some(OnboardingPhase::Login { .. })
                | Some(OnboardingPhase::TelemetryConsent { .. })
                | Some(OnboardingPhase::ModelSelect)
                | Some(OnboardingPhase::ContinuePrompt { .. })
        )
    }

    /// Get suggestion prompts for new users on the initial empty screen.
    /// Returns (label, prompt_text) pairs. Empty once user is experienced or not authenticated.
    pub fn suggestion_prompts(&self) -> Vec<(String, String)> {
        let preview_mode = self.onboarding_preview_mode;
        let is_canary = if self.is_remote {
            self.remote_is_canary.unwrap_or(self.session.is_canary)
        } else {
            self.session.is_canary
        };
        if is_canary && !preview_mode {
            return Vec::new();
        }

        let auth = crate::auth::AuthStatus::check_fast();
        if !auth.has_any_available() {
            return vec![("Log in to get started".to_string(), "/login".to_string())];
        }

        if (!self.display_messages.is_empty() || self.is_processing) && !preview_mode {
            return Vec::new();
        }

        let is_new_user = if preview_mode {
            true
        } else {
            crate::storage::jcode_dir()
                .ok()
                .and_then(|dir| {
                    let path = dir.join("setup_hints.json");
                    std::fs::read_to_string(&path).ok()
                })
                .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
                .and_then(|v| v.get("launch_count")?.as_u64())
                .map(|count| count <= 5)
                .unwrap_or(true)
        };

        if !is_new_user {
            return Vec::new();
        }

        let mut prompts = vec![
            (
                "Customize my terminal theme".to_string(),
                "Find what terminal I'm using, then change its background color to pitch black and make it slightly transparent. Apply the changes for me.".to_string(),
            ),
            (
                "Review something I've been working on".to_string(),
                "Find a recent file or project I've been working on, read through it, and give me concrete suggestions on how I could improve it.".to_string(),
            ),
        ];

        prompts.push((
            "Continue my last CLI agent session".to_string(),
            latest_external_cli_continuation_prompt().unwrap_or_else(|| {
                "Find my recent Codex or Claude Code sessions, identify the latest useful one, summarize what was happening, and continue from there.".to_string()
            }),
        ));

        prompts.push((
            "Find my social media and roast me".to_string(),
            "Find a social media platform I use, look around at my profile and posts, then give me a brutally honest roast based on what you see.".to_string(),
        ));

        prompts
    }

    /// Autocomplete current input - cycles through suggestions on repeated Tab
    /// If the cursor sits inside an active `@<path>` token AND the picker
    /// has a current selection, compute the (new_input, new_cursor) pair
    /// that would result from accepting that selection. Returns `None`
    /// otherwise so the caller can fall through to other autocomplete
    /// modes (slash, dollar).
    fn try_apply_at_completion(&self) -> Option<(String, usize)> {
        let m = super::state_ui_input_helpers::extract_at_token_at_cursor(
            &self.input,
            self.cursor_pos,
        )?;
        // Read suggestions WITHOUT mutably re-initializing the picker —
        // if the user got here by Tab they've already seen the dropdown,
        // which means init succeeded.
        let suggestions = self.command_suggestions();
        if suggestions.is_empty() {
            return None;
        }
        let selected = self
            .command_suggestion_selected
            .min(suggestions.len().saturating_sub(1));
        let (display, help) = &suggestions[selected];
        // Sanity: only proceed if this is genuinely an @-suggestion (the
        // dropdown could in theory be showing slash/dollar items if both
        // happen to coexist, but we've ordered branches so @ wins).
        if !display.starts_with('@') {
            return None;
        }
        let is_directory = *help == "Folder";

        // Strip the leading `@` from the display to get the bare path.
        let bare = display.strip_prefix('@').unwrap_or(display);
        // For folders the display already has trailing `/`; strip it so
        // we can decide quoting based on the bare path text.
        let (bare_path, _had_slash) = if is_directory && bare.ends_with('/') {
            (&bare[..bare.len() - 1], true)
        } else {
            (bare, false)
        };

        let needs_quotes = bare_path.contains(' ');
        // Build the replacement text. Folders → trailing `/` and KEEP the
        // token open for drill-in; files → trailing space, close token.
        let replacement = match (needs_quotes, is_directory) {
            (true, true) => format!("@\"{bare_path}\"/"),
            (true, false) => format!("@\"{bare_path}\" "),
            (false, true) => format!("@{bare_path}/"),
            (false, false) => format!("@{bare_path} "),
        };

        let new_input = format!(
            "{}{}{}",
            &self.input[..m.start],
            replacement,
            &self.input[m.end..]
        );
        let new_cursor = m.start + replacement.len();
        Some((new_input, new_cursor))
    }

    pub fn autocomplete(&mut self) -> bool {
        // ---- @<path> autocomplete: replace the active @-token in place. ----
        //
        // This MUST come before the slash/dollar tab-cycle logic because
        // @-tokens live in arbitrary positions inside the input (not just
        // at the start), and the existing logic assumes whole-input
        // replacement for `/` commands.
        if let Some(replacement) = self.try_apply_at_completion() {
            self.remember_input_undo_state();
            let (new_input, new_cursor) = replacement;
            self.input = new_input;
            self.cursor_pos = new_cursor;
            self.tab_completion_state = None;
            self.command_suggestion_selected = 0;
            return true;
        }

        // Get suggestions for current input
        let current_suggestions = self.get_suggestions_for(&self.input);

        // Check if we're continuing a tab cycle from a previous base
        if let Some((ref base, idx)) = self.tab_completion_state.clone() {
            let base_suggestions = self.get_suggestions_for(base);

            // If current input is in base suggestions AND there are multiple options, continue cycling
            if base_suggestions.len() > 1
                && base_suggestions.iter().any(|(cmd, _)| cmd == &self.input)
            {
                let next_index = (idx + 1) % base_suggestions.len();
                let (cmd, _) = &base_suggestions[next_index];
                self.remember_input_undo_state();
                // Same prefix-preservation fix as the fresh-cycle path below.
                if cmd.starts_with('$') {
                    if let Some(token) =
                        crate::tui::app::state_ui_input_helpers::active_dollar_token(base)
                    {
                        let prefix_len = base.len() - token.len();
                        self.input = format!("{}{cmd}", &base[..prefix_len]);
                    } else {
                        self.input = cmd.clone();
                    }
                } else {
                    self.input = cmd.clone();
                }
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
        let selected = self
            .command_suggestion_selected
            .min(current_suggestions.len().saturating_sub(1));
        let (cmd, _) = &current_suggestions[selected];
        let base = self.input.clone();
        self.remember_input_undo_state();

        // Bug fix: when the suggestion is a `$<skill>` token and the user
        // typed it mid-text (e.g. "fix the auth $gri"), replace only the
        // `$token` at the end of the input, not the whole input. Otherwise
        // "xxxx $gri" → Tab → "$grill-me" (drops "xxxx ").
        //
        // For `/` commands the input always starts with `/` so the whole-
        // input replacement is correct and unchanged.
        if cmd.starts_with('$') {
            if let Some(token) =
                crate::tui::app::state_ui_input_helpers::active_dollar_token(&self.input)
            {
                let prefix_len = self.input.len() - token.len();
                let prefix = self.input[..prefix_len].to_string();
                self.input = format!("{prefix}{cmd}");
            } else {
                self.input = cmd.clone();
            }
        } else {
            self.input = cmd.clone();
        }

        // If unique match, add trailing space for arg-accepting commands
        if current_suggestions.len() == 1 && Self::command_accepts_args(&self.input) {
            self.input.push(' ');
        }
        self.cursor_pos = self.input.len();
        self.tab_completion_state = Some((base, selected));
        self.command_suggestion_selected = 0;
        true
    }

    /// Reset tab completion state (call when user types/modifies input)
    pub fn reset_tab_completion(&mut self) {
        self.tab_completion_state = None;
        self.command_suggestion_selected = 0;
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
        // Any manual edit cancels history browsing
        self.reset_input_history_browse();
    }

    pub(super) fn clear_input_undo_history(&mut self) {
        self.input_undo_stack.clear();
    }

    pub(super) fn undo_input_change(&mut self) {
        if let Some((input, cursor_pos)) = self.input_undo_stack.pop() {
            self.input = input;
            self.cursor_pos = cursor_pos.min(self.input.len());
            self.reset_tab_completion();
            self.reset_input_history_browse();
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
                | "/git"
                | "/transcript"
                | "/observe"
                | "/todos"
                | "/splitview"
                | "/split-view"
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
                | "/poke"
                | "/memory"
                | "/test"
                | "/initiatives"
                | "/initiatives show"
                | "/goals"
                | "/goals show"
                | "/swarm"
                | "/plan"
                | "/improve"
                | "/refactor"
                | "/rewind"
                | "/history"
                | "/compact"
                | "/compact mode"
                | "/alignment"
                | "/config"
                | "/save"
                | "/rename"
                | "/cache"
        )
    }
}

#[cfg(test)]
mod dollar_token_tests {
    use super::active_dollar_token;
    use super::{active_at_token, suggest_at_path};

    #[test]
    fn detects_dollar_at_start_of_input() {
        assert_eq!(active_dollar_token("$grill-me"), Some("$grill-me"));
        assert_eq!(active_dollar_token("$"), Some("$"));
    }

    #[test]
    fn detects_dollar_after_whitespace() {
        // "fix the auth $gri" → autocomplete on $gri
        assert_eq!(active_dollar_token("fix the auth $gri"), Some("$gri"));
        // Multiple spaces
        assert_eq!(active_dollar_token("xxx   $foo"), Some("$foo"));
        // Bare dollar at the end (just typed)
        assert_eq!(active_dollar_token("xxx $"), Some("$"));
    }

    #[test]
    fn last_dollar_wins_when_multiple_tokens() {
        // $a $b → caller is on $b (last token)
        assert_eq!(active_dollar_token("$a $b"), Some("$b"));
        assert_eq!(active_dollar_token("xxx $foo $bar"), Some("$bar"));
    }

    #[test]
    fn rejects_when_token_ended_with_whitespace() {
        // User typed `$foo` then space → token boundary; no active token.
        assert_eq!(active_dollar_token("$foo "), None);
        assert_eq!(active_dollar_token("$grill-me hello"), None);
    }

    #[test]
    fn rejects_dollar_in_middle_of_identifier() {
        // Embedded `$` like "abc$xyz" is not a skill token.
        assert_eq!(active_dollar_token("abc$xyz"), None);
        assert_eq!(active_dollar_token("price=$100"), None);
    }

    #[test]
    fn returns_none_when_no_dollar() {
        assert_eq!(active_dollar_token(""), None);
        assert_eq!(active_dollar_token("hello world"), None);
        assert_eq!(active_dollar_token("/help"), None);
    }

    // ---- Bug fix: autocomplete must preserve prefix before $token ----

    #[test]
    fn active_dollar_token_prefix_len_is_correct() {
        // Verify the prefix-length arithmetic used in autocomplete.
        let input = "fix the auth $gri";
        let token = active_dollar_token(input).unwrap();
        assert_eq!(token, "$gri");
        let prefix_len = input.len() - token.len();
        assert_eq!(&input[..prefix_len], "fix the auth ");
    }

    #[test]
    fn active_dollar_token_prefix_len_for_bare_dollar() {
        let input = "xxxx $";
        let token = active_dollar_token(input).unwrap();
        assert_eq!(token, "$");
        let prefix_len = input.len() - token.len();
        assert_eq!(&input[..prefix_len], "xxxx ");
    }

    // ---- Issue #11: @<path> token detection + suggestion ----

    #[test]
    fn at_token_at_start_returns_full() {
        assert_eq!(active_at_token("@src/main.rs"), Some("@src/main.rs"));
    }

    #[test]
    fn at_token_after_whitespace_returns_token() {
        assert_eq!(active_at_token("look at @docs/READ"), Some("@docs/READ"));
    }

    #[test]
    fn at_token_terminated_by_whitespace_returns_none() {
        assert_eq!(active_at_token("look at @docs done"), None);
    }

    #[test]
    fn at_token_in_middle_of_word_is_email_like_returns_none() {
        // Likely an email address — shouldn't trigger autocomplete.
        assert_eq!(active_at_token("email@example.com"), None);
    }

    #[test]
    fn at_token_empty_after_at_still_matches() {
        assert_eq!(active_at_token("@"), Some("@"));
        assert_eq!(active_at_token("hi @"), Some("@"));
    }

    #[test]
    fn at_token_no_at_returns_none() {
        assert_eq!(active_at_token("hello world"), None);
        assert_eq!(active_at_token(""), None);
    }

    // ---- extract_at_token_at_cursor: cursor-based detection ----
    //
    // Each test uses a sentinel "|" in the input to mark cursor position.
    // Helper splits on "|" so test cases stay readable.

    fn at(input: &str) -> (String, usize) {
        let cursor = input.find('|').expect("test input must contain '|'");
        let mut s = input.to_string();
        s.remove(cursor);
        (s, cursor)
    }

    fn extract<'a>(input: &'a str, cursor: usize) -> Option<super::AtTokenMatch<'a>> {
        super::extract_at_token_at_cursor(input, cursor)
    }

    #[test]
    fn cursor_before_at_returns_none() {
        let (s, c) = at("|@src");
        assert_eq!(extract(&s, c), None);
    }

    #[test]
    fn cursor_right_after_at_at_start() {
        let (s, c) = at("@|src");
        let m = extract(&s, c).expect("active");
        // Token extends forward to end of word regardless of cursor.
        assert_eq!(m.query, "src");
        assert_eq!(m.start, 0);
        assert_eq!(m.end, 4); // "@src"
        assert!(!m.is_quoted);
    }

    #[test]
    fn cursor_at_end_of_token() {
        let (s, c) = at("@src|");
        let m = extract(&s, c).expect("active");
        assert_eq!(m.query, "src");
        assert_eq!(m.start, 0);
        assert_eq!(m.end, 4);
    }

    #[test]
    fn cursor_in_middle_of_input_inside_token() {
        let (s, c) = at("look @src| done");
        let m = extract(&s, c).expect("active");
        assert_eq!(m.query, "src");
        assert_eq!(m.start, 5);
        assert_eq!(m.end, 9);
    }

    #[test]
    fn cursor_in_whitespace_after_token_returns_none() {
        let (s, c) = at("look @src |done");
        assert_eq!(extract(&s, c), None);
    }

    #[test]
    fn cursor_in_next_token_returns_none() {
        let (s, c) = at("look @src d|one");
        assert_eq!(extract(&s, c), None);
    }

    #[test]
    fn multiple_tokens_picks_active_one() {
        let (s, c) = at("@a @b|");
        let m = extract(&s, c).expect("active");
        assert_eq!(m.query, "b");
        assert_eq!(m.start, 3);
        assert_eq!(m.end, 5);
    }

    #[test]
    fn multiple_tokens_picks_first_when_cursor_there() {
        let (s, c) = at("@a| @b");
        let m = extract(&s, c).expect("active");
        assert_eq!(m.query, "a");
        assert_eq!(m.start, 0);
        assert_eq!(m.end, 2);
    }

    #[test]
    fn three_consecutive_tokens_middle_active() {
        let (s, c) = at("@a @b| @c");
        let m = extract(&s, c).expect("active");
        assert_eq!(m.query, "b");
        assert_eq!(m.start, 3);
        assert_eq!(m.end, 5);
    }

    #[test]
    fn email_like_never_triggers() {
        let (s, c) = at("user@exam|ple.com");
        assert_eq!(extract(&s, c), None);
    }

    #[test]
    fn at_after_punctuation_no_space_rejects() {
        let (s, c) = at("before,@src|");
        // `,` is not whitespace → not a valid token start.
        assert_eq!(extract(&s, c), None);
    }

    #[test]
    fn at_after_paren_with_space_accepts() {
        let (s, c) = at("( @src|)");
        let m = extract(&s, c).expect("active");
        // `)` is not whitespace → token extends through it.
        // Submission-time `expand_at_path_references` strips trailing
        // punctuation; here we just verify the token is detected.
        assert_eq!(m.query, "src)");
    }

    #[test]
    fn quoted_path_with_spaces() {
        let (s, c) = at(r#"@"foo bar|""#);
        let m = extract(&s, c).expect("active");
        assert_eq!(m.query, "foo bar");
        assert_eq!(m.start, 0);
        assert!(m.is_quoted);
    }

    #[test]
    fn quoted_path_cursor_in_middle() {
        let (s, c) = at(r#"@"foo b|ar baz""#);
        let m = extract(&s, c).expect("active");
        assert_eq!(m.query, "foo bar baz");
        assert!(m.is_quoted);
    }

    #[test]
    fn quoted_path_unclosed() {
        let (s, c) = at(r#"@"unclosed|"#);
        let m = extract(&s, c).expect("active");
        assert_eq!(m.query, "unclosed");
        assert!(m.is_quoted);
    }

    #[test]
    fn cursor_after_closing_quote_returns_none() {
        let (s, c) = at(r#"cmd @"a b" |then"#);
        assert_eq!(extract(&s, c), None);
    }

    #[test]
    fn slash_separator_works() {
        let (s, c) = at("@src/|main.rs");
        let m = extract(&s, c).expect("active");
        assert_eq!(m.query, "src/main.rs");
        assert_eq!(m.start, 0);
    }

    #[test]
    fn unicode_path() {
        let (s, c) = at("@日本/|");
        let m = extract(&s, c).expect("active");
        assert!(m.query.contains("日本"));
    }

    #[test]
    fn empty_input() {
        assert_eq!(extract("", 0), None);
    }

    #[test]
    fn bare_at_at_start() {
        let m = extract("@", 1).expect("active");
        assert_eq!(m.query, "");
        assert_eq!(m.start, 0);
        assert_eq!(m.end, 1);
    }

    #[test]
    fn cursor_clamped_when_out_of_range() {
        // Should not panic.
        let m = extract("@src", 999).expect("active");
        assert_eq!(m.query, "src");
    }

    #[test]
    fn whitespace_between_at_tokens_blocks() {
        // Cursor sits in whitespace between two tokens — neither is active.
        let (s, c) = at("@a |@b");
        assert_eq!(extract(&s, c), None);
    }

    #[test]
    fn suggest_at_path_finds_top_level_entry() {
        let temp = tempfile::TempDir::new().unwrap();
        std::fs::write(temp.path().join("main.rs"), "").unwrap();
        std::fs::create_dir(temp.path().join("src")).unwrap();
        std::fs::write(temp.path().join("Cargo.toml"), "").unwrap();

        let mut suggestions = suggest_at_path(temp.path(), "@", 10);
        suggestions.sort();
        assert!(
            suggestions.contains(&"@main.rs".to_string()),
            "got: {:?}",
            suggestions
        );
        assert!(
            suggestions.contains(&"@src/".to_string()),
            "directories should have trailing slash: {:?}",
            suggestions
        );
        assert!(suggestions.contains(&"@Cargo.toml".to_string()));
    }

    #[test]
    fn suggest_at_path_filters_by_prefix() {
        let temp = tempfile::TempDir::new().unwrap();
        std::fs::write(temp.path().join("main.rs"), "").unwrap();
        std::fs::write(temp.path().join("Cargo.toml"), "").unwrap();
        std::fs::write(temp.path().join("README.md"), "").unwrap();

        let suggestions = suggest_at_path(temp.path(), "@m", 10);
        assert_eq!(suggestions, vec!["@main.rs"]);

        let suggestions = suggest_at_path(temp.path(), "@C", 10);
        assert_eq!(suggestions, vec!["@Cargo.toml"]);
    }

    #[test]
    fn suggest_at_path_descends_into_subdirectory() {
        let temp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(temp.path().join("src")).unwrap();
        std::fs::write(temp.path().join("src/main.rs"), "").unwrap();
        std::fs::write(temp.path().join("src/lib.rs"), "").unwrap();

        let mut suggestions = suggest_at_path(temp.path(), "@src/", 10);
        suggestions.sort();
        assert_eq!(suggestions, vec!["@src/lib.rs", "@src/main.rs"]);

        let suggestions = suggest_at_path(temp.path(), "@src/m", 10);
        assert_eq!(suggestions, vec!["@src/main.rs"]);
    }

    #[test]
    fn suggest_at_path_skips_hidden_unless_dot_typed() {
        let temp = tempfile::TempDir::new().unwrap();
        std::fs::write(temp.path().join(".env"), "").unwrap();
        std::fs::write(temp.path().join("public.txt"), "").unwrap();

        // Default: don't suggest dotfiles.
        let suggestions = suggest_at_path(temp.path(), "@", 10);
        assert_eq!(suggestions, vec!["@public.txt"]);

        // User explicitly typed '.': suggest dotfiles.
        let suggestions = suggest_at_path(temp.path(), "@.", 10);
        assert_eq!(suggestions, vec!["@.env"]);
    }

    #[test]
    fn suggest_at_path_respects_limit() {
        let temp = tempfile::TempDir::new().unwrap();
        for i in 0..20 {
            std::fs::write(temp.path().join(format!("f{i:02}.txt")), "").unwrap();
        }
        let suggestions = suggest_at_path(temp.path(), "@f", 5);
        assert_eq!(suggestions.len(), 5);
    }
}

#[derive(Clone, Debug)]
struct ExternalCliSuggestionCandidate {
    source: &'static str,
    path: PathBuf,
    modified: SystemTime,
    session_id: Option<String>,
    working_dir: Option<String>,
    context: Option<String>,
}

fn latest_external_cli_continuation_prompt() -> Option<String> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let mut candidates = Vec::new();
    candidates.extend(latest_jsonl_suggestion_candidates(
        &home.join(".codex/sessions"),
        "Codex",
        32,
    ));
    candidates.extend(latest_jsonl_suggestion_candidates(
        &home.join(".claude/projects"),
        "Claude Code",
        32,
    ));
    let candidate = candidates
        .into_iter()
        .max_by_key(|candidate| candidate.modified)?;
    let location = candidate
        .working_dir
        .as_deref()
        .map_or_else(String::new, |dir| {
            let label = Path::new(dir)
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| !name.trim().is_empty())
                .unwrap_or(dir);
            format!(" in {label}")
        });
    let cwd = candidate
        .working_dir
        .as_deref()
        .map(|dir| format!(" cwd `{dir}`"))
        .unwrap_or_default();
    let session_id = candidate
        .session_id
        .as_deref()
        .map(|id| format!(" session `{id}`"))
        .unwrap_or_default();
    let context = candidate
        .context
        .as_deref()
        .map(|context| format!(": {}", compact_suggestion_text(context, 72)))
        .unwrap_or_default();
    Some(format!(
        "Continue the latest {source} session{location}. Transcript: `{path}`.{session_id}{cwd}{context}. Read that transcript if needed, summarize the current state, then continue from there.",
        source = candidate.source,
        path = candidate.path.display(),
    ))
}

fn latest_jsonl_suggestion_candidates(
    root: &Path,
    source: &'static str,
    scan_limit: usize,
) -> Vec<ExternalCliSuggestionCandidate> {
    if !root.is_dir() {
        return Vec::new();
    }
    let mut files = Vec::new();
    collect_jsonl_suggestion_files(root, &mut files);
    files.sort_by(|a, b| b.1.cmp(&a.1));
    files.truncate(scan_limit);
    files
        .into_iter()
        .filter_map(|(path, modified)| suggestion_candidate_from_jsonl(&path, source, modified))
        .collect()
}

fn collect_jsonl_suggestion_files(root: &Path, files: &mut Vec<(PathBuf, SystemTime)>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            collect_jsonl_suggestion_files(&path, files);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
            files.push((path, metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH)));
        }
    }
}

fn suggestion_candidate_from_jsonl(
    path: &Path,
    source: &'static str,
    modified: SystemTime,
) -> Option<ExternalCliSuggestionCandidate> {
    let content = std::fs::read_to_string(path).ok()?;
    let mut working_dir = None;
    let mut session_id = None;
    let mut last_user_text = None;
    let mut summary_text = None;
    for line in content.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
            continue;
        };
        if working_dir.is_none() {
            working_dir = value
                .get("cwd")
                .or_else(|| value.get("payload").and_then(|payload| payload.get("cwd")))
                .and_then(|value| value.as_str())
                .map(str::to_string);
        }
        if session_id.is_none() {
            session_id = value
                .get("sessionId")
                .or_else(|| value.get("session_id"))
                .or_else(|| {
                    value
                        .get("payload")
                        .and_then(|payload| payload.get("session_id"))
                })
                .or_else(|| value.get("payload").and_then(|payload| payload.get("id")))
                .and_then(|value| value.as_str())
                .map(str::to_string);
        }
        if summary_text.is_none() {
            summary_text = value
                .get("summary")
                .or_else(|| value.get("lastPrompt"))
                .or_else(|| {
                    value
                        .get("payload")
                        .and_then(|payload| payload.get("summary"))
                })
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_string);
        }
        if jsonl_suggestion_role(&value) == Some("user")
            && let Some(text) = jsonl_suggestion_text(&value)
            && !text.trim().is_empty()
        {
            last_user_text = Some(text);
        }
    }
    if working_dir.is_none()
        && session_id.is_none()
        && last_user_text.is_none()
        && summary_text.is_none()
    {
        return None;
    }
    Some(ExternalCliSuggestionCandidate {
        source,
        path: path.to_path_buf(),
        modified,
        session_id,
        working_dir,
        context: last_user_text.or(summary_text),
    })
}

fn jsonl_suggestion_role(value: &serde_json::Value) -> Option<&str> {
    value
        .get("message")
        .and_then(|message| message.get("role"))
        .or_else(|| value.get("role"))
        .or_else(|| value.get("payload").and_then(|payload| payload.get("role")))
        .or_else(|| value.get("type"))
        .and_then(|role| role.as_str())
}

fn jsonl_suggestion_text(value: &serde_json::Value) -> Option<String> {
    let content = value
        .get("message")
        .and_then(|message| message.get("content"))
        .or_else(|| value.get("lastPrompt"))
        .or_else(|| value.get("content"))
        .or_else(|| {
            value
                .get("payload")
                .and_then(|payload| payload.get("content"))
        })?;
    if let Some(text) = content
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        return Some(text.to_string());
    }
    let text = content
        .as_array()?
        .iter()
        .filter_map(|block| {
            block
                .get("text")
                .or_else(|| block.get("input_text"))
                .or_else(|| block.get("output_text"))
                .or_else(|| block.get("content"))
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|text| !text.is_empty())
        })
        .collect::<Vec<_>>()
        .join(" ");
    (!text.is_empty()).then_some(text)
}

fn compact_suggestion_text(text: &str, max_chars: usize) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= max_chars {
        return compact;
    }
    let mut truncated = compact
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    truncated.push('…');
    truncated
}

#[cfg(test)]
mod external_cli_suggestion_tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn parses_claude_code_jsonl_with_session_path_and_context() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("session.jsonl");
        std::fs::write(
            &path,
            r#"{"type":"queue-operation","operation":"enqueue","timestamp":"2026-05-28T02:30:54.188Z","sessionId":"abc","content":"queued prompt"}
{"type":"user","message":{"role":"user","content":"Organize my windows by project"},"cwd":"/home/jeremy","sessionId":"abc"}
{"type":"last-prompt","lastPrompt":"fallback prompt","sessionId":"abc"}
"#,
        )
        .expect("write fixture");

        let candidate =
            suggestion_candidate_from_jsonl(&path, "Claude Code", SystemTime::UNIX_EPOCH)
                .expect("candidate");
        assert_eq!(candidate.source, "Claude Code");
        assert_eq!(candidate.path, path);
        assert_eq!(candidate.session_id.as_deref(), Some("abc"));
        assert_eq!(candidate.working_dir.as_deref(), Some("/home/jeremy"));
        assert_eq!(
            candidate.context.as_deref(),
            Some("Organize my windows by project")
        );
    }

    #[test]
    fn parses_codex_input_text_blocks() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("codex.jsonl");
        std::fs::write(
            &path,
            r#"{"type":"session_meta","payload":{"id":"sid","cwd":"/home/jeremy/jcode"}}
{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"check in on jcode"}]}}
"#,
        )
        .expect("write fixture");

        let candidate = suggestion_candidate_from_jsonl(&path, "Codex", SystemTime::UNIX_EPOCH)
            .expect("candidate");
        assert_eq!(candidate.session_id.as_deref(), Some("sid"));
        assert_eq!(candidate.working_dir.as_deref(), Some("/home/jeremy/jcode"));
        assert_eq!(candidate.context.as_deref(), Some("check in on jcode"));
    }

    #[test]
    fn discovery_sorts_after_collecting_nested_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        let old_dir = temp.path().join("a");
        let new_dir = temp.path().join("z/deep");
        std::fs::create_dir_all(&old_dir).expect("old dir");
        std::fs::create_dir_all(&new_dir).expect("new dir");
        std::fs::write(
            old_dir.join("old.jsonl"),
            r#"{"type":"user","message":{"role":"user","content":"old"},"sessionId":"old"}"#,
        )
        .expect("old fixture");
        std::thread::sleep(std::time::Duration::from_millis(20));

        let new_path = new_dir.join("new.jsonl");
        std::fs::write(
            &new_path,
            r#"{"type":"user","message":{"role":"user","content":"new"},"sessionId":"new"}"#,
        )
        .expect("new fixture");
        // Ensure the newer file has a strictly later mtime even on coarse filesystems.
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&new_path)
            .expect("open new");
        writeln!(file).expect("touch new");

        let candidates = latest_jsonl_suggestion_candidates(temp.path(), "Claude Code", 1);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].context.as_deref(), Some("new"));
    }
}
