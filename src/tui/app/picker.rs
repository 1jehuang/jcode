use super::*;
use crate::tui::session_picker::{self, OverlayAction, PickerResult, ResumeTarget, SessionPicker};
use crate::tui::{
    AccountPickerAction, AgentModelTarget, PickerAction, PickerEntry, PickerKind, PickerOption,
    PickerState,
};

enum InlinePickerPreviewRequest {
    Model {
        filter: String,
    },
    Login {
        filter: String,
    },
    Usage {
        filter: String,
    },
    Account {
        provider_filter: Option<String>,
        filter: String,
    },
}

impl InlinePickerPreviewRequest {
    fn kind(&self) -> PickerKind {
        match self {
            Self::Model { .. } => PickerKind::Model,
            Self::Login { .. } => PickerKind::Login,
            Self::Usage { .. } => PickerKind::Usage,
            Self::Account { .. } => PickerKind::Account,
        }
    }

    fn filter(&self) -> &str {
        match self {
            Self::Model { filter }
            | Self::Login { filter }
            | Self::Usage { filter }
            | Self::Account { filter, .. } => filter,
        }
    }

    fn account_provider_filter(&self) -> Option<&str> {
        match self {
            Self::Account {
                provider_filter: Some(provider_filter),
                ..
            } => Some(provider_filter.as_str()),
            _ => None,
        }
    }

    fn open(&self, app: &mut App) {
        match self {
            Self::Model { .. } => app.open_model_picker(),
            Self::Usage { .. } => {
                app.open_usage_picker_loading();
                app.request_usage_report();
            }
            Self::Login { .. } => app.open_login_picker_inline(),
            Self::Account {
                provider_filter, ..
            } => app.open_account_picker(provider_filter.as_deref()),
        }
    }

    fn matches_picker(&self, app: &App, picker: &PickerState) -> bool {
        if !picker.preview || picker.kind != self.kind() {
            return false;
        }

        if self.kind() != PickerKind::Account {
            return true;
        }

        let desired_provider =
            app.inline_account_picker_provider_id(self.account_provider_filter());
        desired_provider.as_deref() == picker_account_provider_scope(picker)
    }
}

fn picker_account_provider_scope(picker: &PickerState) -> Option<&str> {
    picker.entries.first().and_then(|entry| match entry.action {
        PickerAction::Account(
            AccountPickerAction::Switch {
                ref provider_id, ..
            }
            | AccountPickerAction::Add { ref provider_id }
            | AccountPickerAction::Replace {
                ref provider_id, ..
            },
        ) => Some(provider_id.as_str()),
        PickerAction::Account(AccountPickerAction::OpenCenter {
            provider_filter: Some(ref provider_id),
        }) => Some(provider_id.as_str()),
        PickerAction::Account(AccountPickerAction::OpenCenter {
            provider_filter: None,
        })
        | PickerAction::Model
        | PickerAction::Login(_)
        | PickerAction::Usage { .. }
        | PickerAction::AgentTarget(_)
        | PickerAction::AgentModelChoice { .. } => None,
    })
}

fn slash_command_preview_filter(input: &str, commands: &[&str]) -> Option<String> {
    let trimmed = input.trim_start();
    for command in commands {
        if let Some(rest) = trimmed.strip_prefix(command) {
            if rest.is_empty() {
                return Some(String::new());
            }
            if rest
                .chars()
                .next()
                .map(|ch| ch.is_whitespace())
                .unwrap_or(false)
            {
                return Some(rest.trim_start().to_string());
            }
        }
    }
    None
}

fn catchup_candidates(current_session_id: &str) -> Vec<crate::tui::session_picker::SessionInfo> {
    session_picker::load_sessions()
        .unwrap_or_default()
        .into_iter()
        .filter(|session| session.id != current_session_id && session.needs_catchup)
        .collect()
}

fn catchup_queue_position(current_session_id: &str, session_id: &str) -> Option<(usize, usize)> {
    let candidates = catchup_candidates(current_session_id);
    let total = candidates.len();
    candidates
        .iter()
        .position(|session| session.id == session_id)
        .map(|idx| (idx + 1, total))
}

fn parse_agent_model_target(input: &str) -> Option<AgentModelTarget> {
    match input.trim().to_ascii_lowercase().as_str() {
        "swarm" | "agent" | "agents" | "subagent" | "subagents" => Some(AgentModelTarget::Swarm),
        "review" | "reviewer" | "code-review" | "codereview" => Some(AgentModelTarget::Review),
        "judge" | "judging" | "execution-judge" | "autojudge" => Some(AgentModelTarget::Judge),
        "memory" | "memories" | "sidecar" => Some(AgentModelTarget::Memory),
        "ambient" => Some(AgentModelTarget::Ambient),
        _ => None,
    }
}

fn agent_model_target_label(target: AgentModelTarget) -> &'static str {
    match target {
        AgentModelTarget::Swarm => "Swarm / subagent",
        AgentModelTarget::Review => "Code review",
        AgentModelTarget::Judge => "Judge",
        AgentModelTarget::Memory => "Memory",
        AgentModelTarget::Ambient => "Ambient",
    }
}

fn agent_model_target_slug(target: AgentModelTarget) -> &'static str {
    match target {
        AgentModelTarget::Swarm => "swarm",
        AgentModelTarget::Review => "review",
        AgentModelTarget::Judge => "judge",
        AgentModelTarget::Memory => "memory",
        AgentModelTarget::Ambient => "ambient",
    }
}

fn agent_model_target_config_path(target: AgentModelTarget) -> &'static str {
    match target {
        AgentModelTarget::Swarm => "agents.swarm_model",
        AgentModelTarget::Review => "autoreview.model",
        AgentModelTarget::Judge => "autojudge.model",
        AgentModelTarget::Memory => "agents.memory_model",
        AgentModelTarget::Ambient => "ambient.model",
    }
}

fn load_agent_model_override(target: AgentModelTarget) -> Option<String> {
    let cfg = crate::config::Config::load();
    match target {
        AgentModelTarget::Swarm => cfg.agents.swarm_model,
        AgentModelTarget::Review => cfg.autoreview.model,
        AgentModelTarget::Judge => cfg.autojudge.model,
        AgentModelTarget::Memory => cfg.agents.memory_model,
        AgentModelTarget::Ambient => cfg.ambient.model,
    }
}

fn save_agent_model_override(target: AgentModelTarget, model: Option<&str>) -> anyhow::Result<()> {
    let mut cfg = crate::config::Config::load();
    let value = model
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    match target {
        AgentModelTarget::Swarm => cfg.agents.swarm_model = value,
        AgentModelTarget::Review => cfg.autoreview.model = value,
        AgentModelTarget::Judge => cfg.autojudge.model = value,
        AgentModelTarget::Memory => cfg.agents.memory_model = value,
        AgentModelTarget::Ambient => cfg.ambient.model = value,
    }
    cfg.save()
}

fn model_entry_base_name(entry: &PickerEntry) -> String {
    if entry.effort.is_some() {
        entry
            .name
            .rsplit_once(" (")
            .map(|(base, _)| base.to_string())
            .unwrap_or_else(|| entry.name.clone())
    } else {
        entry.name.clone()
    }
}

fn model_entry_saved_spec(entry: &PickerEntry) -> String {
    let bare_name = model_entry_base_name(entry);
    let route = entry.options.get(entry.selected_option);
    if let Some(route) = route {
        if route.api_method == "copilot" {
            format!("copilot:{}", bare_name)
        } else if route.api_method == "cursor" {
            format!("cursor:{}", bare_name)
        } else if route.api_method == "openrouter" && route.provider != "auto" {
            if bare_name.contains('/') {
                format!("{}@{}", bare_name, route.provider)
            } else {
                format!("anthropic/{}@{}", bare_name, route.provider)
            }
        } else {
            bare_name
        }
    } else {
        bare_name
    }
}

