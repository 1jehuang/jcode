use super::*;
use crate::tui::core;
use std::path::PathBuf;

#[derive(Clone, Copy)]
struct RegisteredCommand {
    name: &'static str,
    help: &'static str,
    hidden: bool,
}

impl RegisteredCommand {
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

const REGISTERED_COMMANDS: &[RegisteredCommand] = &[
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
    RegisteredCommand::public(
        "/swarm-prompt",
        "Open the active swarm routing prompt in your editor",
    ),
    RegisteredCommand::public("/subagent", "Launch a subagent manually"),
    RegisteredCommand::public("/observe", "Show the latest tool context in the side panel"),
    RegisteredCommand::public("/todos", "Show the session todo list as a card in the chat"),
    RegisteredCommand::hidden("/todo", "Alias for /todos"),
    RegisteredCommand::public("/splitview", "Mirror the current chat in the side panel"),
    RegisteredCommand::public("/split-view", "Alias for /splitview"),
    RegisteredCommand::public("/btw", "Ask a side question in the side panel"),
    RegisteredCommand::public("/ssh", "Connect to a remote machine using system SSH"),
    RegisteredCommand::public("/git", "Show git status for the session working directory"),
    RegisteredCommand::public("/colors", "List, configure, and score every TUI color"),
    RegisteredCommand::hidden("/color", "Alias for /colors"),
    RegisteredCommand::public("/hotkeys", "List hotkeys with your personal usage"),
    RegisteredCommand::public("/terminal-setup", "Fix Shift+Enter newlines"),
    RegisteredCommand::public("/commit", "Make logical commits from current changes"),
    RegisteredCommand::public(
        "/merge",
        "Merge current branch into main/master and switch to it (no push)",
    ),
    RegisteredCommand::public(
        "/commit-push",
        "Make logical commits from current changes, then push",
    ),
    RegisteredCommand::hidden("/commit-and-push", "Alias for /commit-push"),
    RegisteredCommand::public(
        "/fast-release",
        "Publish Linux immediately from the warm selfdev cache; CI adds other platforms",
    ),
    RegisteredCommand::public(
        "/fast-macos-release",
        "Publish a prepared macOS arm64 build immediately; CI adds other platforms",
    ),
    RegisteredCommand::public("/remote", "Reach this session from another machine"),
    RegisteredCommand::public(
        "/merge-remote-release",
        "Merge into main/master, validate, push, and release remotely",
    ),
    RegisteredCommand::public(
        "/remote-release",
        "Push the release tag immediately; CI builds and publishes every platform",
    ),
    RegisteredCommand::hidden("/cut-release", "Alias for /fast-release"),
    RegisteredCommand::hidden("/commit-push-release", "Alias for /cut-release"),
    RegisteredCommand::public(
        "/triage",
        "Triage new GitHub issues and autonomously fix the safe ones",
    ),
    RegisteredCommand::public("/transcript", "Open the current session transcript file"),
    RegisteredCommand::public("/subagent-model", "Show/change subagent model policy"),
    RegisteredCommand::public("/autoreview", "Show/toggle automatic end-of-turn review"),
    RegisteredCommand::public("/autojudge", "Show/toggle automatic end-of-turn judging"),
    RegisteredCommand::public("/review", "Launch a one-shot headed review session"),
    RegisteredCommand::public("/judge", "Launch a one-shot headed judge session"),
    RegisteredCommand::public("/effort", crate::tui::keybind::EFFORT_HELP),
    RegisteredCommand::public("/fast", "Toggle fast mode"),
    RegisteredCommand::public("/transport", "Show/change connection transport"),
    RegisteredCommand::public("/alignment", "Show/change default text alignment"),
    RegisteredCommand::public(
        "/compact-notifications",
        "Show/toggle single-line swarm/file-activity notifications",
    ),
    RegisteredCommand::public(
        "/show-agentgrep-output",
        "Show/toggle full agentgrep search output inline in chat",
    ),
    RegisteredCommand::public(
        "/tool-call-details",
        "Show/toggle dimmed technical details on tool rows with an intent",
    ),
    RegisteredCommand::public(
        "/thinking-display",
        "Show/hide the model's thinking text (off/full/current)",
    ),
    RegisteredCommand::hidden("/thinking", "Alias for /thinking-display"),
    RegisteredCommand::hidden("/reasoning", "Alias for /thinking-display"),
    RegisteredCommand::public("/cancel", "Cancel the current prompt or operation"),
    RegisteredCommand::public("/clear", "Clear conversation history"),
    RegisteredCommand::public("/cls", "Clear the view only, keeping context"),
    RegisteredCommand::hidden("/clear-view", "Alias for /cls"),
    RegisteredCommand::public("/rewind", "Rewind conversation to previous message"),
    RegisteredCommand::public("/poke", "Poke model to resume with incomplete todos"),
    RegisteredCommand::public("/plan", "Create a plan-only response as a plan card"),
    RegisteredCommand::public("/improve", "Autonomously improve the repository"),
    RegisteredCommand::public("/refactor", "Run a safe refactor loop"),
    RegisteredCommand::public("/compact", "Compact context"),
    RegisteredCommand::public("/fix", "Recover when the model cannot continue"),
    RegisteredCommand::public("/voice", "Voice input: speak, then send (Ctrl+Space)"),
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
    RegisteredCommand::public(
        "/skills",
        "Show loaded skills and jcode-endorsed recommendations",
    ),
    RegisteredCommand::public("/plugin", "Inspect and install GitHub plugin bundles"),
    RegisteredCommand::public("/version", "Show current version"),
    RegisteredCommand::public("/changelog", "Show recent changes in this build"),
    RegisteredCommand::public("/info", "Show session info and tokens"),
    RegisteredCommand::public("/reset", "Review and confirm a banked OpenAI usage reset"),
    RegisteredCommand::public("/usage", "Show connected provider usage limits"),
    RegisteredCommand::public(
        "/productivity",
        "Generate a shareable usage report + dashboard image",
    ),
    RegisteredCommand::public("/wrapped", "Alias for /productivity"),
    RegisteredCommand::public("/feedback", "Send feedback about jcode"),
    RegisteredCommand::public("/telemetry", "Show or change what jcode sends"),
    RegisteredCommand::public("/support", "Email support with diagnostics prefilled"),
    RegisteredCommand::public("/subscription", "Show jcode subscription status"),
    RegisteredCommand::public("/subscribe", "Why and how to subscribe to jcode"),
    RegisteredCommand::public("/config", "Show or edit configuration"),
    RegisteredCommand::public("/log", "Mark the current location in the jcode logs"),
    RegisteredCommand::public(
        "/keys",
        "Show keybinding conflicts with your terminal and OS (/keys refresh to rescan)",
    ),
    RegisteredCommand::hidden("/keybindings", "Alias for /keys"),
    RegisteredCommand::public(
        "/diff",
        "Cycle or set diff display mode (off/inline/full/file)",
    ),
    RegisteredCommand::public(
        "/onboarding-preview",
        "Preview the first-run onboarding screen",
    ),
    RegisteredCommand::public(
        "/onboarding-sim",
        "Rehearse real first-run onboarding as a new user (Alt+5 restart, Cmd+5 toggle)",
    ),
    RegisteredCommand::public("/reload", "Reload into newest available binary"),
    RegisteredCommand::public("/restart", "Restart with current binary"),
    RegisteredCommand::public("/rebuild", "Background rebuild and auto reload"),
    RegisteredCommand::public("/selfdev", "Open a new self-dev jcode session"),
    RegisteredCommand::public("/update", "Background update and auto reload"),
    RegisteredCommand::public("/update-sim", "Preview update UI safely (Alt+_)"),
    RegisteredCommand::hidden(
        "/update-rehearsal",
        "Self-dev: run the real updater against a fake local release in a sandbox (Alt+Shift+U)",
    ),
    RegisteredCommand::public("/resume", "Open session picker"),
    RegisteredCommand::public("/sessions", "Alias for /resume"),
    RegisteredCommand::public("/session", "Alias for /resume"),
    RegisteredCommand::public("/active", "Manage live sessions (working vs ready)"),
    RegisteredCommand::public("/catchup", "Open Catch Up picker"),
    RegisteredCommand::public("/back", "Return to the previous Catch Up session"),
    RegisteredCommand::public("/save", "Bookmark session for easy access"),
    RegisteredCommand::public("/unsave", "Remove bookmark from session"),
    RegisteredCommand::public("/rename", "Rename current session"),
    RegisteredCommand::public("/fork", "Fork session into a new window (optional prompt)"),
    RegisteredCommand::hidden("/split", "Alias for /fork"),
    RegisteredCommand::public("/desktop", "Open this session in Jcode Desktop"),
    RegisteredCommand::public("/transfer", "Compact context into a fresh handoff session"),
    RegisteredCommand::public(
        "/cloud",
        "Move this session to a cloud machine and keep working",
    ),
    RegisteredCommand::public("/local", "Bring a cloud session back to this machine"),
    RegisteredCommand::public("/workspace", "Niri-style session workspace"),
    RegisteredCommand::public("/quit", "Exit jcode"),
    RegisteredCommand::public("/auth", "Show authentication status"),
    RegisteredCommand::public("/login", "Login to a provider"),
    RegisteredCommand::public("/logout", "Log out of a provider"),
    RegisteredCommand::public("/account", "Open the combined account picker"),
    RegisteredCommand::public("/accounts", "Alias for /account"),
    RegisteredCommand::public("/cache", "Show cache stats; extend/5m saves Anthropic TTL"),
    RegisteredCommand::public("/debug-visual", "Toggle visual debug overlay"),
    RegisteredCommand::public("/screenshot-mode", "Toggle screenshot capture mode"),
    RegisteredCommand::public("/screenshot", "Capture a screenshot debug state"),
    RegisteredCommand::public("/record", "Record a demo capture"),
    RegisteredCommand::remote("/client-reload", "Force reload client binary"),
    RegisteredCommand::remote("/server-reload", "Force reload server binary"),
    RegisteredCommand::remote(
        "/continue",
        "Continue every interrupted live session that would auto-resume",
    ),
    RegisteredCommand::remote("/resumeall", "Alias for /continue"),
    RegisteredCommand::hidden("/resume-all", "Alias for /continue"),
    RegisteredCommand::hidden("/z", "Secret premium-mode command"),
    RegisteredCommand::hidden("/zz", "Secret premium-mode command"),
    RegisteredCommand::hidden("/zzz", "Secret premium-mode command"),
    RegisteredCommand::hidden("/zstatus", "Secret premium-mode status command"),
];

/// Every non-hidden slash command with its one-line description, in
/// registration order. The `/help` overlay uses this to list commands its
/// hand-written sections have not covered, so a newly registered command can
/// never be invisible to users.
pub(crate) fn registered_command_entries() -> impl Iterator<Item = (&'static str, &'static str)> {
    REGISTERED_COMMANDS
        .iter()
        .filter(|command| !command.hidden)
        .map(|command| (command.name, command.help))
}

pub(crate) fn registered_command_names() -> impl Iterator<Item = &'static str> {
    REGISTERED_COMMANDS.iter().map(|command| command.name)
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

