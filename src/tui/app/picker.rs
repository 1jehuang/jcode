use super::*;
use crate::tui::session_picker::{self, OverlayAction, PickerResult, SessionPicker};
use crate::tui::{
    AccountPickerSelection, ModelEntry, PickerKind, PickerSelection, PickerState, RouteOption,
};

enum InlinePickerPreviewRequest {
    Model {
        filter: String,
    },
    Account {
        provider_filter: Option<String>,
        filter: String,
    },
}

impl App {
    pub(super) fn model_picker_preview_filter(input: &str) -> Option<String> {
        let trimmed = input.trim_start();
        for cmd in ["/model", "/models"] {
            if let Some(rest) = trimmed.strip_prefix(cmd) {
                if rest.is_empty() {
                    return Some(String::new());
                }
                if rest
                    .chars()
                    .next()
                    .map(|c| c.is_whitespace())
                    .unwrap_or(false)
                {
                    return Some(rest.trim_start().to_string());
                }
            }
        }
        None
    }

    fn account_picker_preview_request(input: &str) -> Option<InlinePickerPreviewRequest> {
        let _ = input;
        None
    }

    fn inline_picker_preview_request(input: &str) -> Option<InlinePickerPreviewRequest> {
        Self::model_picker_preview_filter(input)
            .map(|filter| InlinePickerPreviewRequest::Model { filter })
            .or_else(|| Self::account_picker_preview_request(input))
    }