fn agent_model_default_summary(target: AgentModelTarget, app: &App) -> String {
    match target {
        AgentModelTarget::Swarm => load_agent_model_override(target)
            .or_else(|| app.session.subagent_model.clone())
            .unwrap_or_else(|| app.provider.model()),
        AgentModelTarget::Review => load_agent_model_override(target)
            .or_else(|| super::commands::preferred_one_shot_review_override().map(|(m, _)| m))
            .or_else(|| app.session.model.clone())
            .unwrap_or_else(|| app.provider.model()),
        AgentModelTarget::Judge => load_agent_model_override(target)
            .or_else(|| super::commands::preferred_one_shot_review_override().map(|(m, _)| m))
            .or_else(|| app.session.model.clone())
            .unwrap_or_else(|| app.provider.model()),
        AgentModelTarget::Memory => {
            load_agent_model_override(target).unwrap_or_else(|| "sidecar auto-select".to_string())
        }
        AgentModelTarget::Ambient => {
            load_agent_model_override(target).unwrap_or_else(|| "provider default".to_string())
        }
    }
}

impl App {
    pub(super) fn model_picker_preview_filter(input: &str) -> Option<String> {
        slash_command_preview_filter(input, &["/model", "/models"])
    }

    pub(super) fn login_picker_preview_filter(input: &str) -> Option<String> {
        slash_command_preview_filter(input, &["/login"])
    }

    pub(super) fn usage_picker_preview_filter(input: &str) -> Option<String> {
        let _ = input;
        None
    }

    fn account_picker_preview_request(&self, input: &str) -> Option<InlinePickerPreviewRequest> {
        let trimmed = input.trim_start();
        let rest = trimmed
            .strip_prefix("/account")
            .or_else(|| trimmed.strip_prefix("/accounts"))?;

        if rest.is_empty() {
            return Some(InlinePickerPreviewRequest::Account {
                provider_filter: None,
                filter: String::new(),
            });
        }

        if !rest
            .chars()
            .next()
            .map(|c| c.is_whitespace())
            .unwrap_or(false)
        {
            return None;
        }

        let rest = rest.trim_start();
        if rest.is_empty() {
            return Some(InlinePickerPreviewRequest::Account {
                provider_filter: None,
                filter: String::new(),
            });
        }

        let mut parts = rest.split_whitespace();
        let first = parts.next()?;
        let remainder = parts.collect::<Vec<_>>().join(" ");
        let remainder = remainder.trim();

        match first {
            "switch" | "use" | "add" | "login" | "remove" | "rm" | "delete"
            | "default-provider" | "default-model" => return None,
            "list" | "ls" => {
                return Some(InlinePickerPreviewRequest::Account {
                    provider_filter: None,
                    filter: String::new(),
                });
            }
            _ => {}
        }

        let provider = crate::provider_catalog::resolve_login_provider(first);
        let provider_filter =
            provider.and_then(|provider| self.inline_account_picker_scope_key(Some(provider.id)));

        if provider.is_some() && provider_filter.is_none() {
            return None;
        }

        if let Some(provider_filter) = provider_filter {
            if remainder.is_empty() {
                return Some(InlinePickerPreviewRequest::Account {
                    provider_filter: Some(provider_filter),
                    filter: String::new(),
                });
            }

            let subcommand = remainder.split_whitespace().next().unwrap_or_default();
            match subcommand {
                "list" | "ls" => Some(InlinePickerPreviewRequest::Account {
                    provider_filter: Some(provider_filter),
                    filter: String::new(),
                }),
                "settings" | "login" | "add" | "switch" | "use" | "remove" | "rm" | "delete"
                | "transport" | "effort" | "fast" | "premium" | "api-base" | "api-key-name"
                | "env-file" | "default-model" => None,
                _ => Some(InlinePickerPreviewRequest::Account {
                    provider_filter: Some(provider_filter),
                    filter: remainder.to_string(),
                }),
            }
        } else {
            Some(InlinePickerPreviewRequest::Account {
                provider_filter: None,
                filter: rest.to_string(),
            })
        }
    }

    fn inline_picker_preview_request(&self, input: &str) -> Option<InlinePickerPreviewRequest> {
        Self::model_picker_preview_filter(input)
            .map(|filter| InlinePickerPreviewRequest::Model { filter })
            .or_else(|| {
                Self::login_picker_preview_filter(input)
                    .map(|filter| InlinePickerPreviewRequest::Login { filter })
            })
            .or_else(|| self.account_picker_preview_request(input))
    }

    pub(super) fn sync_model_picker_preview_from_input(&mut self) {
        let Some(request) = self.inline_picker_preview_request(&self.input) else {
            if self
                .picker_state
                .as_ref()
                .map(|picker| picker.preview)
                .unwrap_or(false)
            {
                self.picker_state = None;
            }
            return;
        };

        let should_open = self
            .picker_state
            .as_ref()
            .map(|picker| !request.matches_picker(self, picker))
            .unwrap_or(true);

        if should_open {
            let saved_input = self.input.clone();
            let saved_cursor = self.cursor_pos;
            request.open(self);
            if let Some(ref mut picker) = self.picker_state {
                picker.preview = true;
            }
            // Preview must not steal the user's command input.
            self.input = saved_input;
            self.cursor_pos = saved_cursor;
        }

        if let Some(ref mut picker) = self.picker_state {
            if picker.preview {
                picker.filter = request.filter().to_string();
                Self::apply_picker_filter(picker);
            }
        }
    }

    pub(super) fn activate_picker_from_preview(&mut self) -> bool {
        if !self
            .picker_state
            .as_ref()
            .map(|picker| picker.preview)
            .unwrap_or(false)
        {
            return false;
        }

        if let Some(ref mut picker) = self.picker_state {
            picker.preview = false;
        }
        if self
            .picker_state
            .as_ref()
            .map(|picker| picker.kind == PickerKind::Usage)
            .unwrap_or(false)
        {
            if let Some(ref mut picker) = self.picker_state {
                picker.column = 0;
            }
            self.input.clear();
            self.cursor_pos = 0;
            return true;
        }
        self.input.clear();
        self.cursor_pos = 0;
        let _ = self.handle_picker_key(KeyCode::Enter, KeyModifiers::NONE);
        true
    }

    pub(super) fn open_agents_picker(&mut self) {
        let models = [
            AgentModelTarget::Swarm,
            AgentModelTarget::Review,
            AgentModelTarget::Judge,
            AgentModelTarget::Memory,
            AgentModelTarget::Ambient,
        ]
        .into_iter()
        .map(|target| {
            let configured = load_agent_model_override(target);
            let summary = configured
                .clone()
                .unwrap_or_else(|| agent_model_default_summary(target, self));
            PickerEntry {
                name: agent_model_target_label(target).to_string(),
                options: vec![PickerOption {
                    provider: summary,
                    api_method: agent_model_target_config_path(target).to_string(),
                    available: true,
                    detail: format!("/agents {}", agent_model_target_slug(target)),
                    estimated_reference_cost_micros: None,
                }],
                action: PickerAction::AgentTarget(target),
                selected_option: 0,
                is_current: false,
                is_default: configured.is_some(),
                recommended: false,
                recommendation_rank: usize::MAX,
                old: false,
                created_date: None,
                effort: None,
            }
        })
        .collect();

        self.picker_state = Some(PickerState {
            kind: PickerKind::Model,
            filtered: (0..5).collect(),
            entries: models,
            selected: 0,
            column: 0,
            filter: String::new(),
            preview: false,
        });
        self.input.clear();
        self.cursor_pos = 0;
    }

