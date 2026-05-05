use super::*;
use crate::tui::session_picker::{self, OverlayAction, PickerResult, ResumeTarget, SessionPicker};
use crate::tui::{
    AccountPickerAction, InlineInteractiveState, PickerAction, PickerEntry, PickerKind,
    PickerOption,
};

#[path = "inline_interactive/helpers.rs"]
mod helpers;
#[path = "inline_interactive/openers.rs"]
mod openers;
#[path = "inline_interactive/preview.rs"]
mod preview;
#[path = "inline_interactive/preview_request.rs"]
mod preview_request;
use helpers::{
    agent_model_default_summary, agent_model_target_label, catchup_candidates,
    catchup_queue_position, model_entry_base_name, model_entry_saved_spec,
    openai_compatible_profile_id_for_route, openrouter_route_model_id, picker_route_model_spec,
    save_agent_model_override,
};

impl App {
    pub(super) fn invalidate_model_picker_cache(&mut self) {
        self.model_picker_cache = None;
        self.model_picker_catalog_revision = self.model_picker_catalog_revision.wrapping_add(1);
        self.pending_model_picker_load = None;
        self.model_picker_load_request_id = self.model_picker_load_request_id.wrapping_add(1);
    }

    fn model_route_cache_marker(route: &crate::provider::ModelRoute) -> String {
        format!(
            "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
            route.model, route.provider, route.api_method, route.available, route.detail
        )
    }

    fn model_picker_cache_signature(
        &self,
        current_model: &str,
        config_default_model: Option<String>,
        current_effort: Option<String>,
        available_efforts: &[&str],
    ) -> ModelPickerCacheSignature {
        ModelPickerCacheSignature {
            is_remote: self.is_remote,
            provider_name: if self.is_remote {
                self.remote_provider_name
                    .clone()
                    .unwrap_or_else(|| "remote".to_string())
            } else {
                self.provider.name().to_string()
            },
            current_model: current_model.to_string(),
            config_default_model,
            reasoning_effort: current_effort,
            available_efforts: available_efforts
                .iter()
                .map(|effort| (*effort).to_string())
                .collect(),
            simplified_model_picker: crate::perf::tui_policy().simplified_model_picker,
            catalog_revision: self.model_picker_catalog_revision,
            remote_provider_name: self.remote_provider_name.clone(),
            remote_available_len: self.remote_available_entries.len(),
            remote_available_first: self.remote_available_entries.first().cloned(),
            remote_available_last: self.remote_available_entries.last().cloned(),
            remote_routes_len: self.remote_model_options.len(),
            remote_routes_first: self
                .remote_model_options
                .first()
                .map(Self::model_route_cache_marker),
            remote_routes_last: self
                .remote_model_options
                .last()
                .map(Self::model_route_cache_marker),
        }
    }

    fn open_cached_model_picker_if_fresh(
        &mut self,
        signature: &ModelPickerCacheSignature,
        picker_started: std::time::Instant,
    ) -> bool {
        let Some(cache) = self.model_picker_cache.as_ref() else {
            return false;
        };
        if cache.signature != *signature {
            return false;
        }

        let entries = cache.entries.clone();
        let entry_count = entries.len();
        let route_count = cache.route_count;
        let model_count = cache.model_count;
        self.inline_view_state = None;
        self.inline_interactive_state = Some(InlineInteractiveState {
            kind: PickerKind::Model,
            filtered: (0..entry_count).collect(),
            entries,
            selected: 0,
            column: 0,
            filter: String::new(),
            preview: false,
        });
        self.input.clear();
        self.cursor_pos = 0;

        if std::env::var("JCODE_LOG_MODEL_PICKER_TIMING").is_ok() {
            crate::logging::info(&format!(
                "[TIMING] model_picker_open: cache_hit=true, remote={}, simplified={}, routes={}, models={}, entries={}, total={}ms",
                self.is_remote,
                crate::perf::tui_policy().simplified_model_picker,
                route_count,
                model_count,
                entry_count,
                picker_started.elapsed().as_millis(),
            ));
        }
        true
    }

    fn should_cache_model_picker_entries(model_count: usize, route_count: usize) -> bool {
        // A single model/route result is commonly a startup fallback (for example, the
        // current model while the real provider catalog is still loading). Caching that
        // fallback makes `/model` look permanently collapsed to just the active model.
        model_count > 1 && route_count > 1
    }