    /// Typo-resistant fuzzy score. Higher is better; `None` means no match.
    /// Delegates to the shared [`crate::tui::fuzzy`] matcher so slash-command
    /// ranking and highlight positions stay in sync.
    pub(super) fn fuzzy_score(needle: &str, haystack: &str) -> Option<i32> {
        crate::tui::fuzzy::fuzzy_score(needle, haystack)
    }

    pub(super) fn rank_suggestions(
        &self,
        needle: &str,
        candidates: Vec<(String, &'static str)>,
    ) -> Vec<(String, &'static str)> {
        let needle = needle.to_lowercase();
        // Bucket 1 = literal prefix matches (exact typing always wins).
        // Bucket 0 = typo-tolerant fuzzy matches by descending score.
        let mut scored: Vec<(u8, i32, String, &'static str)> = Vec::new();
        for (cmd, help) in candidates {
            let lower = cmd.to_lowercase();
            if lower.starts_with(&needle) {
                scored.push((1, i32::MAX, cmd, help));
            } else if let Some(score) = Self::fuzzy_score(&needle, &lower) {
                scored.push((0, score, cmd, help));
            }
        }
        scored.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| b.1.cmp(&a.1))
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

        fn push_skill_commands(
            commands: &mut Vec<(String, &'static str)>,
            seen: &mut std::collections::HashSet<String>,
            skills: &crate::skill::SkillRegistry,
        ) {
            for skill in skills.list() {
                let command = format!("/{}", skill.name);
                if seen.insert(command.clone()) {
                    commands.push((command, "Activate skill"));
                }
            }
        }

        let mut seen = std::collections::HashSet::new();
        let mut commands: Vec<(String, &'static str)> = REGISTERED_COMMANDS
            .iter()
            .filter(|command| !command.hidden)
            .filter_map(|command| {
                let name = command.name.to_string();
                seen.insert(name.clone()).then_some((name, command.help))
            })
            .collect();

        let skills = self.current_skills_snapshot();
        push_skill_commands(&mut commands, &mut seen, &skills);

        if self.is_remote && !self.remote_skills.is_empty() {
            for skill in &self.remote_skills {
                let command = format!("/{skill}");
                if seen.insert(command.clone()) {
                    commands.push((command, "Activate skill"));
                }
            }
        }

        *self.command_candidates_cache.borrow_mut() = Some(CommandCandidatesCache {
            candidates: commands.clone(),
        });
        commands
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
        let cursor = if input == self.input {
            self.cursor_pos.min(input.len())
        } else {
            input.len()
        };
        let input = super::slash_command_parser::active_token_before_cursor(input, cursor)
            .map_or(input, |(start, end)| &input[start..end]);
        let input = input.trim_start();

        if crate::tui::is_ssh_remote() {
            // Do not enumerate local account labels, projects, or goals while
            // completing a command intended for a different host.
            if input.starts_with("/model ") || input.starts_with("/models ") {
                return self.rank_suggestions(input, self.model_suggestion_candidates());
            }
            return if input.starts_with('/') {
                self.rank_suggestions(input, self.command_candidates())
            } else {
                Vec::new()
            };
        }

        // Only show suggestions when input starts with /
        if !input.starts_with('/') {
            return vec![];
        }

        let prefix = input.to_lowercase();
        let prefix_trimmed = prefix.trim_end();

        if prefix.starts_with("/reset ") {
            return self.rank_suggestions(
                // Keep the read-only command first even after a trailing space.
                // Enter must not silently turn review into cancel or confirm.
                input.trim_end(),
                vec![
                    (
                        "/reset usage limits openai".into(),
                        "Review an available banked reset (read-only)",
                    ),
                    (
                        "/reset usage limits openai confirm".into(),
                        "Spend the pending banked reset",
                    ),
                    (
                        "/reset usage limits openai cancel".into(),
                        "Clear the pending reset confirmation",
                    ),
                ],
            );
        }

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
                    (
                        "/agents memory".into(),
                        "Configure optional memory extraction model",
                    ),
                    ("/agents ambient".into(), "Configure ambient model"),
                ],
            );
        }

        if prefix.starts_with("/subagent-model ") {
            return self.rank_suggestions(
                input,
                vec![
                    (
                        "/subagent-model inherit".into(),
                        "Use the current active model",
                    ),
                    (
                        "/subagent-model show".into(),
                        "Show the current subagent model policy",
                    ),
                ],
            );
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

        if prefix.starts_with("/colors ") || prefix.starts_with("/color ") {
            let base = if prefix.starts_with("/color ") {
                "/color"
            } else {
                "/colors"
            };
            let mut suggestions: Vec<(String, &'static str)> = vec![
                (
                    format!("{base} harmony"),
                    "Score the palette and list fixes",
                ),
                (
                    format!("{base} generate #8ab4f8"),
                    "Build a harmonious palette from one seed color",
                ),
                (format!("{base} reset"), "Reset every color to its default"),
                (format!("{base} export"), "Print the palette as config TOML"),
            ];
            suggestions.extend(
                jcode_tui_style::ALL_ROLES
                    .iter()
                    .map(|role| (format!("{base} {} #", role.key()), "Set this color role")),
            );
            return self.rank_suggestions(input, suggestions);
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
            let efforts = [
                "none",
                "minimal",
                "low",
                "medium",
                "high",
                "xhigh",
                "max",
                "swarm",
                "swarm-deep",
            ];
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
                ("/cache extend".into(), "Save 1 hour Anthropic cache TTL"),
                ("/cache 1h".into(), "Save 1 hour Anthropic cache TTL"),
                ("/cache 5m".into(), "Save 5 minute Anthropic cache TTL"),
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

        if prefix.starts_with("/compact-notifications ") {
            return self.rank_suggestions(
                input,
                vec![
                    (
                        "/compact-notifications status".into(),
                        "Show whether notifications are compact",
                    ),
                    (
                        "/compact-notifications on".into(),
                        "Collapse swarm/file-activity notifications to one line",
                    ),
                    (
                        "/compact-notifications off".into(),
                        "Show full multi-line notification cards",
                    ),
                ],
            );
        }

        if prefix.starts_with("/tool-call-details ") {
            return self.rank_suggestions(
                input,
                vec![
                    (
                        "/tool-call-details status".into(),
                        "Show whether technical details render on intent rows",
                    ),
                    (
                        "/tool-call-details on".into(),
                        "Show the dimmed technical detail next to tool intents",
                    ),
                    (
                        "/tool-call-details off".into(),
                        "Show only the intent on tool rows that have one",
                    ),
                ],
            );
        }

        if prefix.starts_with("/show-agentgrep-output ") {
            return self.rank_suggestions(
                input,
                vec![
                    (
                        "/show-agentgrep-output status".into(),
                        "Show whether agentgrep output is shown inline",
                    ),
                    (
                        "/show-agentgrep-output on".into(),
                        "Render full agentgrep search results inline in chat",
                    ),
                    (
                        "/show-agentgrep-output off".into(),
                        "Show only the one-line agentgrep summary",
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
            let visible_count = self.session.rewind_target_count();

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
    pub fn command_suggestions(&self) -> Vec<(String, &'static str)> {
        // Read up to eight times per frame; recomputing each time re-ranks
        // every registered command and skill (and can touch disk for some
        // prefixes). Memoize on the exact input plus the guard state the
        // branches below consult, so any transition still recomputes.
        let signature = self.command_suggestions_signature();
        let epoch = self.command_suggestions_epoch.get();
        if let Some(cache) = self.command_suggestions_cache.borrow().as_ref()
            && cache.epoch == epoch
            && cache.signature == signature
            && cache.input == self.input
        {
            return cache.suggestions.clone();
        }

        let suggestions = self.command_suggestions_uncached(&signature);
        *self.command_suggestions_cache.borrow_mut() = Some(CommandSuggestionsCache {
            input: self.input.clone(),
            signature,
            epoch,
            suggestions: suggestions.clone(),
        });
        suggestions
    }

    /// Advance the suggestion memo epoch, invalidating it. Called once per
    /// rendered frame so the memo only ever collapses reads *within* a frame
    /// and never serves data that predates a state change.
    pub(crate) fn advance_command_suggestions_epoch(&self) {
        self.command_suggestions_epoch
            .set(self.command_suggestions_epoch.get().wrapping_add(1));
    }

    /// Snapshot the non-input state that `command_suggestions` branches on
    /// before consulting the input buffer.
    pub(super) fn command_suggestions_signature(&self) -> CommandSuggestionsSignature {
        CommandSuggestionsSignature {
            pending_login: self.pending_login.is_some(),
            pending_account_input: self.pending_account_input.is_some(),
            pending_ssh_remote_name: self.pending_ssh_remote_name.is_some(),
            inline_preview_kind: self
                .inline_interactive_state
                .as_ref()
                .filter(|picker| picker.preview)
                .map(|picker| picker.kind),
        }
    }

    /// Uncached body of [`Self::command_suggestions`].
    pub(super) fn command_suggestions_uncached(
        &self,
        signature: &CommandSuggestionsSignature,
    ) -> Vec<(String, &'static str)> {
        // While an interactive prompt is waiting for typed input (API key,
        // OAuth callback, account label, SSH target), the composer is an
        // answer box, not a command line. Rendering the full command palette
        // there is misleading (issue #496): the only command those prompts
        // advertise is /cancel, so suggest exactly that and nothing else.
        if signature.pending_login
            || signature.pending_account_input
            || signature.pending_ssh_remote_name
        {
            let input = self.input.trim_start();
            let typed = input.trim_end();
            if !typed.is_empty() && typed.starts_with('/') && "/cancel".starts_with(typed) {
                return vec![("/cancel".into(), "Cancel the pending prompt")];
            }
            return Vec::new();
        }

        // While an inline picker preview is open for the command being typed,
        // the picker itself is the suggestion surface. Rendering the textual
        // suggestion list underneath would duplicate it (and its rows are not
        // arrow-navigable anyway, since the preview claims Up/Down first).
        if let Some(kind) = signature.inline_preview_kind {
            let input = self.input.trim_start();
            let suppress = match kind {
                crate::tui::PickerKind::Model => {
                    input.starts_with("/model") || input.starts_with("/models")
                }
                crate::tui::PickerKind::Login => input.starts_with("/login"),
                _ => false,
            };
            if suppress {
                return Vec::new();
            }
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
        self.sync_model_picker_preview_from_input();
        true
    }

    /// Whether to show the dedicated first-run onboarding welcome screen
    /// (gray telemetry header, prominent donut, welcome text, login prompt).
    ///
    /// This is true exactly when the empty screen is showing onboarding
    /// suggestion prompts (brand-new install / unauthenticated / new user) so
    /// the welcome layout and the suggestions stay in sync.
    pub fn onboarding_welcome_active(&self) -> bool {
        if crate::tui::is_ssh_remote() {
            return false;
        }
        // While the guided onboarding flow is driving the pre-suggestion phases
        // (model select / continue prompt), keep the welcome screen up even
        // though the server may have pushed scaffolding messages. The flow
        // renders its own body via `onboarding_welcome_kind()`.
        if self.onboarding_flow_drives_welcome() {
            return true;
        }
        // Once the guided phases are done, onboarding lands on the regular
        // new-session screen. The only remaining welcome body is the
        // unauthenticated "log in" hint. The preview mimics an empty session,
        // so it ignores the real transcript here.
        if (!self.display_messages.is_empty() || self.is_processing)
            && !self.onboarding_preview_mode
        {
            return false;
        }
        !self.suggestion_prompts().is_empty()
    }

    /// What the onboarding welcome screen should render in its body, driven by
    /// the active guided flow phase. Defaults to the starter suggestion cards.
    pub fn onboarding_welcome_kind(&self) -> crate::tui::OnboardingWelcomeKind {
        use crate::tui::OnboardingWelcomeKind;
        use crate::tui::app::onboarding_flow::{OnboardingPhase, SummaryPill, TelemetryLevel};
        match self.onboarding_phase() {
            Some(OnboardingPhase::Login { import }) => {
                let prompt = import.as_ref().map(|review| {
                    let rows = review
                        .candidates
                        .iter()
                        .enumerate()
                        .map(|(i, candidate)| crate::tui::LoginImportRow {
                            provider_summary: candidate.provider_summary().to_string(),
                            source_name: candidate.source_name().to_string(),
                            checked: review.checked.get(i).copied().unwrap_or(false),
                        })
                        .collect();
                    crate::tui::LoginImportPrompt {
                        rows,
                        cursor: review.cursor,
                        continue_focused: review.continue_focused,
                        choosing: review.choosing,
                        summary_pill: match review.summary_pill {
                            SummaryPill::Continue => crate::tui::ImportSummaryPill::Continue,
                            SummaryPill::ImportLess => crate::tui::ImportSummaryPill::ImportLess,
                            SummaryPill::Telemetry => crate::tui::ImportSummaryPill::Telemetry,
                        },
                        telemetry: review.telemetry.map(|level| match level {
                            TelemetryLevel::Everything => crate::tui::TelemetryChoice::Everything,
                            TelemetryLevel::NoContent => crate::tui::TelemetryChoice::NoContent,
                            TelemetryLevel::Nothing => crate::tui::TelemetryChoice::Nothing,
                        }),
                        telemetry_env_forced_off: crate::telemetry::opt_out_forced_by_env(),
                        checked_count: review.checked_count(),
                        seconds_left: review.seconds_remaining(),
                    }
                });
                OnboardingWelcomeKind::Login {
                    import: prompt,
                    importing: self.onboarding_import_in_progress.is_some(),
                    error: self.onboarding_import_error.clone(),
                    // Only offer the agent-repair option on the failure screen,
                    // and only when we can name an agent the user recently used.
                    repair_agent_label: self.onboarding_import_error.as_ref().and_then(|_| {
                        crate::tui::app::onboarding_repair::detect_preferred_repair_agent()
                            .map(|a| a.label().to_string())
                    }),
                }
            }
            Some(OnboardingPhase::LoginOpenAi { yes_highlighted }) => {
                OnboardingWelcomeKind::LoginOpenAi {
                    yes_highlighted: *yes_highlighted,
                }
            }
            Some(OnboardingPhase::ModelSelect) => OnboardingWelcomeKind::Suggestions,
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
    /// the welcome screen body (login, OpenAI-login prompt, or continue prompt).
    /// The transcript-pick phase uses the session-picker overlay instead, and
    /// the suggestions phase is the default welcome body.
    fn onboarding_flow_drives_welcome(&self) -> bool {
        use crate::tui::app::onboarding_flow::OnboardingPhase;
        matches!(
            self.onboarding_phase(),
            Some(OnboardingPhase::Login { .. })
                | Some(OnboardingPhase::LoginOpenAi { .. })
                | Some(OnboardingPhase::ContinuePrompt { .. })
        )
    }

    /// Get suggestion prompts for new users on the initial empty screen.
    /// Returns (label, prompt_text) pairs. Empty once user is experienced or not authenticated.
    pub fn suggestion_prompts(&self) -> Vec<(String, String)> {
        if crate::tui::is_ssh_remote() {
            return Vec::new();
        }
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

        Vec::new()
    }

    /// Autocomplete current input - cycles through suggestions on repeated Tab
    pub fn autocomplete(&mut self) -> bool {
        if let Some(range) =
            super::slash_command_parser::active_token_before_cursor(&self.input, self.cursor_pos)
        {
            let suggestions = self.get_suggestions_for(&self.input);
            return self.autocomplete_active_slash_token(range, suggestions);
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
        let selected = self
            .command_suggestion_selected
            .min(current_suggestions.len().saturating_sub(1));
        let (cmd, _) = &current_suggestions[selected];
        let base = self.input.clone();
        self.remember_input_undo_state();
        self.input = cmd.clone();
        // If unique match, add trailing space for arg-accepting commands
        if current_suggestions.len() == 1 && Self::command_accepts_args(&self.input) {
            self.input.push(' ');
        }
        self.cursor_pos = self.input.len();
        self.tab_completion_state = Some((base, selected));
        self.command_suggestion_selected = 0;
        true
    }

    fn autocomplete_active_slash_token(
        &mut self,
        range: (usize, usize),
        current_suggestions: Vec<(String, &'static str)>,
    ) -> bool {
        let current_token = self.input[range.0..range.1].to_string();

        if let Some((ref base, idx)) = self.tab_completion_state.clone()
            && super::slash_command_parser::active_token_before_cursor(base, base.len()).is_some()
        {
            let base_suggestions = self.get_suggestions_for(base);
            if base_suggestions.len() > 1
                && base_suggestions
                    .iter()
                    .any(|(command, _)| command == &current_token)
            {
                let next_index = (idx + 1) % base_suggestions.len();
                let (command, _) = &base_suggestions[next_index];
                self.remember_input_undo_state();
                self.input.replace_range(range.0..range.1, command.as_str());
                self.cursor_pos = range.0 + command.len();
                self.tab_completion_state = Some((base.clone(), next_index));
                return true;
            }
        }

        if current_suggestions.is_empty() {
            self.tab_completion_state = None;
            return false;
        }

        if current_suggestions.len() == 1 && current_suggestions[0].0 == current_token {
            if Self::command_accepts_args(&current_token) {
                self.remember_input_undo_state();
                self.input
                    .replace_range(range.0..range.1, &format!("{} ", current_token));
                self.cursor_pos = range.1 + 1;
                return true;
            }
            self.tab_completion_state = None;
            return false;
        }

        let selected = self
            .command_suggestion_selected
            .min(current_suggestions.len().saturating_sub(1));
        let (command, _) = &current_suggestions[selected];
        let base = self.input.clone();
        let mut replacement = command.clone();
        if current_suggestions.len() == 1 && Self::command_accepts_args(command) {
            replacement.push(' ');
        }
        self.remember_input_undo_state();
        self.input
            .replace_range(range.0..range.1, replacement.as_str());
        self.cursor_pos = range.0 + replacement.len();
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
        self.input_typing_undo = None;
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
        self.input_typing_undo = None;
        self.history_draft = None;
    }

    pub(super) fn undo_input_change(&mut self) {
        self.input_typing_undo = None;
        if let Some((input, cursor_pos)) = self.input_undo_stack.pop() {
            // The composer now holds a restored draft, so the copy stashed by a
            // history jump is stale: a later Down must not resurrect it.
            self.history_draft = None;
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
                | "/fork"
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
                | "/reset"
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
                | "/compact"
                | "/compact mode"
                | "/alignment"
                | "/compact-notifications"
                | "/show-agentgrep-output"
                | "/reasoning"
                | "/thinking"
                | "/thinking-display"
                | "/config"
                | "/save"
                | "/rename"
                | "/cache"
        )
    }
}

#[cfg(test)]
mod command_registry_tests {
    use super::*;

    /// Every slash command must be registered exactly once. Duplicate entries
    /// mean two different handlers claim the same name, so which one runs
    /// depends on dispatch order rather than on the registry the palette and
    /// `/help` show the user.
    #[test]
    fn registered_commands_have_no_duplicate_names() {
        let mut seen = std::collections::HashSet::new();
        let duplicates: Vec<&str> = REGISTERED_COMMANDS
            .iter()
            .filter(|command| !seen.insert(command.name))
            .map(|command| command.name)
            .collect();
        assert!(
            duplicates.is_empty(),
            "duplicate slash command registrations: {:?}",
            duplicates
        );
    }

    /// Aliases users can actually type must be discoverable through the
    /// registry, otherwise autocomplete silently omits working commands.
    #[test]
    fn known_aliases_are_registered() {
        let names: std::collections::HashSet<&str> =
            REGISTERED_COMMANDS.iter().map(|c| c.name).collect();
        for alias in [
            "/keybindings",
            "/commit-and-push",
            "/resume-all",
            "/hotkeys",
            "/keys",
        ] {
            assert!(names.contains(alias), "{alias} is not registered");
        }
    }
}