    pub(super) fn open_login_picker_inline(&mut self) {
        let status = crate::auth::AuthStatus::check_fast();
        let providers = crate::provider_catalog::tui_login_providers();
        let models = providers
            .into_iter()
            .map(|provider| {
                let auth_state = status.state_for_provider(provider);
                let state_label = match auth_state {
                    crate::auth::AuthState::Available => "configured",
                    crate::auth::AuthState::Expired => "attention",
                    crate::auth::AuthState::NotConfigured => "setup",
                };
                let method_detail = status.method_detail_for_provider(provider);
                PickerEntry {
                    name: provider.display_name.to_string(),
                    options: vec![PickerOption {
                        provider: provider.auth_kind.label().to_string(),
                        api_method: state_label.to_string(),
                        available: true,
                        detail: format!("{} · {}", method_detail, provider.menu_detail),
                        estimated_reference_cost_micros: None,
                    }],
                    action: PickerAction::Login(provider),
                    selected_option: 0,
                    is_current: auth_state == crate::auth::AuthState::Available,
                    is_default: false,
                    recommended: provider.recommended,
                    recommendation_rank: usize::MAX,
                    old: false,
                    created_date: None,
                    effort: None,
                }
            })
            .collect::<Vec<_>>();

        self.picker_state = Some(PickerState {
            kind: PickerKind::Login,
            filtered: (0..models.len()).collect(),
            entries: models,
            selected: 0,
            column: 0,
            filter: String::new(),
            preview: false,
        });
        self.input.clear();
        self.cursor_pos = 0;
    }

    pub(super) fn open_agent_model_picker(&mut self, target: AgentModelTarget) {
        let configured = load_agent_model_override(target);
        let inherit_summary = agent_model_default_summary(target, self);
        self.open_model_picker();

        if let Some(ref mut picker) = self.picker_state {
            if target == AgentModelTarget::Memory {
                picker.entries.retain(|entry| {
                    matches!(
                        crate::provider::provider_for_model(&model_entry_base_name(entry)),
                        Some("openai" | "claude")
                    )
                });
            }

            for entry in &mut picker.entries {
                let matches_saved = configured.as_deref().map(|saved| {
                    let base = model_entry_base_name(entry);
                    model_entry_saved_spec(entry) == saved || base == saved
                }) == Some(true);
                entry.action = PickerAction::AgentModelChoice {
                    target,
                    clear_override: false,
                };
                entry.is_current = matches_saved;
                entry.is_default = false;
            }

            if let Some(saved) = configured.as_deref() {
                let already_present = picker.entries.iter().any(|entry| {
                    model_entry_saved_spec(entry) == saved || model_entry_base_name(entry) == saved
                });
                if !already_present {
                    picker.entries.insert(
                        0,
                        PickerEntry {
                            name: saved.to_string(),
                            options: vec![PickerOption {
                                provider: "saved override".to_string(),
                                api_method: agent_model_target_config_path(target).to_string(),
                                available: true,
                                detail: "not in current picker catalog".to_string(),
                                estimated_reference_cost_micros: None,
                            }],
                            action: PickerAction::AgentModelChoice {
                                target,
                                clear_override: false,
                            },
                            selected_option: 0,
                            is_current: true,
                            is_default: false,
                            recommended: false,
                            recommendation_rank: usize::MAX,
                            old: false,
                            created_date: None,
                            effort: None,
                        },
                    );
                }
            }

            picker.entries.insert(
                0,
                PickerEntry {
                    name: format!("inherit ({})", inherit_summary),
                    options: vec![PickerOption {
                        provider: "default".to_string(),
                        api_method: agent_model_target_config_path(target).to_string(),
                        available: true,
                        detail: "clear saved override".to_string(),
                        estimated_reference_cost_micros: None,
                    }],
                    action: PickerAction::AgentModelChoice {
                        target,
                        clear_override: true,
                    },
                    selected_option: 0,
                    is_current: configured.is_none(),
                    is_default: false,
                    recommended: false,
                    recommendation_rank: usize::MAX,
                    old: false,
                    created_date: None,
                    effort: None,
                },
            );

            picker.filtered = (0..picker.entries.len()).collect();
            picker.selected = picker
                .entries
                .iter()
                .position(|entry| entry.is_current)
                .unwrap_or(0);
            picker.column = 0;
            picker.filter.clear();
        }
    }