    fn simplified_model_routes_for_picker(
        &self,
        current_model: &str,
    ) -> Vec<crate::provider::ModelRoute> {
        let auth = crate::auth::AuthStatus::check_fast();
        let mut routes = Vec::new();

        for model in self.provider.available_models_display() {
            if !model.contains('/') && crate::provider::provider_for_model(&model) == Some("openai")
            {
                if auth.openai_has_oauth {
                    routes.push(crate::provider::ModelRoute {
                        model: model.clone(),
                        provider: "OpenAI".to_string(),
                        api_method: "openai-oauth".to_string(),
                        available: true,
                        detail: String::new(),
                        cheapness: None,
                    });
                }
                if auth.openai_has_api_key {
                    routes.push(crate::provider::ModelRoute {
                        model: model.clone(),
                        provider: "OpenAI".to_string(),
                        api_method: "openai-api-key".to_string(),
                        available: true,
                        detail: String::new(),
                        cheapness: None,
                    });
                }
                if auth.openai == crate::auth::AuthState::NotConfigured {
                    routes.push(crate::provider::ModelRoute {
                        model,
                        provider: "OpenAI".to_string(),
                        api_method: "openai-oauth".to_string(),
                        available: false,
                        detail: "no credentials".to_string(),
                        cheapness: None,
                    });
                }
                continue;
            }

            let (provider, api_method, available, detail) = if model.contains('/') {
                (
                    "auto".to_string(),
                    "openrouter".to_string(),
                    auth.openrouter != crate::auth::AuthState::NotConfigured,
                    "simplified catalog".to_string(),
                )
            } else {
                match crate::provider::provider_for_model(&model) {
                    Some("claude") => (
                        "Anthropic".to_string(),
                        "claude-oauth".to_string(),
                        auth.anthropic.has_oauth || auth.anthropic.has_api_key,
                        String::new(),
                    ),
                    Some("openai") => unreachable!("OpenAI models are handled above"),
                    Some("gemini") => (
                        "Gemini".to_string(),
                        "code-assist-oauth".to_string(),
                        auth.gemini != crate::auth::AuthState::NotConfigured,
                        String::new(),
                    ),
                    Some("cursor") => (
                        "Cursor".to_string(),
                        "cursor".to_string(),
                        auth.cursor != crate::auth::AuthState::NotConfigured,
                        String::new(),
                    ),
                    Some("openrouter") => (
                        "auto".to_string(),
                        "openrouter".to_string(),
                        auth.openrouter != crate::auth::AuthState::NotConfigured,
                        "simplified catalog".to_string(),
                    ),
                    Some(other) => (other.to_string(), other.to_string(), true, String::new()),
                    None => (
                        self.provider.name().to_string(),
                        "current".to_string(),
                        true,
                        String::new(),
                    ),
                }
            };

            routes.push(crate::provider::ModelRoute {
                model,
                provider,
                api_method,
                available,
                detail,
                cheapness: None,
            });
        }

        if routes.is_empty() && !current_model.is_empty() && current_model != "unknown" {
            routes.push(crate::provider::ModelRoute {
                model: current_model.to_string(),
                provider: self.provider.name().to_string(),
                api_method: "current".to_string(),
                available: true,
                detail: "simplified catalog".to_string(),
                cheapness: None,
            });
        }

        routes
    }

    pub(super) fn open_model_picker(&mut self) {
        let picker_started = std::time::Instant::now();
        const RECENT_AUTH_BOOST_TTL: std::time::Duration = std::time::Duration::from_secs(5 * 60);
        if self
            .recent_authenticated_provider
            .as_ref()
            .map(|(_, at)| at.elapsed() > RECENT_AUTH_BOOST_TTL)
            .unwrap_or(false)
        {
            self.recent_authenticated_provider = None;
            self.invalidate_model_picker_cache();
        }

        let current_model = if self.is_remote {
            self.remote_provider_model
                .clone()
                .unwrap_or_else(|| "unknown".to_string())
        } else {
            self.provider.model().to_string()
        };

        let config_default_model = crate::config::config().provider.default_model.clone();

        let current_effort = if self.is_remote {
            self.remote_reasoning_effort.clone()
        } else {
            self.provider.reasoning_effort()
        };
        let available_efforts = if self.is_remote {
            Vec::new()
        } else {
            self.provider.available_efforts()
        };

        let cache_signature = self.model_picker_cache_signature(
            &current_model,
            config_default_model.clone(),
            current_effort.clone(),
            &available_efforts,
        );
        if self.open_cached_model_picker_if_fresh(&cache_signature, picker_started) {
            return;
        }

        if !self.is_remote && !crate::perf::tui_policy().simplified_model_picker {
            self.open_loading_model_picker(&current_model);
            self.start_model_picker_route_load(cache_signature, picker_started);
            return;
        }

        let routes_started = std::time::Instant::now();
        let routes: Vec<crate::provider::ModelRoute> = if self.is_remote {
            if !self.remote_model_options.is_empty() {
                self.remote_model_options.clone()
            } else {
                self.build_remote_model_routes_fallback()
            }
        } else {
            self.simplified_model_routes_for_picker(&current_model)
        };
        let routes_ms = routes_started.elapsed().as_millis();

        self.open_model_picker_with_routes(
            cache_signature,
            picker_started,
            routes,
            routes_ms,
            false,
        );
    }

    fn open_loading_model_picker(&mut self, current_model: &str) {
        let model_label = if current_model.trim().is_empty() || current_model == "unknown" {
            "Loading models…".to_string()
        } else {
            current_model.to_string()
        };
        self.inline_view_state = None;
        self.inline_interactive_state = Some(InlineInteractiveState {
            kind: PickerKind::Model,
            filtered: vec![0],
            entries: vec![PickerEntry {
                name: model_label,
                options: vec![PickerOption {
                    provider: self.provider.name().to_string(),
                    api_method: "current".to_string(),
                    available: true,
                    detail: "updating model list…".to_string(),
                    estimated_reference_cost_micros: None,
                }],
                action: PickerAction::Model,
                selected_option: 0,
                is_current: true,
                is_default: false,
                recommended: false,
                recommendation_rank: usize::MAX,
                old: false,
                created_date: None,
                effort: None,
            }],
            selected: 0,
            column: 0,
            filter: String::new(),
            preview: false,
        });
        self.set_status_notice("Updating model list…");
    }

    fn start_model_picker_route_load(
        &mut self,
        signature: ModelPickerCacheSignature,
        picker_started: std::time::Instant,
    ) {
        self.model_picker_load_request_id = self.model_picker_load_request_id.wrapping_add(1);
        let request_id = self.model_picker_load_request_id;
        let provider = self.provider.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let build = move || {
            let routes_started = std::time::Instant::now();
            let routes = provider.model_routes();
            let routes_ms = routes_started.elapsed().as_millis();
            let _ = tx.send(Ok(ModelPickerRoutesResult { routes, routes_ms }));
        };

        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn_blocking(build);
        } else {
            std::thread::spawn(build);
        }