    pub(super) fn sync_model_picker_preview_from_input(&mut self) {
        let Some(request) = Self::inline_picker_preview_request(&self.input) else {
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

        let should_open = match (&request, self.picker_state.as_ref()) {
            (_, None) => true,
            (InlinePickerPreviewRequest::Model { .. }, Some(picker)) => {
                picker.preview && picker.kind != PickerKind::Model
            }
            (
                InlinePickerPreviewRequest::Account {
                    provider_filter, ..
                },
                Some(picker),
            ) => {
                let desired_provider =
                    self.inline_account_picker_provider_id(provider_filter.as_deref());
                !picker.preview
                    || picker.kind != PickerKind::Account
                    || desired_provider.as_deref()
                        != picker
                            .models
                            .first()
                            .and_then(|entry| match entry.selection {
                                PickerSelection::Account(
                                    AccountPickerSelection::Switch {
                                        ref provider_id, ..
                                    }
                                    | AccountPickerSelection::Add { ref provider_id }
                                    | AccountPickerSelection::Replace {
                                        ref provider_id, ..
                                    },
                                ) => Some(provider_id.as_str()),
                                PickerSelection::Model => None,
                            })
            }
        };

        if should_open {
            let saved_input = self.input.clone();
            let saved_cursor = self.cursor_pos;
            match &request {
                InlinePickerPreviewRequest::Model { .. } => self.open_model_picker(),
                InlinePickerPreviewRequest::Account {
                    provider_filter, ..
                } => self.open_account_picker(provider_filter.as_deref()),
            }
            if let Some(ref mut picker) = self.picker_state {
                picker.preview = true;
            }
            // Preview must not steal the user's command input.
            self.input = saved_input;
            self.cursor_pos = saved_cursor;
        }

        if let Some(ref mut picker) = self.picker_state {
            if picker.preview {
                picker.filter = match request {
                    InlinePickerPreviewRequest::Model { filter }
                    | InlinePickerPreviewRequest::Account { filter, .. } => filter,
                };
                Self::apply_picker_filter(picker);
            }
        }
    }

    pub(super) fn activate_model_picker_from_preview(&mut self) -> bool {
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
        self.input.clear();
        self.cursor_pos = 0;
        let _ = self.handle_picker_key(KeyCode::Enter, KeyModifiers::NONE);
        true
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
            if !self.remote_model_routes.is_empty() {
                self.remote_model_routes.clone()
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
        let mut model_routes: BTreeMap<String, Vec<RouteOption>> = BTreeMap::new();
        for r in &routes {
            if !model_routes.contains_key(&r.model) {
                model_order.push(r.model.clone());
            }
            model_routes
                .entry(r.model.clone())
                .or_default()
                .push(RouteOption {
                    provider: r.provider.clone(),
                    api_method: r.api_method.clone(),
                    available: r.available,
                    detail: r.detail.clone(),
                    estimated_reference_cost_micros: r.estimated_reference_cost_micros(),
                });
        }

        fn route_sort_key(r: &RouteOption) -> (u8, u8, u64, String) {
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

        let mut models: Vec<ModelEntry> = Vec::new();
        for name in &model_order {
            let mut entry_routes = model_routes.remove(name).unwrap_or_default();
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
                    models.push(ModelEntry {
                        name: display_name,
                        routes: entry_routes.clone(),
                        selection: PickerSelection::Model,
                        selected_route: 0,
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
                models.push(ModelEntry {
                    name: name.clone(),
                    routes: entry_routes,
                    selection: PickerSelection::Model,
                    selected_route: 0,
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

        models.sort_by(|a, b| {
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
            let a_avail = if a.routes.first().map(|r| r.available).unwrap_or(false) {
                0u8
            } else {
                1
            };
            let b_avail = if b.routes.first().map(|r| r.available).unwrap_or(false) {
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
            filtered: (0..models.len()).collect(),
            models,
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
        for model in &self.remote_available_models {
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

            if crate::provider::ALL_CLAUDE_MODELS.contains(&model.as_str())
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
            && !crate::provider::ALL_CLAUDE_MODELS.contains(&model)
            && !crate::provider::ALL_OPENAI_MODELS.contains(&model)
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

    fn handle_account_picker_selection(&mut self, selection: AccountPickerSelection) {
        match selection {
            AccountPickerSelection::Switch { provider_id, label } => {
                if self.is_remote {
                    self.pending_account_picker_selection = Some(AccountPickerSelection::Switch {
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
            AccountPickerSelection::Add { provider_id } => match provider_id.as_str() {
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
            AccountPickerSelection::Replace { provider_id, label } => match provider_id.as_str() {
                "claude" => self.start_claude_login_for_account(&label),
                "openai" => self.start_openai_login_for_account(&label),
                _ => self.push_display_message(DisplayMessage::error(format!(
                    "Provider `{}` does not support account replacement.",
                    provider_id
                ))),
            },
        }
    }

    pub(super) fn open_session_picker(&mut self) {
        match session_picker::load_sessions_grouped() {
            Ok((server_groups, orphan_sessions)) => {
                let picker = SessionPicker::new_grouped(server_groups, orphan_sessions);
                self.session_picker_overlay = Some(RefCell::new(picker));
            }
            Err(e) => {
                self.push_display_message(DisplayMessage::error(format!(
                    "Failed to load sessions: {}",
                    e
                )));
            }
        }
    }

    pub(super) fn handle_session_picker_selection(&mut self, session_id: &str) {
        let exe = std::env::current_exe().unwrap_or_default();
        let mut cwd = std::env::current_dir().unwrap_or_default();
        if let Ok(session) = crate::session::Session::load(session_id) {
            if let Some(dir) = session.working_dir.as_deref() {
                if std::path::Path::new(dir).is_dir() {
                    cwd = std::path::PathBuf::from(dir);
                }
            }
        }
        let socket = std::env::var("JCODE_SOCKET").ok();
        match spawn_in_new_terminal(&exe, session_id, &cwd, socket.as_deref()) {
            Ok(true) => {
                let name = crate::id::extract_session_name(session_id)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| session_id.to_string());
                self.push_display_message(DisplayMessage::system(format!(
                    "Resumed **{}** in new window.",
                    name,
                )));
                self.set_status_notice(format!("Resumed {}", name));
            }
            Ok(false) => {
                self.push_display_message(DisplayMessage::system(format!(
                    "No terminal found. Resume manually:\n```\njcode --resume {}\n```",
                    session_id,
                )));
            }
            Err(e) => {
                self.push_display_message(DisplayMessage::error(format!(
                    "Failed to open window: {}\n\nResume manually: `jcode --resume {}`",
                    e, session_id,
                )));
            }
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

        let exe = std::env::current_exe().unwrap_or_default();
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
            }
            OverlayAction::Selected(PickerResult::Selected(id)) => {
                self.session_picker_overlay = None;
                self.handle_session_picker_selection(&id);
            }
            OverlayAction::Selected(PickerResult::RestoreAllCrashed) => {
                self.session_picker_overlay = None;
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
                        let entry = &mut picker.models[idx];
                        entry.selected_route = entry.selected_route.saturating_sub(1);
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
                        let entry = &mut picker.models[idx];
                        let max = entry.routes.len().saturating_sub(1);
                        entry.selected_route = (entry.selected_route + 1).min(max);
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
                            if picker.models[idx].routes.len() > 1 || picker.column > 0 {
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
                            if picker.models[idx].routes.len() > 1 || picker.column > 0 {
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
                    let entry = &picker.models[idx];
                    let route = entry.routes.get(entry.selected_route);

                    let bare_name = if entry.effort.is_some() {
                        entry
                            .name
                            .rsplit_once(" (")
                            .map(|(base, _)| base.to_string())
                            .unwrap_or_else(|| entry.name.clone())
                    } else {
                        entry.name.clone()
                    };

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
                                if crate::provider::ALL_CLAUDE_MODELS
                                    .contains(&bare_name.as_str()) =>
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
                let entry = picker.models[idx].clone();

                if matches!(entry.selection, PickerSelection::Model) {
                    if picker.column == 0 && entry.routes.len() > 1 {
                        picker.column = 1;
                        return Ok(());
                    }
                    if picker.column == 1 {
                        picker.column = 2;
                        return Ok(());
                    }
                }

                let route = &entry.routes[entry.selected_route];

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

                match entry.selection {
                    PickerSelection::Account(selection) => {
                        self.picker_state = None;
                        self.handle_account_picker_selection(selection);
                    }
                    PickerSelection::Model => {
                        let bare_name = if entry.effort.is_some() {
                            entry
                                .name
                                .rsplit_once(" (")
                                .map(|(base, _)| base.to_string())
                                .unwrap_or_else(|| entry.name.clone())
                        } else {
                            entry.name.clone()
                        };

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
            picker.filtered = (0..picker.models.len()).collect();
        } else {
            let mut scored: Vec<(usize, i32)> = picker
                .models
                .iter()
                .enumerate()
                .filter_map(|(i, m)| {
                    Self::picker_fuzzy_score(&picker.filter, &m.name).map(|s| {
                        let bonus = if m.recommended { 5 } else { 0 };
                        (i, s + bonus)
                    })
                })
                .collect();
            scored.sort_by(|a, b| {
                b.1.cmp(&a.1)
                    .then(
                        picker.models[a.0]
                            .recommendation_rank
                            .cmp(&picker.models[b.0].recommendation_rank),
                    )
                    .then(picker.models[a.0].name.cmp(&picker.models[b.0].name))
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
            let name = picker.models[picker.filtered[0]].name.clone();
            picker.filter = name;
            Self::apply_picker_filter(picker);
            return;
        }
        let names: Vec<&str> = picker
            .filtered
            .iter()
            .map(|&i| picker.models[i].name.as_str())
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
            let first_original = &picker.models[picker.filtered[0]].name;
            picker.filter = first_original[..prefix_len].to_string();
            Self::apply_picker_filter(picker);
        }
    }
}