    pub(super) fn open_model_picker(&mut self) {
        use std::collections::BTreeMap;

        let current_model = if self.is_remote {
            self.remote_provider_model
                .clone()
                .unwrap_or_else(|| "unknown".to_string())
        } else {
            self.provider.model().to_string()
        };

        let cfg = crate::config::Config::load();
        let config_default_model = cfg.provider.default_model.clone();

        let is_config_default = |name: &str| -> bool {
            match &config_default_model {
                None => false,
                Some(default) => {
                    let bare = default.strip_prefix("copilot:").unwrap_or(default);
                    let bare = bare.strip_prefix("cursor:").unwrap_or(bare);
                    let bare = bare.split('@').next().unwrap_or(bare);
                    name == default || name == bare
                }
            }
        };

        let routes: Vec<crate::provider::ModelRoute> = if self.is_remote {
            if !self.remote_model_options.is_empty() {
                self.remote_model_options.clone()
            } else {
                self.build_remote_model_routes_fallback()
            }
        } else {
            self.provider.model_routes()
        };

        let routes = if routes.is_empty() && self.is_remote && current_model != "unknown" {
            vec![crate::provider::ModelRoute {
                model: current_model.clone(),
                provider: self
                    .remote_provider_name
                    .clone()
                    .unwrap_or_else(|| "current".to_string()),
                api_method: "current".to_string(),
                available: true,
                detail: "catalog still loading".to_string(),
                cheapness: None,
            }]
        } else {
            routes
        };

        if routes.is_empty() {
            self.set_status_notice("No models available");
            return;
        }

        let mut model_order: Vec<String> = Vec::new();
        let mut model_options: BTreeMap<String, Vec<PickerOption>> = BTreeMap::new();
        for r in &routes {
            if !model_options.contains_key(&r.model) {
                model_order.push(r.model.clone());
            }
            model_options
                .entry(r.model.clone())
                .or_default()
                .push(PickerOption {
                    provider: r.provider.clone(),
                    api_method: r.api_method.clone(),
                    available: r.available,
                    detail: r.detail.clone(),
                    estimated_reference_cost_micros: r.estimated_reference_cost_micros(),
                });
        }

        fn route_sort_key(r: &PickerOption) -> (u8, u8, u64, String) {
            let avail = if r.available { 0 } else { 1 };
            let method = match r.api_method.as_str() {
                "claude-oauth" | "openai-oauth" => 0,
                "copilot" => 1,
                "cursor" => 2,
                "api-key" => 3,
                "openrouter" => 4,
                _ => 5,
            };
            let cheapness = r.estimated_reference_cost_micros.unwrap_or(u64::MAX);
            (avail, method, cheapness, r.provider.clone())
        }

        const RECOMMENDED_MODELS: &[&str] = &[
            "gpt-5.4",
            "gpt-5.4[1m]",
            "gpt-5.4-pro",
            "claude-opus-4-6",
            "claude-opus-4-6[1m]",
            "claude-opus-4.6",
            "claude-sonnet-4-6",
            "claude-sonnet-4-6[1m]",
            "moonshotai/kimi-k2.5",
        ];

        const CLAUDE_OAUTH_ONLY_MODELS: &[&str] = &[
            "claude-opus-4-6",
            "claude-opus-4-6[1m]",
            "claude-sonnet-4-6",
            "claude-sonnet-4-6[1m]",
        ];

        const OPENAI_OAUTH_ONLY_MODELS: &[&str] = &["gpt-5.4", "gpt-5.4[1m]", "gpt-5.4-pro"];
        const COPILOT_OAUTH_MODELS: &[&str] = &["claude-opus-4.6", "gpt-5.4"];

        fn recommendation_rank(name: &str, recommended_models: &[&str]) -> usize {
            recommended_models
                .iter()
                .position(|model| *model == name)
                .unwrap_or(usize::MAX)
        }

        let latest_recommended_ts: Option<u64> = RECOMMENDED_MODELS
            .iter()
            .filter_map(|m| crate::provider::openrouter::model_created_timestamp(m))
            .max();
        let old_threshold_secs = latest_recommended_ts
            .map(|ts| ts.saturating_sub(30 * 86400))
            .unwrap_or(0);

        fn format_created(ts: u64) -> String {
            use chrono::{TimeZone, Utc};
            if let Some(dt) = Utc.timestamp_opt(ts as i64, 0).single() {
                dt.format("%b %Y").to_string()
            } else {
                String::new()
            }
        }

        let current_effort = self.provider.reasoning_effort();
        let available_efforts = self.provider.available_efforts();
        let is_openai = !available_efforts.is_empty();

        let mut entries: Vec<PickerEntry> = Vec::new();
        for name in &model_order {
            let mut entry_routes = model_options.remove(name).unwrap_or_default();
            entry_routes.sort_by_key(route_sort_key);

            let is_openai_model = crate::provider::ALL_OPENAI_MODELS.contains(&name.as_str());

            if is_openai_model && is_openai && !available_efforts.is_empty() {
                for effort in &available_efforts {
                    let effort_label = match *effort {
                        "xhigh" => "max",
                        "high" => "high",
                        "medium" => "med",
                        "low" => "low",
                        "none" => "none",
                        other => other,
                    };
                    let display_name = format!("{} ({})", name, effort_label);
                    let is_this_current =
                        *name == current_model && current_effort.as_deref() == Some(*effort);
                    let or_created = crate::provider::openrouter::model_created_timestamp(name);
                    entries.push(PickerEntry {
                        name: display_name,
                        options: entry_routes.clone(),
                        action: PickerAction::Model,
                        selected_option: 0,
                        is_current: is_this_current,
                        recommended: RECOMMENDED_MODELS.contains(&name.as_str())
                            && (*effort == "xhigh" || *effort == "high")
                            && (!(CLAUDE_OAUTH_ONLY_MODELS.contains(&name.as_str())
                                || OPENAI_OAUTH_ONLY_MODELS.contains(&name.as_str())
                                || COPILOT_OAUTH_MODELS.contains(&name.as_str()))
                                || entry_routes.iter().any(|r| {
                                    (r.api_method == "claude-oauth"
                                        || r.api_method == "openai-oauth"
                                        || r.api_method == "copilot")
                                        && r.available
                                })),
                        recommendation_rank: recommendation_rank(name, RECOMMENDED_MODELS),
                        old: old_threshold_secs > 0
                            && or_created.map(|t| t < old_threshold_secs).unwrap_or(false),
                        created_date: or_created.map(format_created),
                        effort: Some(effort.to_string()),
                        is_default: is_config_default(name),
                    });
                }
            } else {
                let or_created = crate::provider::openrouter::model_created_timestamp(name);
                let is_old = old_threshold_secs > 0
                    && or_created.map(|t| t < old_threshold_secs).unwrap_or(false);
                let is_recommended = RECOMMENDED_MODELS.contains(&name.as_str())
                    && (!(CLAUDE_OAUTH_ONLY_MODELS.contains(&name.as_str())
                        || OPENAI_OAUTH_ONLY_MODELS.contains(&name.as_str())
                        || COPILOT_OAUTH_MODELS.contains(&name.as_str()))
                        || entry_routes.iter().any(|r| {
                            (r.api_method == "claude-oauth"
                                || r.api_method == "openai-oauth"
                                || r.api_method == "copilot")
                                && r.available
                        }));
                entries.push(PickerEntry {
                    name: name.clone(),
                    options: entry_routes,
                    action: PickerAction::Model,
                    selected_option: 0,
                    is_current: *name == current_model,
                    recommended: is_recommended,
                    recommendation_rank: recommendation_rank(name, RECOMMENDED_MODELS),
                    old: is_old,
                    created_date: or_created.map(format_created),
                    effort: None,
                    is_default: is_config_default(name),
                });
            }
        }

        entries.sort_by(|a, b| {
            let a_current = if a.is_current { 0u8 } else { 1 };
            let b_current = if b.is_current { 0u8 } else { 1 };
            let a_rec = if a.recommended { 0u8 } else { 1 };
            let b_rec = if b.recommended { 0u8 } else { 1 };
            let a_rec_rank = if a.recommended {
                a.recommendation_rank
            } else {
                usize::MAX
            };
            let b_rec_rank = if b.recommended {
                b.recommendation_rank
            } else {
                usize::MAX
            };
            let a_avail = if a.options.first().map(|r| r.available).unwrap_or(false) {
                0u8
            } else {
                1
            };
            let b_avail = if b.options.first().map(|r| r.available).unwrap_or(false) {
                0u8
            } else {
                1
            };
            let a_old = if a.old { 1u8 } else { 0 };
            let b_old = if b.old { 1u8 } else { 0 };
            a_current
                .cmp(&b_current)
                .then(a_rec.cmp(&b_rec))
                .then(a_rec_rank.cmp(&b_rec_rank))
                .then(a_avail.cmp(&b_avail))
                .then(a_old.cmp(&b_old))
                .then(a.name.cmp(&b.name))
        });

        self.picker_state = Some(PickerState {
            kind: PickerKind::Model,
            filtered: (0..entries.len()).collect(),
            entries,
            selected: 0,
            column: 0,
            filter: String::new(),
            preview: false,
        });
        self.input.clear();
        self.cursor_pos = 0;
    }

    pub(super) fn build_remote_model_routes_fallback(&self) -> Vec<crate::provider::ModelRoute> {
        let auth = crate::auth::AuthStatus::check_fast();
        let mut routes = Vec::new();
        for model in &self.remote_available_entries {
            if model.contains('/') {
                let cached = crate::provider::openrouter::load_endpoints_disk_cache_public(model);
                let auto_detail = cached
                    .as_ref()
                    .and_then(|(eps, _)| eps.first().map(|ep| format!("→ {}", ep.provider_name)))
                    .unwrap_or_default();
                routes.push(crate::provider::ModelRoute {
                    model: model.clone(),
                    provider: "auto".to_string(),
                    api_method: "openrouter".to_string(),
                    available: auth.openrouter != crate::auth::AuthState::NotConfigured,
                    detail: auto_detail,
                    cheapness: None,
                });
                if let Some((endpoints, age)) = cached {
                    let age_str = if age < 3600 {
                        format!("{}m ago", age / 60)
                    } else if age < 86400 {
                        format!("{}h ago", age / 3600)
                    } else {
                        format!("{}d ago", age / 86400)
                    };
                    for ep in &endpoints {
                        routes.push(crate::provider::ModelRoute {
                            model: model.clone(),
                            provider: ep.provider_name.clone(),
                            api_method: "openrouter".to_string(),
                            available: auth.openrouter != crate::auth::AuthState::NotConfigured,
                            detail: format!("{} ({})", ep.detail_string(), age_str),
                            cheapness: None,
                        });
                    }
                }
                continue;
            }

            let mut added_any = false;

            if crate::provider::provider_for_model(model) == Some("claude")
                && auth.anthropic.has_oauth
            {
                let is_1m = model.ends_with("[1m]");
                let is_opus = model.contains("opus");
                let is_max = crate::auth::claude::is_max_subscription();
                let model_defaults_1m = crate::provider::anthropic::effectively_1m(&model);
                let (available, detail) =
                    if is_1m && !model_defaults_1m && !crate::usage::has_extra_usage() {
                        (false, "requires extra usage".to_string())
                    } else if is_opus && !is_max {
                        (false, "requires Max subscription".to_string())
                    } else {
                        (true, String::new())
                    };
                routes.push(crate::provider::ModelRoute {
                    model: model.clone(),
                    provider: "Anthropic".to_string(),
                    api_method: "claude-oauth".to_string(),
                    available,
                    detail,
                    cheapness: None,
                });
                added_any = true;
            }

            if crate::provider::ALL_OPENAI_MODELS.contains(&model.as_str()) {
                let availability = crate::provider::model_availability_for_account(model);
                let (available, detail) = if auth.openai == crate::auth::AuthState::NotConfigured {
                    (false, "no credentials".to_string())
                } else {
                    match availability.state {
                        crate::provider::AccountModelAvailabilityState::Available => {
                            (true, String::new())
                        }
                        crate::provider::AccountModelAvailabilityState::Unavailable => (
                            false,
                            crate::provider::format_account_model_availability_detail(
                                &availability,
                            )
                            .unwrap_or_else(|| "not available".to_string()),
                        ),
                        crate::provider::AccountModelAvailabilityState::Unknown => (
                            true,
                            crate::provider::format_account_model_availability_detail(
                                &availability,
                            )
                            .unwrap_or_else(|| "availability unknown".to_string()),
                        ),
                    }
                };
                routes.push(crate::provider::ModelRoute {
                    model: model.clone(),
                    provider: "OpenAI".to_string(),
                    api_method: "openai-oauth".to_string(),
                    available,
                    detail,
                    cheapness: None,
                });
                added_any = true;
            }

            if Self::remote_model_should_offer_copilot_route(model) && !model.contains("[1m]") {
                routes.push(crate::provider::ModelRoute {
                    model: model.clone(),
                    provider: "Copilot".to_string(),
                    api_method: "copilot".to_string(),
                    available: auth.copilot == crate::auth::AuthState::Available
                        || Self::remote_model_is_server_copilot_only(model),
                    detail: String::new(),
                    cheapness: None,
                });
                added_any = true;
            }

            if !added_any {
                routes.push(crate::provider::ModelRoute {
                    model: model.clone(),
                    provider: "unknown".to_string(),
                    api_method: "unknown".to_string(),
                    available: false,
                    detail: String::new(),
                    cheapness: None,
                });
            }
        }
        routes
    }