        self.pending_model_picker_load = Some(PendingModelPickerLoad {
            request_id,
            signature,
            picker_started,
            receiver: rx,
        });
    }

    pub(super) fn poll_model_picker_load(&mut self) -> bool {
        let Some(pending) = self.pending_model_picker_load.as_ref() else {
            return false;
        };

        let received = match pending.receiver.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.pending_model_picker_load = None;
                self.set_status_notice("Model list update failed");
                return true;
            }
        };

        let Some(pending) = self.pending_model_picker_load.take() else {
            return false;
        };
        if pending.request_id != self.model_picker_load_request_id {
            return false;
        }

        let current_model = if self.is_remote {
            self.remote_provider_model
                .clone()
                .unwrap_or_else(|| "unknown".to_string())
        } else {
            self.provider.model().to_string()
        };
        let config_default_model = crate::config::config().provider.default_model.clone();
        let current_effort = if self.is_remote {
            self.remote_reasoning_effort.clone()
        } else {
            self.provider.reasoning_effort()
        };
        let available_efforts = if self.is_remote {
            Vec::new()
        } else {
            self.provider.available_efforts()
        };
        let current_signature = self.model_picker_cache_signature(
            &current_model,
            config_default_model,
            current_effort,
            &available_efforts,
        );
        if current_signature != pending.signature {
            return false;
        }

        match received {
            Ok(result) => {
                self.open_model_picker_with_routes(
                    pending.signature,
                    pending.picker_started,
                    result.routes,
                    result.routes_ms,
                    true,
                );
                if self.inline_interactive_state.is_some() {
                    self.set_status_notice("Model list updated");
                }
                true
            }
            Err(error) => {
                self.set_status_notice(format!("Model list update failed: {}", error));
                true
            }
        }
    }

    fn open_model_picker_with_routes(
        &mut self,
        cache_signature: ModelPickerCacheSignature,
        picker_started: std::time::Instant,
        routes: Vec<crate::provider::ModelRoute>,
        routes_ms: u128,
        preserve_input: bool,
    ) {
        use std::collections::BTreeMap;

        let current_model = if self.is_remote {
            self.remote_provider_model
                .clone()
                .unwrap_or_else(|| "unknown".to_string())
        } else {
            self.provider.model().to_string()
        };
        let config_default_model = crate::config::config().provider.default_model.clone();
        let current_effort = if self.is_remote {
            self.remote_reasoning_effort.clone()
        } else {
            self.provider.reasoning_effort()
        };
        let available_efforts = if self.is_remote {
            Vec::new()
        } else {
            self.provider.available_efforts()
        };

        let is_config_default = |name: &str| -> bool {
            match &config_default_model {
                None => false,
                Some(default) => {
                    let bare = default.strip_prefix("copilot:").unwrap_or(default);
                    let bare = bare.strip_prefix("cursor:").unwrap_or(bare);
                    let bare = bare.strip_prefix("antigravity:").unwrap_or(bare);
                    let bare = bare.split('@').next().unwrap_or(bare);
                    name == default || name == bare
                }
            }
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
            self.inline_interactive_state = None;
            self.push_display_message(DisplayMessage::system(
                crate::tui::app::model_context::no_models_available_message(self.is_remote),
            ));
            self.set_status_notice("No models available");
            return;
        }

        let grouping_started = std::time::Instant::now();
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
        let grouping_ms = grouping_started.elapsed().as_millis();

        fn route_sort_key(r: &PickerOption) -> (u8, u8, u64, String) {
            let avail = if r.available { 0 } else { 1 };
            let method = match r.api_method.as_str() {
                "claude-oauth" | "openai-oauth" | "openai-api-key" => 0,
                "api-key" => 1,
                method if method.starts_with("openai-compatible") => 1,
                "cursor" => 2,
                "copilot" => 3,
                "openrouter" => 4,
                _ => 5,
            };
            let cheapness = r.estimated_reference_cost_micros.unwrap_or(u64::MAX);
            (avail, method, cheapness, r.provider.clone())
        }

        fn normalize_provider_label(value: &str) -> String {
            value
                .trim()
                .to_ascii_lowercase()
                .replace([' ', '_', '-'], "")
        }

        fn route_matches_recent_auth(route_provider: &str, login_provider: &str) -> bool {
            let route = normalize_provider_label(route_provider);
            let login = normalize_provider_label(login_provider);
            if route == login || route.contains(&login) || login.contains(&route) {
                return true;
            }
            matches!(
                (login.as_str(), route.as_str()),
                ("claude" | "anthropic", "anthropic" | "claude")
                    | ("openai", "openai")
                    | ("gemini" | "google", "gemini" | "google")
                    | ("antigravity", "antigravity")
                    | ("copilot" | "copilotcode", "copilot")
                    | ("cursor", "cursor")
                    | ("openrouter", "openrouter" | "auto")
            )
        }

        const RECOMMENDED_MODELS: &[&str] = &["gpt-5.5", "claude-opus-4-7", "moonshotai/kimi-k2.6"];

        const CLAUDE_OAUTH_ONLY_MODELS: &[&str] = &["claude-opus-4-7"];

        const OPENAI_OAUTH_ONLY_MODELS: &[&str] =
            &["gpt-5.5", "gpt-5.4", "gpt-5.4[1m]", "gpt-5.4-pro"];
        const COPILOT_OAUTH_MODELS: &[&str] = &["claude-opus-4.7", "gpt-5.5", "gpt-5.4"];

        fn recommendation_rank(name: &str, recommended_models: &[&str]) -> usize {
            recommended_models
                .iter()
                .position(|model| *model == name)
                .unwrap_or(usize::MAX)
        }

        fn route_can_be_recommended(route: &PickerOption) -> bool {
            matches!(
                route.api_method.as_str(),
                "claude-oauth" | "openai-oauth" | "openai-api-key" | "copilot"
            ) || (route.api_method == "openrouter" && route.provider == "auto")
        }

        let timestamp_started = std::time::Instant::now();
        let openrouter_created_timestamps =
            crate::provider::openrouter::load_model_timestamp_index();
        let timestamp_ms = timestamp_started.elapsed().as_millis();
        let openrouter_created_timestamp = |model: &str| {
            crate::provider::openrouter::model_created_timestamp_from_index(
                model,
                &openrouter_created_timestamps,
            )
        };

        let latest_recommended_ts: Option<u64> = RECOMMENDED_MODELS
            .iter()
            .filter_map(|m| openrouter_created_timestamp(m))
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

        let is_openai = !available_efforts.is_empty();
        let recent_auth_provider = self
            .recent_authenticated_provider
            .as_ref()
            .map(|(provider, _)| provider.as_str());

        let entries_started = std::time::Instant::now();
        let mut entries: Vec<PickerEntry> = Vec::new();
        for name in &model_order {
            let mut entry_routes = model_options.remove(name).unwrap_or_default();
            entry_routes.sort_by_key(route_sort_key);
            let recently_authenticated = recent_auth_provider
                .map(|provider| {
                    entry_routes
                        .iter()
                        .any(|route| route_matches_recent_auth(&route.provider, provider))
                })
                .unwrap_or(false);
            if recently_authenticated {
                for route in &mut entry_routes {
                    if recent_auth_provider
                        .map(|provider| route_matches_recent_auth(&route.provider, provider))
                        .unwrap_or(false)
                        && !route.detail.contains("recently added")
                    {
                        route.detail = if route.detail.trim().is_empty() {
                            "recently added".to_string()
                        } else {
                            format!("recently added · {}", route.detail)
                        };
                    }
                }
            }

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
                    let or_created = openrouter_created_timestamp(name);
                    for route in &entry_routes {
                        entries.push(PickerEntry {
                            name: display_name.clone(),
                            options: vec![route.clone()],
                            action: PickerAction::Model,
                            selected_option: 0,
                            is_current: is_this_current,
                            recommended: RECOMMENDED_MODELS.contains(&name.as_str())
                                && (*effort == "xhigh" || *effort == "high")
                                && (!(CLAUDE_OAUTH_ONLY_MODELS.contains(&name.as_str())
                                    || OPENAI_OAUTH_ONLY_MODELS.contains(&name.as_str())
                                    || COPILOT_OAUTH_MODELS.contains(&name.as_str()))
                                    || (route_can_be_recommended(route) && route.available)),
                            recommendation_rank: recommendation_rank(name, RECOMMENDED_MODELS),
                            old: old_threshold_secs > 0
                                && or_created.map(|t| t < old_threshold_secs).unwrap_or(false),
                            created_date: or_created.map(format_created),
                            effort: Some(effort.to_string()),
                            is_default: is_config_default(name),
                        });
                    }
                }
            } else {
                let or_created = openrouter_created_timestamp(name);
                let is_old = old_threshold_secs > 0
                    && or_created.map(|t| t < old_threshold_secs).unwrap_or(false);
                for route in entry_routes {
                    let is_recommended = RECOMMENDED_MODELS.contains(&name.as_str())
                        && (!(CLAUDE_OAUTH_ONLY_MODELS.contains(&name.as_str())
                            || OPENAI_OAUTH_ONLY_MODELS.contains(&name.as_str())
                            || COPILOT_OAUTH_MODELS.contains(&name.as_str()))
                            || (route_can_be_recommended(&route) && route.available));
                    entries.push(PickerEntry {
                        name: name.clone(),
                        options: vec![route],
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
        }

        entries.sort_by(|a, b| {
            let a_current = if a.is_current { 0u8 } else { 1 };
            let b_current = if b.is_current { 0u8 } else { 1 };
            let a_recent = if a
                .options
                .iter()
                .any(|option| option.detail.contains("recently added"))
            {
                0u8
            } else {
                1
            };
            let b_recent = if b
                .options
                .iter()
                .any(|option| option.detail.contains("recently added"))
            {
                0u8
            } else {
                1
            };
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
                .then(a_recent.cmp(&b_recent))
                .then(a_rec.cmp(&b_rec))
                .then(a_rec_rank.cmp(&b_rec_rank))
                .then(a_avail.cmp(&b_avail))
                .then(a_old.cmp(&b_old))
                .then(a.name.cmp(&b.name))
                .then_with(|| {
                    a.active_option()
                        .map(|route| route.provider.as_str())
                        .cmp(&b.active_option().map(|route| route.provider.as_str()))
                })
                .then_with(|| {
                    a.active_option()
                        .map(|route| route.api_method.as_str())
                        .cmp(&b.active_option().map(|route| route.api_method.as_str()))
                })
        });
        let entries_ms = entries_started.elapsed().as_millis();
        let total_ms = picker_started.elapsed().as_millis();

        if total_ms >= 250 || std::env::var("JCODE_LOG_MODEL_PICKER_TIMING").is_ok() {
            crate::logging::info(&format!(
                "[TIMING] model_picker_open: remote={}, simplified={}, routes={}, models={}, entries={}, routes={}ms, grouping={}ms, timestamps={}ms, entries_sort={}ms, total={}ms",
                self.is_remote,
                crate::perf::tui_policy().simplified_model_picker,
                routes.len(),
                model_order.len(),
                entries.len(),
                routes_ms,
                grouping_ms,
                timestamp_ms,
                entries_ms,
                total_ms,
            ));
        }

        let previous_picker = self.inline_interactive_state.as_ref().and_then(|picker| {
            if picker.kind == PickerKind::Model {
                Some((
                    picker.preview,
                    picker.filter.clone(),
                    picker.selected,
                    picker.column,
                ))
            } else {
                None
            }
        });
        let saved_input = if preserve_input {
            Some((self.input.clone(), self.cursor_pos))
        } else {
            None
        };

        self.inline_view_state = None;
        if Self::should_cache_model_picker_entries(model_order.len(), routes.len()) {
            self.model_picker_cache = Some(ModelPickerCache {
                signature: cache_signature,
                entries: entries.clone(),
                route_count: routes.len(),
                model_count: model_order.len(),
            });
        } else {
            self.model_picker_cache = None;
        }
        self.inline_interactive_state = Some(InlineInteractiveState {
            kind: PickerKind::Model,
            filtered: (0..entries.len()).collect(),
            entries,
            selected: 0,
            column: 0,
            filter: String::new(),
            preview: false,
        });

        if let Some((preview, filter, selected, column)) = previous_picker
            && let Some(ref mut picker) = self.inline_interactive_state
        {
            picker.preview = preview;
            picker.filter = filter;
            picker.selected = selected.min(picker.filtered.len().saturating_sub(1));
            picker.column = column.min(picker.max_navigable_column());
            Self::apply_inline_interactive_filter(picker);
        }

        if let Some((input, cursor_pos)) = saved_input {
            self.input = input;
            self.cursor_pos = cursor_pos;
        } else {
            self.input.clear();
            self.cursor_pos = 0;
        }
    }

    pub(super) fn build_remote_model_routes_fallback(&self) -> Vec<crate::provider::ModelRoute> {
        let auth = crate::auth::AuthStatus::check_fast();
        let mut routes = Vec::new();
        for model in &self.remote_available_entries {
            if !crate::provider::is_listable_model_name(model) {
                continue;
            }

            let openrouter_catalog_model = crate::provider::openrouter_catalog_model_id(model);
            let openrouter_cached = openrouter_catalog_model
                .as_deref()
                .and_then(crate::provider::openrouter::load_endpoints_disk_cache_public);

            if model.contains('/') {
                let cached = openrouter_cached;
                let auto_detail = cached
                    .as_ref()
                    .and_then(|(eps, _)| eps.first().map(|ep| format!("→ {}", ep.provider_name)))
                    .unwrap_or_default();
                routes.push(crate::provider::build_openrouter_auto_route(
                    model,
                    auth.openrouter != crate::auth::AuthState::NotConfigured,
                    auto_detail,
                ));
                if let Some((endpoints, age)) = cached {
                    let age_str = if age < 3600 {
                        format!("{}m ago", age / 60)
                    } else if age < 86400 {
                        format!("{}h ago", age / 3600)
                    } else {
                        format!("{}d ago", age / 86400)
                    };
                    for ep in &endpoints {
                        routes.push(crate::provider::build_openrouter_endpoint_route(
                            model,
                            ep,
                            auth.openrouter != crate::auth::AuthState::NotConfigured,
                            Some(&age_str),
                        ));
                    }
                }
                continue;
            }

            let mut added_any = false;

            if crate::provider::provider_for_model(model) == Some("claude")
                && auth.anthropic.has_oauth
            {
                let (available, detail) =
                    crate::provider::anthropic_oauth_route_availability(model);
                routes.push(crate::provider::build_anthropic_oauth_route(
                    model, available, detail,
                ));
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
                routes.push(crate::provider::build_openai_oauth_route(
                    model, available, detail,
                ));
                added_any = true;
            }

            if auth.openrouter != crate::auth::AuthState::NotConfigured {
                match (
                    crate::provider::provider_for_model(model),
                    openrouter_cached.as_ref(),
                ) {
                    (_, Some((endpoints, _age))) => {
                        for ep in endpoints {
                            routes.push(crate::provider::build_openrouter_endpoint_route(
                                model, ep, true, None,
                            ));
                        }
                        added_any = true;
                    }
                    (Some("claude"), None) => {
                        routes.push(crate::provider::build_openrouter_fallback_provider_route(
                            model,
                            openrouter_catalog_model.as_deref().unwrap_or(model),
                            "Anthropic",
                        ));
                        added_any = true;
                    }
                    (Some("openai"), None) => {
                        routes.push(crate::provider::build_openrouter_fallback_provider_route(
                            model,
                            openrouter_catalog_model.as_deref().unwrap_or(model),
                            "OpenAI",
                        ));
                        added_any = true;
                    }
                    _ => {}
                }
            }

            if Self::remote_model_should_offer_copilot_route(model) && !model.contains("[1m]") {
                routes.push(crate::provider::build_copilot_route(
                    model,
                    auth.copilot == crate::auth::AuthState::Available
                        || Self::remote_model_is_server_copilot_only(model),
                    String::new(),
                ));
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

    pub(super) fn handle_inline_interactive_preview_key(
        &mut self,
        code: &KeyCode,
        modifiers: KeyModifiers,
    ) -> Result<bool> {
        let is_preview = self
            .inline_interactive_state
            .as_ref()
            .is_some_and(|p| p.preview);
        if !is_preview {
            return Ok(false);
        }
        match code {
            KeyCode::Down => {
                if let Some(picker) = self.inline_interactive_state.as_mut() {
                    let max = picker.filtered.len().saturating_sub(1);
                    picker.selected = (picker.selected + 1).min(max);
                }
                Ok(true)
            }
            KeyCode::Up => {
                if let Some(picker) = self.inline_interactive_state.as_mut() {
                    picker.selected = picker.selected.saturating_sub(1);
                }
                Ok(true)
            }
            KeyCode::Char('j') if modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(picker) = self.inline_interactive_state.as_mut() {
                    let max = picker.filtered.len().saturating_sub(1);
                    picker.selected = (picker.selected + 1).min(max);
                }
                Ok(true)
            }
            KeyCode::Char('k') if modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(picker) = self.inline_interactive_state.as_mut() {
                    picker.selected = picker.selected.saturating_sub(1);
                }
                Ok(true)
            }
            KeyCode::PageDown => {
                if let Some(picker) = self.inline_interactive_state.as_mut() {
                    let max = picker.filtered.len().saturating_sub(1);
                    picker.selected = (picker.selected + 5).min(max);
                }
                Ok(true)
            }
            KeyCode::PageUp => {
                if let Some(picker) = self.inline_interactive_state.as_mut() {
                    picker.selected = picker.selected.saturating_sub(5);
                }
                Ok(true)
            }
            KeyCode::Enter => {
                if let Some(ref mut picker) = self.inline_interactive_state {
                    if picker.filtered.is_empty() {
                        self.inline_interactive_state = None;
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
                    picker.column = picker.preview_activation_column();
                }
                self.input.clear();
                self.cursor_pos = 0;
                self.handle_inline_interactive_key(KeyCode::Enter, modifiers)?;
                Ok(true)
            }
            KeyCode::Esc => {
                self.inline_interactive_state = None;
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
        let picker = SessionPicker::loading();
        self.session_picker_overlay = Some(RefCell::new(picker));
        self.session_picker_mode = SessionPickerMode::Resume;
        self.set_status_notice("Loading sessions...");
        self.start_session_picker_load();
    }

    fn start_session_picker_load(&mut self) {
        let (tx, rx) = std::sync::mpsc::channel();
        self.pending_session_picker_load = Some(super::PendingSessionPickerLoad { receiver: rx });

        tokio::task::spawn_blocking(move || {
            let result = session_picker::load_sessions_grouped();
            let _ = tx.send(result);
        });
    }

    pub(super) fn poll_session_picker_load(&mut self) -> bool {
        let recv_result = {
            let Some(pending) = self.pending_session_picker_load.as_ref() else {
                return false;
            };
            pending.receiver.try_recv()
        };

        match recv_result {
            Ok(Ok((server_groups, orphan_sessions))) => {
                self.pending_session_picker_load = None;
                if self.session_picker_overlay.is_some()
                    && self.session_picker_mode == SessionPickerMode::Resume
                {
                    let picker = SessionPicker::new_grouped(server_groups, orphan_sessions);
                    self.session_picker_overlay = Some(RefCell::new(picker));
                    self.set_status_notice("Sessions loaded");
                    return true;
                }
                false
            }
            Ok(Err(e)) => {
                self.pending_session_picker_load = None;
                if self.session_picker_overlay.is_some()
                    && self.session_picker_mode == SessionPickerMode::Resume
                {
                    self.session_picker_overlay = None;
                    self.push_display_message(DisplayMessage::error(format!(
                        "Failed to load sessions: {}",
                        e
                    )));
                    self.set_status_notice("Session load failed");
                    return true;
                }
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.pending_session_picker_load = None;
                if self.session_picker_overlay.is_some()
                    && self.session_picker_mode == SessionPickerMode::Resume
                {
                    self.session_picker_overlay = None;
                    self.push_display_message(DisplayMessage::error(
                        "Session loading stopped before returning a result.".to_string(),
                    ));
                    self.set_status_notice("Session load failed");
                    return true;
                }
                false
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
                if let Some(session) = picker.session_for_target(target)
                    && let Some(dir) = session.working_dir.as_deref()
                    && std::path::Path::new(dir).is_dir()
                {
                    cwd = std::path::PathBuf::from(dir);
                }
            }

            let name = match target {
                ResumeTarget::JcodeSession { session_id } => {
                    crate::id::extract_session_name(session_id)
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| session_id.to_string())
                }
                ResumeTarget::ClaudeCodeSession { session_id, .. } => {
                    format!("Claude Code {}", &session_id[..session_id.len().min(8)])
                }
                ResumeTarget::CodexSession { session_id, .. } => {
                    format!("Codex {}", &session_id[..session_id.len().min(8)])
                }
                ResumeTarget::PiSession { session_path } => std::path::Path::new(session_path)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Pi session")
                    .to_string(),
                ResumeTarget::OpenCodeSession { session_id, .. } => {
                    format!("OpenCode {}", &session_id[..session_id.len().min(8)])
                }
            };
            let resolved_target = match crate::import::resolve_resume_target_to_jcode(target) {
                Ok(target) => target,
                Err(err) => {
                    failed.push(format!("failed to import {}: {}", name, err));
                    continue;
                }
            };

            match spawn_resume_target_in_new_terminal(&resolved_target, &cwd, socket.as_deref()) {
                Ok(true) => {
                    spawned += 1;
                    names.push(name);
                }
                Ok(false) | Err(_) => failed.push(resume_target_manual_command(
                    &resolved_target,
                    socket.as_deref(),
                )),
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

    pub(super) fn handle_session_picker_current_terminal_selection(
        &mut self,
        targets: &[ResumeTarget],
    ) {
        let Some(target) = targets.first() else {
            return;
        };

        let name = match target {
            ResumeTarget::JcodeSession { session_id } => {
                crate::id::extract_session_name(session_id)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| session_id.to_string())
            }
            ResumeTarget::ClaudeCodeSession { session_id, .. } => {
                format!("Claude Code {}", &session_id[..session_id.len().min(8)])
            }
            ResumeTarget::CodexSession { session_id, .. } => {
                format!("Codex {}", &session_id[..session_id.len().min(8)])
            }
            ResumeTarget::PiSession { session_path } => std::path::Path::new(session_path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Pi session")
                .to_string(),
            ResumeTarget::OpenCodeSession { session_id, .. } => {
                format!("OpenCode {}", &session_id[..session_id.len().min(8)])
            }
        };

        let resolved_target = match crate::import::resolve_resume_target_to_jcode(target) {
            Ok(target) => target,
            Err(err) => {
                self.push_display_message(DisplayMessage::error(format!(
                    "Failed to import {}: {}",
                    name, err
                )));
                return;
            }
        };

        let ResumeTarget::JcodeSession { session_id } = resolved_target else {
            self.push_display_message(DisplayMessage::error(format!(
                "Cannot resume {} in the current terminal.",
                name
            )));
            return;
        };

        if targets.len() > 1 {
            self.push_display_message(DisplayMessage::system(format!(
                "Selected {} sessions; resuming **{}** in this terminal.",
                targets.len(),
                name
            )));
        }
        crate::tui::workspace_client::queue_resume_session(session_id);
        self.session_picker_overlay = None;
        self.session_picker_mode = SessionPickerMode::Resume;
        self.set_status_notice(format!("Switching → {}", name));
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
            if let Ok(session) = crate::session::Session::load(session_id)
                && let Some(dir) = session.working_dir.as_deref()
                && std::path::Path::new(dir).is_dir()
            {
                session_cwd = std::path::PathBuf::from(dir);
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
            OverlayAction::Selected(PickerResult::Selected(ids))
            | OverlayAction::Selected(PickerResult::SelectedInNewTerminal(ids)) => {
                self.handle_session_picker_selection(&ids);
                if let Some(picker_cell) = self.session_picker_overlay.as_ref() {
                    picker_cell.borrow_mut().clear_selected_sessions();
                }
            }
            OverlayAction::Selected(PickerResult::SelectedInCurrentTerminal(ids)) => {
                self.handle_session_picker_current_terminal_selection(&ids);
            }
            OverlayAction::Selected(PickerResult::RestoreAllCrashed) => {
                self.handle_batch_crash_restore();
            }
        }
        Ok(())
    }

    pub(super) fn handle_inline_interactive_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> Result<()> {
        match code {
            KeyCode::Esc => {
                if let Some(ref mut picker) = self.inline_interactive_state
                    && !picker.filter.is_empty()
                {
                    picker.filter.clear();
                    Self::apply_inline_interactive_filter(picker);
                    return Ok(());
                }
                self.inline_interactive_state = None;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let vim_nav = self
                    .inline_interactive_state
                    .as_ref()
                    .map(|picker| picker.uses_compact_navigation())
                    .unwrap_or(false);
                if matches!(code, KeyCode::Char('k'))
                    && !modifiers.contains(KeyModifiers::CONTROL)
                    && !vim_nav
                {
                    if let Some(ref mut picker) = self.inline_interactive_state {
                        picker.filter.push('k');
                        Self::apply_inline_interactive_filter(picker);
                    }
                    return Ok(());
                }
                if let Some(ref mut picker) = self.inline_interactive_state {
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
                    .inline_interactive_state
                    .as_ref()
                    .map(|picker| picker.uses_compact_navigation())
                    .unwrap_or(false);
                if matches!(code, KeyCode::Char('j'))
                    && !modifiers.contains(KeyModifiers::CONTROL)
                    && !vim_nav
                {
                    if let Some(ref mut picker) = self.inline_interactive_state {
                        picker.filter.push('j');
                        Self::apply_inline_interactive_filter(picker);
                    }
                    return Ok(());
                }
                if let Some(ref mut picker) = self.inline_interactive_state {
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
                if let Some(ref mut picker) = self.inline_interactive_state {
                    if picker.uses_compact_navigation() {
                        return Ok(());
                    }
                    if picker.column < picker.max_navigable_column()
                        && let Some(&idx) = picker.filtered.get(picker.selected)
                        && (picker.entries[idx].options.len() > 1 || picker.column > 0)
                    {
                        picker.column += 1;
                    }
                }
            }
            KeyCode::Left | KeyCode::BackTab => {
                if let Some(ref mut picker) = self.inline_interactive_state {
                    if picker.uses_compact_navigation() {
                        return Ok(());
                    }
                    if picker.column > 0 {
                        picker.column -= 1;
                    }
                }
            }
            KeyCode::Tab => {
                if let Some(ref mut picker) = self.inline_interactive_state {
                    if picker.uses_compact_navigation() {
                        return Ok(());
                    }
                    if picker.column == 0 && !picker.filter.is_empty() {
                        Self::tab_complete_inline_interactive_filter(picker);
                    } else if picker.column < picker.max_navigable_column()
                        && let Some(&idx) = picker.filtered.get(picker.selected)
                        && (picker.entries[idx].options.len() > 1 || picker.column > 0)
                    {
                        picker.column += 1;
                    }
                }
            }
            KeyCode::Char('d') if modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(ref picker) = self.inline_interactive_state {
                    if picker.uses_compact_navigation() {
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
                        } else if r.provider == "Antigravity" {
                            format!("antigravity:{}", bare_name)
                        } else if openai_compatible_profile_id_for_route(r).is_some() {
                            bare_name.clone()
                        } else if r.api_method == "openrouter" && r.provider != "auto" {
                            if bare_name.contains('/') {
                                format!("{}@{}", bare_name, r.provider)
                            } else {
                                format!("anthropic/{}@{}", bare_name, r.provider)
                            }
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
                            "openai-oauth" | "openai-api-key" => Some("openai"),
                            "copilot" => Some("copilot"),
                            "cursor" => Some("cursor"),
                            "cli" if r.provider == "Antigravity" => Some("antigravity"),
                            "openrouter" => Some("openrouter"),
                            method if method.starts_with("openai-compatible") => {
                                openai_compatible_profile_id_for_route(r)
                            }
                            _ => openai_compatible_profile_id_for_route(r),
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
                        Ok(()) => {
                            self.invalidate_model_picker_cache();
                            self.set_status_notice(notice)
                        }
                        Err(e) => self.set_status_notice(format!("Failed to save default: {}", e)),
                    }
                    self.inline_interactive_state = None;
                }
            }
            KeyCode::Enter => {
                let Some(ref mut picker) = self.inline_interactive_state else {
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
                        picker.column = picker.max_navigable_column();
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
                    self.inline_interactive_state = None;
                    self.set_status_notice(format!("{} — {}", entry.name, detail));
                    return Ok(());
                }

                match entry.action {
                    PickerAction::Account(selection) => {
                        self.inline_interactive_state = None;
                        self.handle_account_picker_selection(selection);
                    }
                    PickerAction::Login(provider) => {
                        self.inline_interactive_state = None;
                        self.start_login_provider(provider);
                    }
                    PickerAction::Usage {
                        title,
                        subtitle,
                        status,
                        detail_lines,
                        ..
                    } => {
                        self.inline_interactive_state = None;
                        let mut content = vec![format!("# {}", title), subtitle];
                        content.push(format!("status: {}", status.label_for_display()));
                        content.extend(detail_lines);
                        self.push_display_message(DisplayMessage::usage(content.join("\n")));
                        self.set_status_notice(format!("Usage → {}", title));
                    }
                    PickerAction::AgentTarget(target) => {
                        self.open_agent_model_picker(target);
                    }
                    PickerAction::AgentModelChoice {
                        target,
                        clear_override,
                    } => {
                        self.inline_interactive_state = None;
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
                        if !route.available {
                            self.push_display_message(DisplayMessage::error(
                                crate::tui::app::model_context::unavailable_model_route_message(
                                    &entry.name,
                                    &route.provider,
                                    &route.detail,
                                    self.is_remote,
                                ),
                            ));
                            self.set_status_notice("Model unavailable");
                            return Ok(());
                        }

                        let bare_name = model_entry_base_name(&entry);
                        let spec = if route.api_method == "openrouter" && route.provider == "auto" {
                            openrouter_route_model_id(&bare_name)
                        } else {
                            picker_route_model_spec(&entry, route)
                        };

                        let effort = entry.effort.clone();
                        let notice = format!(
                            "Model → {} via {} ({})",
                            entry.name, route.provider, route.api_method
                        );

                        if self.is_remote {
                            self.inline_interactive_state = None;
                            self.upstream_provider = None;
                            self.status_detail = None;
                            self.pending_model_switch = Some(spec);
                        } else {
                            match self.provider.set_model(&spec) {
                                Ok(()) => {
                                    self.inline_interactive_state = None;
                                    self.upstream_provider = None;
                                    self.status_detail = None;
                                    self.invalidate_model_picker_cache();
                                }
                                Err(error) => {
                                    self.push_display_message(DisplayMessage::error(
                                        crate::tui::app::model_context::model_switch_failure_message(
                                            &error.to_string(),
                                            self.is_remote,
                                        ),
                                    ));
                                    self.set_status_notice("Model switch failed");
                                    return Ok(());
                                }
                            }
                        }
                        if let Some(effort) = effort {
                            let _ = self.provider.set_reasoning_effort(&effort);
                        }
                        self.set_status_notice(notice);
                    }
                }
            }
            KeyCode::Backspace => {
                if let Some(ref mut picker) = self.inline_interactive_state
                    && picker.filter.pop().is_some()
                {
                    Self::apply_inline_interactive_filter(picker);
                }
            }
            KeyCode::Char(c) => {
                if let Some(ref mut picker) = self.inline_interactive_state
                    && !c.is_whitespace()
                {
                    picker.filter.push(c);
                    Self::apply_inline_interactive_filter(picker);
                }
            }
            _ => {}
        }
        Ok(())
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
                if let Some(last) = last_match
                    && last + 1 == ti
                {
                    score += 3;
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

    pub(super) fn apply_inline_interactive_filter(picker: &mut InlineInteractiveState) {
        if picker.filter.is_empty() {
            picker.filtered = (0..picker.entries.len()).collect();
        } else {
            let mut scored: Vec<(usize, i32)> = picker
                .entries
                .iter()
                .enumerate()
                .filter_map(|(i, m)| {
                    let filter_text = picker.filter_text(m);
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

    pub(super) fn tab_complete_inline_interactive_filter(picker: &mut InlineInteractiveState) {
        if picker.filtered.is_empty() {
            return;
        }
        if picker.filtered.len() == 1 {
            let name = picker.entries[picker.filtered[0]].name.clone();
            picker.filter = name;
            Self::apply_inline_interactive_filter(picker);
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
            Self::apply_inline_interactive_filter(picker);
        }
    }
}
