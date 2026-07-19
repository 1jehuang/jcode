use super::*;
use crate::tui::core;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

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
    RegisteredCommand::public("/hotkeys", "List hotkeys with your personal usage"),
    RegisteredCommand::hidden("/keys", "Alias for /hotkeys"),
    RegisteredCommand::public("/commit", "Make logical commits from current changes"),
    RegisteredCommand::public(
        "/commit-push",
        "Make logical commits from current changes, then push",
    ),
    RegisteredCommand::public(
        "/fast-release",
        "Publish Linux immediately from the warm selfdev cache; CI adds other platforms",
    ),
    RegisteredCommand::public(
        "/remote-release",
        "Push the release tag immediately; CI builds and publishes every platform",
    ),
    RegisteredCommand::hidden("/cut-release", "Alias for /fast-release"),
    RegisteredCommand::hidden("/commit-push-release", "Alias for /cut-release"),
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
        "/reasoning",
        "Show/change reasoning display (off/full/current)",
    ),
    RegisteredCommand::public("/cancel", "Cancel the current prompt or operation"),
    RegisteredCommand::public("/clear", "Clear conversation history"),
    RegisteredCommand::public("/rewind", "Rewind conversation to previous message"),
    RegisteredCommand::public("/poke", "Poke model to resume with incomplete todos"),
    RegisteredCommand::public("/plan", "Create a plan-only response as a plan card"),
    RegisteredCommand::public("/improve", "Autonomously improve the repository"),
    RegisteredCommand::public("/refactor", "Run a safe refactor loop"),
    RegisteredCommand::public("/compact", "Compact context"),
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
    RegisteredCommand::public(
        "/skills",
        "Show loaded skills and jcode-endorsed recommendations",
    ),
    RegisteredCommand::public("/version", "Show current version"),
    RegisteredCommand::public("/changelog", "Show recent changes in this build"),
    RegisteredCommand::public("/info", "Show session info and tokens"),
    RegisteredCommand::public("/usage", "Show connected provider usage limits"),
    RegisteredCommand::public(
        "/productivity",
        "Generate a shareable usage report + dashboard image",
    ),
    RegisteredCommand::public("/wrapped", "Alias for /productivity"),
    RegisteredCommand::public("/feedback", "Send feedback about jcode"),
    RegisteredCommand::public("/support", "Email support with diagnostics prefilled"),
    RegisteredCommand::public("/subscription", "Show jcode subscription status"),
    RegisteredCommand::public("/config", "Show or edit configuration"),
    RegisteredCommand::public("/log", "Mark the current location in the jcode logs"),
    RegisteredCommand::public(
        "/keys",
        "Show keybinding conflicts with your terminal and OS (/keys refresh to rescan)",
    ),
    RegisteredCommand::public(
        "/diff",
        "Cycle or set diff display mode (off/inline/full/pinned/file)",
    ),
    RegisteredCommand::public(
        "/onboarding-preview",
        "Preview the first-run onboarding screen",
    ),
    RegisteredCommand::public(
        "/onboarding-sim",
        "Walk through every first-run onboarding screen (Alt+5 reset, Cmd+5 toggle)",
    ),
    RegisteredCommand::public("/reload", "Reload into newest available binary"),
    RegisteredCommand::public("/restart", "Restart with current binary"),
    RegisteredCommand::public("/rebuild", "Background rebuild and auto reload"),
    RegisteredCommand::public("/selfdev", "Open a new self-dev jcode session"),
    RegisteredCommand::public("/update", "Background update and auto reload"),
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
    RegisteredCommand::remote(
        "/continue",
        "Continue every interrupted live session that would auto-resume",
    ),
    RegisteredCommand::remote("/resumeall", "Alias for /continue"),
    RegisteredCommand::hidden("/z", "Secret premium-mode command"),
    RegisteredCommand::hidden("/zz", "Secret premium-mode command"),
    RegisteredCommand::hidden("/zzz", "Secret premium-mode command"),
    RegisteredCommand::hidden("/zstatus", "Secret premium-mode status command"),
];

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
        // Bucket 1 = literal prefix matches (kept ahead of looser fuzzy hits so
        // exact typing always wins). Bucket 0 = typo-tolerant fuzzy matches,
        // ordered by descending fuzzy score.
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
        let input = input.trim_start();

        // Only show suggestions when input starts with /
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
            let efforts = ["none", "minimal", "low", "medium", "high", "xhigh", "max"];
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
        // While an interactive prompt is waiting for typed input (API key,
        // OAuth callback, account label, SSH target), the composer is an
        // answer box, not a command line. Rendering the full command palette
        // there is misleading (issue #496): the only command those prompts
        // advertise is /cancel, so suggest exactly that and nothing else.
        if self.pending_login.is_some()
            || self.pending_account_input.is_some()
            || self.pending_ssh_remote_name.is_some()
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
        if let Some(picker) = self.inline_interactive_state.as_ref()
            && picker.preview
        {
            let input = self.input.trim_start();
            let suppress = match picker.kind {
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
            Self::is_new_user_install()
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

        // macOS-only: offer to install ScrollWM, a scrolling window manager for
        // macOS. The web installer downloads the latest release, strips the
        // Gatekeeper quarantine, installs to ~/Applications, and launches it,
        // with no sudo and no system files touched.
        if cfg!(target_os = "macos") {
            prompts.push((
                "Install ScrollWM (scrolling window manager for macOS)".to_string(),
                "Install ScrollWM, the scrolling window manager for macOS, by running its official one-line installer: `curl -fsSL https://raw.githubusercontent.com/1jehuang/scrollwm/main/scripts/web-install.sh | bash`. It downloads the latest release, removes the Gatekeeper quarantine, installs to ~/Applications, and launches it (no sudo, no system files touched). Run the command for me and report whether it succeeded.".to_string(),
            ));
        }

        prompts.push((
            "Continue my last Codex CLI / Claude Code session".to_string(),
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
    pub fn autocomplete(&mut self) -> bool {
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
                | "/config"
                | "/save"
                | "/rename"
                | "/cache"
        )
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

/// How long a scan of the external-CLI session directories is reused before we
/// re-scan. The onboarding welcome screen animates a donut, so it redraws at
/// animation FPS and calls [`latest_external_cli_continuation_prompt`] multiple
/// times per frame. Scanning `~/.codex/sessions` / `~/.claude/projects` (reading
/// and JSON-parsing the newest transcripts) can cost hundreds of milliseconds
/// for users with large histories, which would otherwise make first-run
/// onboarding extremely laggy. A short TTL keeps the suggestion fresh while
/// reducing the cost to a single scan per window.
const EXTERNAL_CLI_PROMPT_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(30);

/// Cached result of the external-CLI continuation-prompt scan, with the time it
/// was computed. `None` value means "scanned, but nothing found".
type ExternalCliPromptCache = std::sync::RwLock<Option<(Option<String>, std::time::Instant)>>;
static EXTERNAL_CLI_PROMPT_CACHE: std::sync::LazyLock<ExternalCliPromptCache> =
    std::sync::LazyLock::new(|| std::sync::RwLock::new(None));

/// Cached front-end for [`latest_external_cli_continuation_prompt_uncached`].
///
/// See [`EXTERNAL_CLI_PROMPT_CACHE_TTL`] for why this is cached: the uncached
/// scan reads and parses the newest external transcripts, which is expensive for
/// large histories and would otherwise run several times per onboarding frame.
fn latest_external_cli_continuation_prompt() -> Option<String> {
    if let Ok(cache) = EXTERNAL_CLI_PROMPT_CACHE.read()
        && let Some((ref value, ref when)) = *cache
        && when.elapsed() < EXTERNAL_CLI_PROMPT_CACHE_TTL
    {
        return value.clone();
    }

    let value = latest_external_cli_continuation_prompt_uncached();

    if let Ok(mut cache) = EXTERNAL_CLI_PROMPT_CACHE.write() {
        *cache = Some((value.clone(), std::time::Instant::now()));
    }

    value
}

fn latest_external_cli_continuation_prompt_uncached() -> Option<String> {
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

    /// Faithful, real-home measurement of the per-frame onboarding cost.
    /// Ignored by default (depends on local ~/.codex and ~/.claude contents).
    /// Run with:
    ///   cargo test -p jcode-tui --lib onboarding_suggestion_scan_cost -- --ignored --nocapture
    #[test]
    #[ignore]
    fn onboarding_suggestion_scan_cost() {
        use std::time::Instant;

        // Cold: the uncached scan that reads + JSON-parses the newest external
        // transcripts. This is the work that used to run several times per frame.
        let cold_start = Instant::now();
        let cold = latest_external_cli_continuation_prompt_uncached();
        let cold_ms = cold_start.elapsed().as_secs_f64() * 1000.0;

        // Warm: the cached front-end the onboarding screen actually calls. Prime
        // the cache once, then measure repeated calls (as a redrawing frame does).
        let _ = latest_external_cli_continuation_prompt();
        let runs = 1000;
        let warm_start = Instant::now();
        let mut warm = None;
        for _ in 0..runs {
            warm = latest_external_cli_continuation_prompt();
        }
        let warm_ms = warm_start.elapsed().as_secs_f64() * 1000.0 / runs as f64;

        eprintln!(
            "external-cli continuation prompt: cold(uncached)={cold_ms:.1} ms, \
             warm(cached, avg of {runs})={warm_ms:.4} ms; cold_some={}, warm_some={}",
            cold.is_some(),
            warm.is_some()
        );
    }

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