    pub(super) fn remote_model_should_offer_copilot_route(model: &str) -> bool {
        Self::remote_model_is_server_copilot_only(model)
            || crate::provider::copilot::is_known_display_model(model)
    }

    pub(super) fn remote_model_is_server_copilot_only(model: &str) -> bool {
        !model.is_empty()
            && !model.contains('/')
            && !matches!(
                crate::provider::provider_for_model(model),
                Some("claude" | "openai" | "gemini" | "cursor")
            )
    }

    pub(super) fn handle_picker_preview_key(
        &mut self,
        code: &KeyCode,
        modifiers: KeyModifiers,
    ) -> Result<bool> {
        let is_preview = self.picker_state.as_ref().map_or(false, |p| p.preview);
        if !is_preview {
            return Ok(false);
        }
        match code {
            KeyCode::Down => {
                if let Some(picker) = self.picker_state.as_mut() {
                    let max = picker.filtered.len().saturating_sub(1);
                    picker.selected = (picker.selected + 1).min(max);
                }
                Ok(true)
            }
            KeyCode::Up => {
                if let Some(picker) = self.picker_state.as_mut() {
                    picker.selected = picker.selected.saturating_sub(1);
                }
                Ok(true)
            }
            KeyCode::Char('j') if modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(picker) = self.picker_state.as_mut() {
                    let max = picker.filtered.len().saturating_sub(1);
                    picker.selected = (picker.selected + 1).min(max);
                }
                Ok(true)
            }
            KeyCode::Char('k') if modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(picker) = self.picker_state.as_mut() {
                    picker.selected = picker.selected.saturating_sub(1);
                }
                Ok(true)
            }
            KeyCode::PageDown => {
                if let Some(picker) = self.picker_state.as_mut() {
                    let max = picker.filtered.len().saturating_sub(1);
                    picker.selected = (picker.selected + 5).min(max);
                }
                Ok(true)
            }
            KeyCode::PageUp => {
                if let Some(picker) = self.picker_state.as_mut() {
                    picker.selected = picker.selected.saturating_sub(5);
                }
                Ok(true)
            }
            KeyCode::Enter => {
                if let Some(ref mut picker) = self.picker_state {
                    if picker.filtered.is_empty() {
                        self.picker_state = None;
                        self.input.clear();
                        self.cursor_pos = 0;
                        return Ok(true);
                    }
                    picker.preview = false;
                    if picker.kind == PickerKind::Usage {
                        picker.column = 0;
                        self.input.clear();
                        self.cursor_pos = 0;
                        self.request_usage_report();
                        return Ok(true);
                    }
                    picker.column = if picker.kind == PickerKind::Account {
                        0
                    } else {
                        2
                    };
                }
                self.input.clear();
                self.cursor_pos = 0;
                self.handle_picker_key(KeyCode::Enter, modifiers)?;
                Ok(true)
            }
            KeyCode::Esc => {
                self.picker_state = None;
                self.input.clear();
                self.cursor_pos = 0;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn handle_account_picker_selection(&mut self, action: AccountPickerAction) {
        match action {
            AccountPickerAction::Switch { provider_id, label } => {
                if self.is_remote {
                    self.pending_account_picker_action = Some(AccountPickerAction::Switch {
                        provider_id: provider_id.clone(),
                        label: label.clone(),
                    });
                    self.set_status_notice(format!("Account → {} ({})", label, provider_id));
                    return;
                }

                match provider_id.as_str() {
                    "claude" => self.switch_account(&label),
                    "openai" => self.switch_openai_account(&label),
                    _ => self.push_display_message(DisplayMessage::error(format!(
                        "Provider `{}` does not support account switching.",
                        provider_id
                    ))),
                }
            }
            AccountPickerAction::Add { provider_id } => match provider_id.as_str() {
                "claude" => match crate::auth::claude::next_account_label() {
                    Ok(label) => self.start_claude_login_for_account(&label),
                    Err(e) => self.push_display_message(DisplayMessage::error(format!(
                        "Failed to prepare Claude account: {}",
                        e
                    ))),
                },
                "openai" => match crate::auth::codex::next_account_label() {
                    Ok(label) => self.start_openai_login_for_account(&label),
                    Err(e) => self.push_display_message(DisplayMessage::error(format!(
                        "Failed to prepare OpenAI account: {}",
                        e
                    ))),
                },
                _ => self.push_display_message(DisplayMessage::error(format!(
                    "Provider `{}` does not support multiple accounts.",
                    provider_id
                ))),
            },
            AccountPickerAction::Replace { provider_id, label } => match provider_id.as_str() {
                "claude" => self.start_claude_login_for_account(&label),
                "openai" => self.start_openai_login_for_account(&label),
                _ => self.push_display_message(DisplayMessage::error(format!(
                    "Provider `{}` does not support account replacement.",
                    provider_id
                ))),
            },
            AccountPickerAction::OpenCenter { provider_filter } => {
                self.open_account_center(provider_filter.as_deref())
            }
        }
    }

    pub(super) fn open_session_picker(&mut self) {
        match session_picker::load_sessions_grouped() {
            Ok((server_groups, orphan_sessions)) => {
                let picker = SessionPicker::new_grouped(server_groups, orphan_sessions);
                self.session_picker_overlay = Some(RefCell::new(picker));
                self.session_picker_mode = SessionPickerMode::Resume;
            }
            Err(e) => {
                self.push_display_message(DisplayMessage::error(format!(
                    "Failed to load sessions: {}",
                    e
                )));
            }
        }
    }

    pub(super) fn open_catchup_picker(&mut self) {
        let current_session_id = super::commands::active_session_id(self);
        if catchup_candidates(&current_session_id).is_empty() {
            self.push_display_message(DisplayMessage::system(
                "No sessions currently need catch up.".to_string(),
            ));
            self.set_status_notice("Catch Up: none waiting");
            return;
        }

        match session_picker::load_sessions_grouped() {
            Ok((server_groups, orphan_sessions)) => {
                let mut picker = SessionPicker::new_grouped(server_groups, orphan_sessions);
                picker.activate_catchup_filter();
                self.session_picker_overlay = Some(RefCell::new(picker));
                self.session_picker_mode = SessionPickerMode::CatchUp;
            }
            Err(e) => {
                self.push_display_message(DisplayMessage::error(format!(
                    "Failed to load catch-up sessions: {}",
                    e
                )));
            }
        }
    }

    pub(super) fn handle_session_picker_selection(&mut self, targets: &[ResumeTarget]) {
        if targets.is_empty() {
            return;
        }

        if self.session_picker_mode == SessionPickerMode::CatchUp {
            let current_session_id = super::commands::active_session_id(self);
            let mut names = Vec::with_capacity(targets.len());
            for target in targets {
                let ResumeTarget::JcodeSession { session_id } = target else {
                    continue;
                };
                let queue_position = catchup_queue_position(&current_session_id, session_id);
                self.queue_catchup_resume(
                    session_id.to_string(),
                    Some(current_session_id.clone()),
                    queue_position,
                    true,
                );
                names.push(
                    crate::id::extract_session_name(session_id)
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| session_id.to_string()),
                );
            }

            if names.len() == 1 {
                self.push_display_message(DisplayMessage::system(format!(
                    "Queued Catch Up for **{}**.",
                    names[0],
                )));
                self.set_status_notice(format!("Catch Up → {}", names[0]));
            } else {
                self.push_display_message(DisplayMessage::system(format!(
                    "Queued Catch Up for **{} sessions**: {}.",
                    names.len(),
                    names.join(", "),
                )));
                self.set_status_notice(format!("Catch Up → {} sessions", names.len()));
            }
            return;
        }

        let default_cwd = std::env::current_dir().unwrap_or_default();
        let socket = std::env::var("JCODE_SOCKET").ok();
        let mut spawned = 0usize;
        let mut failed = Vec::new();
        let mut names = Vec::with_capacity(targets.len());

        for target in targets {
            let mut cwd = default_cwd.clone();
            if let Some(picker_cell) = self.session_picker_overlay.as_ref() {
                let picker = picker_cell.borrow();
                if let Some(session) = picker.session_for_target(target) {
                    if let Some(dir) = session.working_dir.as_deref() {
                        if std::path::Path::new(dir).is_dir() {
                            cwd = std::path::PathBuf::from(dir);
                        }
                    }
                }
            }

            let name = match target {
                ResumeTarget::JcodeSession { session_id } => {
                    crate::id::extract_session_name(session_id)
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| session_id.to_string())
                }
                ResumeTarget::CodexSession { session_id } => {
                    format!("Codex {}", &session_id[..session_id.len().min(8)])
                }
                ResumeTarget::PiSession { session_path } => std::path::Path::new(session_path)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Pi session")
                    .to_string(),
                ResumeTarget::OpenCodeSession { session_id } => {
                    format!("OpenCode {}", &session_id[..session_id.len().min(8)])
                }
            };
            match spawn_resume_target_in_new_terminal(target, &cwd, socket.as_deref()) {
                Ok(true) => {
                    spawned += 1;
                    names.push(name);
                }
                Ok(false) | Err(_) => {
                    failed.push(resume_target_manual_command(target, socket.as_deref()))
                }
            }
        }

