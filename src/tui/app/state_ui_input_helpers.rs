use super::*;
use crate::tui::core;

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
            (
                "/todos".into(),
                "Show the current session todo list in the side panel",
            ),
            (
                "/splitview".into(),
                "Mirror the current chat in the side panel",
            ),
            ("/btw".into(), "Ask a side question in the side panel"),
            (
                "/git".into(),
                "Show git status for the session working directory",
            ),
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
            (
                "/poke".into(),
                "Poke model to resume with incomplete todos (on/off/status)",
            ),
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
        let openrouter_model = crate::provider::openrouter_catalog_model_id(model)
            .unwrap_or_else(|| model.to_string());

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
                | "/git"
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
}