        if spawned > 0 && failed.is_empty() {
            if names.len() == 1 {
                self.push_display_message(DisplayMessage::system(format!(
                    "Resumed **{}** in new window.",
                    names[0],
                )));
                self.set_status_notice(format!("Resumed {}", names[0]));
            } else {
                self.push_display_message(DisplayMessage::system(format!(
                    "Resumed **{} sessions** in new windows: {}.",
                    names.len(),
                    names.join(", "),
                )));
                self.set_status_notice(format!("Resumed {} sessions", names.len()));
            }
            return;
        }

        let manual: Vec<String> = failed.iter().map(|cmd| format!("  {}", cmd)).collect();

        if spawned > 0 {
            self.push_display_message(DisplayMessage::system(format!(
                "Resumed **{} session(s)** in new windows. {} failed:\n```\n{}\n```",
                spawned,
                failed.len(),
                manual.join("\n")
            )));
            self.set_status_notice(format!("Resumed {} session(s)", spawned));
        } else {
            self.push_display_message(DisplayMessage::system(format!(
                "No terminal found. Resume manually:\n```\n{}\n```",
                manual.join("\n")
            )));
        }
    }

    pub(super) fn handle_batch_crash_restore(&mut self) {
        let recovered = match crate::session::recover_crashed_sessions() {
            Ok(ids) => ids,
            Err(e) => {
                self.push_display_message(DisplayMessage::error(format!(
                    "Failed to recover crashed sessions: {}",
                    e
                )));
                return;
            }
        };

        if recovered.is_empty() {
            self.push_display_message(DisplayMessage::system(
                "No crashed sessions found to restore.".to_string(),
            ));
            return;
        }

        let exe = launch_client_executable();
        let cwd = std::env::current_dir().unwrap_or_default();
        let socket = std::env::var("JCODE_SOCKET").ok();
        let mut spawned = 0usize;
        let mut failed = Vec::new();

        for session_id in &recovered {
            let mut session_cwd = cwd.clone();
            if let Ok(session) = crate::session::Session::load(session_id) {
                if let Some(dir) = session.working_dir.as_deref() {
                    if std::path::Path::new(dir).is_dir() {
                        session_cwd = std::path::PathBuf::from(dir);
                    }
                }
            }

            match spawn_in_new_terminal(&exe, session_id, &session_cwd, socket.as_deref()) {
                Ok(true) => spawned += 1,
                Ok(false) => failed.push(session_id.clone()),
                Err(e) => {
                    crate::logging::error(&format!(
                        "Failed to spawn session {}: {}",
                        session_id, e
                    ));
                    failed.push(session_id.clone());
                }
            }
        }

        if spawned > 0 && failed.is_empty() {
            self.push_display_message(DisplayMessage::system(format!(
                "Restored {} crashed session(s) in new windows.",
                spawned
            )));
            self.set_status_notice(format!("Restored {} session(s)", spawned));
        } else if spawned > 0 {
            let manual: Vec<String> = failed
                .iter()
                .map(|id| format!("  jcode --resume {}", id))
                .collect();
            self.push_display_message(DisplayMessage::system(format!(
                "Restored {} session(s) in new windows. {} failed:\n```\n{}\n```",
                spawned,
                failed.len(),
                manual.join("\n")
            )));
        } else {
            let manual: Vec<String> = recovered
                .iter()
                .map(|id| format!("  jcode --resume {}", id))
                .collect();
            self.push_display_message(DisplayMessage::system(format!(
                "No terminal found. Resume manually:\n```\n{}\n```",
                manual.join("\n")
            )));
        }
    }

    pub(super) fn handle_session_picker_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> Result<()> {
        let action = {
            let Some(picker_cell) = self.session_picker_overlay.as_ref() else {
                return Ok(());
            };
            let mut picker = picker_cell.borrow_mut();
            picker.handle_overlay_key(code, modifiers)?
        };
        match action {
            OverlayAction::Continue => {}
            OverlayAction::Close => {
                self.session_picker_overlay = None;
                self.session_picker_mode = SessionPickerMode::Resume;
            }
            OverlayAction::Selected(PickerResult::Selected(ids)) => {
                self.handle_session_picker_selection(&ids);
                if let Some(picker_cell) = self.session_picker_overlay.as_ref() {
                    picker_cell.borrow_mut().clear_selected_sessions();
                }
            }
            OverlayAction::Selected(PickerResult::RestoreAllCrashed) => {
                self.handle_batch_crash_restore();
            }
        }
        Ok(())
    }

    pub(super) fn handle_picker_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> Result<()> {
        match code {
            KeyCode::Esc => {
                if let Some(ref mut picker) = self.picker_state {
                    if !picker.filter.is_empty() {
                        picker.filter.clear();
                        Self::apply_picker_filter(picker);
                        return Ok(());
                    }
                }
                self.picker_state = None;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let vim_nav = self
                    .picker_state
                    .as_ref()
                    .map(|picker| picker.kind == PickerKind::Account)
                    .unwrap_or(false);
                if matches!(code, KeyCode::Char('k'))
                    && !modifiers.contains(KeyModifiers::CONTROL)
                    && !vim_nav
                {
                    if let Some(ref mut picker) = self.picker_state {
                        picker.filter.push('k');
                        Self::apply_picker_filter(picker);
                    }
                    return Ok(());
                }
                if let Some(ref mut picker) = self.picker_state {
                    if picker.column == 0 {
                        picker.selected = picker.selected.saturating_sub(1);
                    } else if let Some(&idx) = picker.filtered.get(picker.selected) {
                        let entry = &mut picker.entries[idx];
                        entry.selected_option = entry.selected_option.saturating_sub(1);
                    }
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let vim_nav = self
                    .picker_state
                    .as_ref()
                    .map(|picker| picker.kind == PickerKind::Account)
                    .unwrap_or(false);
                if matches!(code, KeyCode::Char('j'))
                    && !modifiers.contains(KeyModifiers::CONTROL)
                    && !vim_nav
                {
                    if let Some(ref mut picker) = self.picker_state {
                        picker.filter.push('j');
                        Self::apply_picker_filter(picker);
                    }
                    return Ok(());
                }
                if let Some(ref mut picker) = self.picker_state {
                    if picker.column == 0 {
                        let max = picker.filtered.len().saturating_sub(1);
                        picker.selected = (picker.selected + 1).min(max);
                    } else if let Some(&idx) = picker.filtered.get(picker.selected) {
                        let entry = &mut picker.entries[idx];
                        let max = entry.options.len().saturating_sub(1);
                        entry.selected_option = (entry.selected_option + 1).min(max);
                    }
                }
            }
            KeyCode::Right => {
                if let Some(ref mut picker) = self.picker_state {
                    if picker.kind == PickerKind::Account {
                        return Ok(());
                    }
                    if picker.column < 2 {
                        if let Some(&idx) = picker.filtered.get(picker.selected) {
                            if picker.entries[idx].options.len() > 1 || picker.column > 0 {
                                picker.column += 1;
                            }
                        }
                    }
                }
            }
            KeyCode::Left | KeyCode::BackTab => {
                if let Some(ref mut picker) = self.picker_state {
                    if picker.kind == PickerKind::Account {
                        return Ok(());
                    }
                    if picker.column > 0 {
                        picker.column -= 1;
                    }
                }
            }
            KeyCode::Tab => {
                if let Some(ref mut picker) = self.picker_state {
                    if picker.kind == PickerKind::Account {
                        return Ok(());
                    }
                    if picker.column == 0 && !picker.filter.is_empty() {
                        Self::tab_complete_filter(picker);
                    } else if picker.column < 2 {
                        if let Some(&idx) = picker.filtered.get(picker.selected) {
                            if picker.entries[idx].options.len() > 1 || picker.column > 0 {
                                picker.column += 1;
                            }
                        }
                    }
                }
            }
            KeyCode::Char('d') if modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(ref picker) = self.picker_state {
                    if picker.kind == PickerKind::Account {
                        return Ok(());
                    }
                    if picker.filtered.is_empty() {
                        return Ok(());
                    }
                    let idx = picker.filtered[picker.selected];
                    let entry = &picker.entries[idx];
                    if !matches!(entry.action, PickerAction::Model) {
                        return Ok(());
                    }
                    let route = entry.options.get(entry.selected_option);

                    let bare_name = model_entry_base_name(entry);

                    let (model_spec, provider_key) = if let Some(r) = route {
                        let spec = if r.api_method == "copilot" {
                            format!("copilot:{}", bare_name)
                        } else if r.api_method == "cursor" {
                            format!("cursor:{}", bare_name)
                        } else if r.api_method == "openrouter" && r.provider != "auto" {
                            if bare_name.contains('/') {
                                format!("{}@{}", bare_name, r.provider)
                            } else {
                                format!("anthropic/{}@{}", bare_name, r.provider)
                            }
                        } else if r.api_method == "openrouter" {
                            bare_name.clone()
                        } else {
                            bare_name.clone()
                        };
                        let pkey = match r.api_method.as_str() {
                            "claude-oauth" | "api-key"
                                if crate::provider::provider_for_model(&bare_name)
                                    == Some("claude") =>
                            {
                                Some("claude")
                            }
                            "openai-oauth" => Some("openai"),
                            "copilot" => Some("copilot"),
                            "cursor" => Some("cursor"),
                            "openrouter" => Some("openrouter"),
                            _ => None,
                        };
                        (spec, pkey)
                    } else {
                        (bare_name.clone(), None)
                    };

                    let notice = format!(
                        "Default → {} via {}",
                        model_spec,
                        provider_key.unwrap_or("auto")
                    );

                    match crate::config::Config::set_default_model(Some(&model_spec), provider_key)
                    {
                        Ok(()) => self.set_status_notice(notice),
                        Err(e) => self.set_status_notice(format!("Failed to save default: {}", e)),
                    }
                    self.picker_state = None;
                }
            }
            KeyCode::Enter => {
                let Some(ref mut picker) = self.picker_state else {
                    return Ok(());
                };
                if picker.filtered.is_empty() {
                    return Ok(());
                }
                let idx = picker.filtered[picker.selected];
                let entry = picker.entries[idx].clone();

                if matches!(entry.action, PickerAction::Model) {
                    if picker.column == 0 && entry.options.len() > 1 {
                        picker.column = 1;
                        return Ok(());
                    }
                    if picker.column == 1 {
                        picker.column = 2;
                        return Ok(());
                    }
                }

                let route = &entry.options[entry.selected_option];

                if !route.available {
                    let detail = if route.detail.is_empty() {
                        "not available".to_string()
                    } else {
                        route.detail.clone()
                    };
                    self.picker_state = None;
                    self.set_status_notice(format!("{} — {}", entry.name, detail));
                    return Ok(());
                }

                match entry.action {
                    PickerAction::Account(selection) => {
                        self.picker_state = None;
                        self.handle_account_picker_selection(selection);
                    }
                    PickerAction::Login(provider) => {
                        self.picker_state = None;
                        self.start_login_provider(provider);
                    }
                    PickerAction::Usage {
                        id,
                        title,
                        subtitle,
                        status,
                        detail_lines,
                    } => {
                        self.picker_state = None;
                        self.usage_overlay = Some(std::cell::RefCell::new(
                            crate::tui::usage_overlay::UsageOverlay::new(
                                "Usage",
                                vec![crate::tui::usage_overlay::UsageOverlayItem::new(
                                    id,
                                    title.clone(),
                                    subtitle,
                                    status,
                                    detail_lines,
                                )],
                                crate::tui::usage_overlay::UsageOverlaySummary::default(),
                            ),
                        ));
                        self.set_status_notice(format!("Usage → {}", title));
                    }
                    PickerAction::AgentTarget(target) => {
                        self.open_agent_model_picker(target);
                    }
                    PickerAction::AgentModelChoice {
                        target,
                        clear_override,
                    } => {
                        self.picker_state = None;
                        let result = if clear_override {
                            save_agent_model_override(target, None)
                        } else {
                            let spec = model_entry_saved_spec(&entry);
                            save_agent_model_override(target, Some(&spec))
                        };
                        match result {
                            Ok(()) => {
                                let label = agent_model_target_label(target);
                                if clear_override {
                                    self.push_display_message(DisplayMessage::system(format!(
                                        "{} model override cleared. It now inherits `{}`.",
                                        label,
                                        agent_model_default_summary(target, self)
                                    )));
                                    self.set_status_notice(format!("{} model: inherit", label));
                                } else {
                                    let spec = model_entry_saved_spec(&entry);
                                    self.push_display_message(DisplayMessage::system(format!(
                                        "Saved {} model override: `{}`.",
                                        label, spec
                                    )));
                                    self.set_status_notice(format!("{} model → {}", label, spec));
                                }
                            }
                            Err(error) => {
                                self.push_display_message(DisplayMessage::error(format!(
                                    "Failed to save {} model override: {}",
                                    agent_model_target_label(target),
                                    error
                                )));
                                self.set_status_notice("Agent model save failed");
                            }
                        }
                    }
                    PickerAction::Model => {
                        let bare_name = model_entry_base_name(&entry);

                        let spec = if route.api_method == "openrouter" && route.provider != "auto" {
                            if entry.name.contains('/') {
                                format!("{}@{}", entry.name, route.provider)
                            } else {
                                format!("anthropic/{}@{}", entry.name, route.provider)
                            }
                        } else if route.api_method == "openrouter" {
                            entry.name.clone()
                        } else if route.api_method == "cursor" {
                            format!("cursor:{}", bare_name)
                        } else if route.provider == "Copilot" {
                            format!("copilot:{}", bare_name)
                        } else {
                            bare_name.clone()
                        };

                        let effort = entry.effort.clone();
                        let notice = format!(
                            "Model → {} via {} ({})",
                            entry.name, route.provider, route.api_method
                        );

                        self.picker_state = None;
                        self.upstream_provider = None;
                        if self.is_remote {
                            self.pending_model_switch = Some(spec);
                        } else {
                            let _ = self.provider.set_model(&spec);
                        }
                        if let Some(effort) = effort {
                            let _ = self.provider.set_reasoning_effort(&effort);
                        }
                        self.set_status_notice(notice);
                    }
                }
            }
            KeyCode::Backspace => {
                if let Some(ref mut picker) = self.picker_state {
                    if picker.filter.pop().is_some() {
                        Self::apply_picker_filter(picker);
                    }
                }
            }
            KeyCode::Char(c) => {
                if let Some(ref mut picker) = self.picker_state {
                    if !c.is_whitespace() {
                        picker.filter.push(c);
                        Self::apply_picker_filter(picker);
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn picker_fuzzy_match_positions(pattern: &str, text: &str) -> Option<Vec<usize>> {
        let pat: Vec<char> = pattern
            .to_lowercase()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        let txt: Vec<char> = text.to_lowercase().chars().collect();
        if pat.is_empty() {
            return Some(Vec::new());
        }

        let mut pi = 0;
        let mut positions = Vec::new();

        for (ti, &tc) in txt.iter().enumerate() {
            if pi < pat.len() && tc == pat[pi] {
                positions.push(ti);
                pi += 1;
            }
        }

        if pi == pat.len() {
            Some(positions)
        } else {
            None
        }
    }

    pub(super) fn picker_fuzzy_score(pattern: &str, text: &str) -> Option<i32> {
        let pat: Vec<char> = pattern
            .to_lowercase()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        let txt: Vec<char> = text.to_lowercase().chars().collect();
        if pat.is_empty() {
            return Some(0);
        }

        let mut pi = 0;
        let mut score = 0i32;
        let mut last_match: Option<usize> = None;

        for (ti, &tc) in txt.iter().enumerate() {
            if pi < pat.len() && tc == pat[pi] {
                score += 1;
                if let Some(last) = last_match {
                    if last + 1 == ti {
                        score += 3;
                    }
                }
                if ti == 0
                    || matches!(
                        txt.get(ti.wrapping_sub(1)),
                        Some('/' | '-' | '_' | ' ' | '.')
                    )
                {
                    score += 5;
                }
                if pi == 0 && ti == 0 {
                    score += 10;
                }
                last_match = Some(ti);
                pi += 1;
            }
        }

        if pi == pat.len() {
            score -= (txt.len() as i32) / 10;
            Some(score)
        } else {
            None
        }
    }

    pub(super) fn apply_picker_filter(picker: &mut PickerState) {
        if picker.filter.is_empty() {
            picker.filtered = (0..picker.entries.len()).collect();
        } else {
            let mut scored: Vec<(usize, i32)> = picker
                .entries
                .iter()
                .enumerate()
                .filter_map(|(i, m)| {
                    let filter_text = if picker.kind == PickerKind::Account {
                        let provider = m
                            .options
                            .get(m.selected_option)
                            .map(|route| route.provider.as_str())
                            .unwrap_or("");
                        let state = match &m.action {
                            PickerAction::Account(AccountPickerAction::Switch { .. }) => {
                                if m.is_current { "active" } else { "saved" }
                            }
                            PickerAction::Account(AccountPickerAction::Add { .. }) => "add",
                            PickerAction::Account(AccountPickerAction::Replace { .. }) => "replace",
                            PickerAction::Account(AccountPickerAction::OpenCenter { .. }) => {
                                "manage"
                            }
                            PickerAction::Model
                            | PickerAction::Usage { .. }
                            | PickerAction::Login(_)
                            | PickerAction::AgentTarget(_)
                            | PickerAction::AgentModelChoice { .. } => "",
                        };
                        format!("{} {} {}", m.name, provider, state)
                    } else if picker.kind == PickerKind::Login {
                        let auth_kind = m
                            .options
                            .get(m.selected_option)
                            .map(|route| route.provider.as_str())
                            .unwrap_or("");
                        let state = m
                            .options
                            .get(m.selected_option)
                            .map(|route| route.api_method.as_str())
                            .unwrap_or("");
                        let detail = m
                            .options
                            .get(m.selected_option)
                            .map(|route| route.detail.as_str())
                            .unwrap_or("");
                        format!("{} {} {} {}", m.name, auth_kind, state, detail)
                    } else if picker.kind == PickerKind::Usage {
                        let status = m
                            .options
                            .get(m.selected_option)
                            .map(|route| route.provider.as_str())
                            .unwrap_or("");
                        let window = m
                            .options
                            .get(m.selected_option)
                            .map(|route| route.api_method.as_str())
                            .unwrap_or("");
                        let detail = m
                            .options
                            .get(m.selected_option)
                            .map(|route| route.detail.as_str())
                            .unwrap_or("");
                        format!("{} {} {} {}", m.name, status, window, detail)
                    } else {
                        m.name.clone()
                    };
                    Self::picker_fuzzy_score(&picker.filter, &filter_text).map(|s| {
                        let bonus = if m.recommended { 5 } else { 0 };
                        (i, s + bonus)
                    })
                })
                .collect();
            scored.sort_by(|a, b| {
                b.1.cmp(&a.1)
                    .then(
                        picker.entries[a.0]
                            .recommendation_rank
                            .cmp(&picker.entries[b.0].recommendation_rank),
                    )
                    .then(picker.entries[a.0].name.cmp(&picker.entries[b.0].name))
            });
            picker.filtered = scored.into_iter().map(|(i, _)| i).collect();
        }
        if picker.filtered.is_empty() {
            picker.selected = 0;
        } else {
            picker.selected = picker.selected.min(picker.filtered.len() - 1);
        }
    }

    pub(super) fn tab_complete_filter(picker: &mut PickerState) {
        if picker.filtered.is_empty() {
            return;
        }
        if picker.filtered.len() == 1 {
            let name = picker.entries[picker.filtered[0]].name.clone();
            picker.filter = name;
            Self::apply_picker_filter(picker);
            return;
        }
        let names: Vec<&str> = picker
            .filtered
            .iter()
            .map(|&i| picker.entries[i].name.as_str())
            .collect();
        let first = names[0].to_lowercase();
        let first_chars: Vec<char> = first.chars().collect();
        let mut prefix_len = first_chars.len();
        for name in names.iter().skip(1) {
            let lower = (*name).to_lowercase();
            let chars: Vec<char> = lower.chars().collect();
            let mut common = 0;
            for (a, b) in first_chars.iter().zip(chars.iter()) {
                if a == b {
                    common += 1;
                } else {
                    break;
                }
            }
            prefix_len = prefix_len.min(common);
        }
        if prefix_len > picker.filter.len() {
            let first_original = &picker.entries[picker.filtered[0]].name;
            picker.filter = first_original[..prefix_len].to_string();
            Self::apply_picker_filter(picker);
        }
    }
}
