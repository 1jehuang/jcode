use super::*;
use crate::tui::TuiState as _;
use std::cell::RefCell;
use std::sync::Mutex;
use std::time::Duration;

const REMOTE_STARTUP_HEADER_DEBOUNCE: Duration = Duration::from_millis(400);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WidgetProviderKind {
    Anthropic,
    OpenAI,
    OpenCode,
    OpenRouter,
    CostBasedApiKey,
    Copilot,
    Gemini,
    Unknown,
}

impl WidgetProviderKind {
    fn from_provider_key(raw: Option<&str>) -> Self {
        match raw.map(|provider| provider.trim().to_ascii_lowercase()) {
            Some(provider) if provider == "openrouter" => Self::OpenRouter,
            Some(provider) if matches!(provider.as_str(), "opencode" | "opencode-go") => {
                Self::OpenCode
            }
            Some(provider)
                if matches!(
                    provider.as_str(),
                    "bedrock" | "aws-bedrock" | "azure-openai"
                ) || crate::provider_catalog::openai_compatible_profile_by_id(&provider)
                    .is_some_and(|profile| profile.requires_api_key) =>
            {
                Self::CostBasedApiKey
            }
            Some(provider) if provider == "copilot" => Self::Copilot,
            Some(provider) if provider == "gemini" => Self::Gemini,
            Some(provider) if provider == "openai" => Self::OpenAI,
            Some(provider) if matches!(provider.as_str(), "anthropic" | "claude") => {
                Self::Anthropic
            }
            _ => Self::Unknown,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WidgetRouteInfo {
    provider: WidgetProviderKind,
    is_remote: bool,
}

impl App {
    fn sanitize_remote_model_hint(model: Option<String>) -> Option<String> {
        model
            .map(|model| model.trim().to_string())
            .filter(|model| !model.is_empty() && !model.eq_ignore_ascii_case("unknown"))
    }

    fn configured_remote_provider_hint(&self) -> Option<String> {
        std::env::var("JCODE_PROVIDER")
            .ok()
            .or_else(|| crate::config::config().provider.default_provider.clone())
            .map(|provider| provider.trim().to_string())
            .filter(|provider| !provider.is_empty())
    }

    fn configured_remote_model_hint(&self) -> Option<String> {
        Self::sanitize_remote_model_hint(
            std::env::var("JCODE_MODEL")
                .ok()
                .or_else(|| crate::config::config().provider.default_model.clone()),
        )
    }

    pub(super) fn effective_remote_provider_model(&self) -> Option<String> {
        Self::sanitize_remote_model_hint(self.remote_provider_model.clone())
            .or_else(|| Self::sanitize_remote_model_hint(self.session.model.clone()))
            .or_else(|| self.configured_remote_model_hint())
    }

    fn remote_header_provider_model(&self) -> Option<String> {
        let effective_model = self.effective_remote_provider_model();

        self.remote_startup_phase
            .as_ref()
            .and_then(|phase| {
                if matches!(phase, super::RemoteStartupPhase::Connecting)
                    && effective_model.is_some()
                {
                    return effective_model.clone();
                }

                let elapsed = self
                    .remote_startup_phase_started
                    .map(|started| started.elapsed())
                    .unwrap_or_default();
                let should_defer_header = matches!(phase, super::RemoteStartupPhase::Connecting)
                    && elapsed < REMOTE_STARTUP_HEADER_DEBOUNCE;

                if should_defer_header {
                    None
                } else {
                    Some(phase.header_label_with_elapsed(elapsed))
                }
            })
            .or(effective_model)
            .or_else(|| {
                (self.remote_session_id.is_some() || self.connection_type.is_some())
                    .then(|| "connected".to_string())
            })
    }

    fn remote_header_provider_name(&self) -> Option<String> {
        let configured_provider_hint = self.configured_remote_provider_hint();
        self.remote_provider_name
            .clone()
            .or_else(|| {
                self.effective_remote_provider_model().and_then(|model| {
                    crate::provider::provider_for_model_with_hint(&model, None)
                        .or(configured_provider_hint.as_deref())
                        .map(str::to_string)
                })
            })
            .filter(|provider| !provider.trim().is_empty())
    }

    fn widget_route_info(&self, model: Option<&str>) -> WidgetRouteInfo {
        let uses_remote_widget_metadata = self.is_remote || self.is_replay_runtime();
        let remote_provider_name = if uses_remote_widget_metadata {
            self.remote_header_provider_name()
        } else {
            None
        };
        let provider_name = if uses_remote_widget_metadata {
            remote_provider_name.as_deref()
        } else {
            Some(self.provider.name())
        };

        let provider_from_hint = WidgetProviderKind::from_provider_key(provider_name);
        let provider = if provider_from_hint != WidgetProviderKind::Unknown {
            provider_from_hint
        } else {
            WidgetProviderKind::from_provider_key(
                model
                    .map(|model| crate::provider::resolve_model_capabilities(model, provider_name))
                    .and_then(|caps| caps.provider)
                    .as_deref(),
            )
        };

        WidgetRouteInfo {
            provider,
            is_remote: uses_remote_widget_metadata,
        }
    }

    /// Resolve the active credential (OAuth vs API key) for a dual-auth
    /// provider (Anthropic / OpenAI). This is the one place billing identity is
    /// decided for the info widget, regardless of transport:
    ///
    /// * Remote sessions use [`App::remote_resolved_credential`], which the
    ///   server resolved authoritatively from its live credentials.
    /// * Local sessions prefer the provider's *explicitly pinned* credential
    ///   ([`Provider::active_explicit_credential`]) so the widget reflects the
    ///   credential the next request will actually use the instant the user
    ///   switches OAuth<->API (model picker, `/account`, header toggle). That
    ///   read is in-memory and cache-free, so it never lingers on a stale
    ///   [`AuthStatus`] snapshot (cached up to 60s) or a `JCODE_RUNTIME_PROVIDER`
    ///   pin that drifted out of sync with the provider. When the provider is in
    ///   auto mode (no explicit pin) it falls back to
    ///   [`resolve_dual_credential_auth`] -- shared with the header tag and
    ///   model-switch line -- which is cheap (cached probe, no per-frame I/O).
    ///
    /// Returns `None` when neither transport can determine the credential (e.g.
    /// the server didn't report one, or no credentials are configured locally).
    fn dual_credential_active(
        &self,
        route: WidgetRouteInfo,
        provider: jcode_provider_core::ActiveProvider,
    ) -> Option<crate::auth::ActiveCredential> {
        if route.is_remote {
            return self.remote_resolved_credential.map(Into::into);
        }

        // Authoritative, cache-free answer from the live provider whenever the
        // user has explicitly pinned a credential. This reflects exactly what the
        // next request will use, so an explicit OAuth<->API switch is visible on
        // the very next frame. For local sessions the requested `provider` always
        // matches the live active provider (the widget route is derived from
        // `self.provider.name()`), and remote sessions returned above, so the
        // pin maps onto the right dual-auth provider. Explicit reads do no disk
        // I/O, so the common per-frame path stays cheap; auto mode returns `None`
        // here and falls through to the cached heuristic below.
        if let Some(resolved) = self.provider.active_explicit_credential() {
            return Some(resolved.into());
        }

        let auth_status = crate::auth::AuthStatus::check_fast();
        let runtime_provider = active_runtime_provider_key();
        crate::auth::resolve_dual_credential_auth(
            provider,
            &auth_status,
            runtime_provider.as_deref(),
        )
        .map(|resolved| resolved.active)
    }

    fn widget_auth_method(&self, route: WidgetRouteInfo) -> crate::tui::info_widget::AuthMethod {
        use crate::auth::ActiveCredential;
        use crate::tui::info_widget::AuthMethod;

        match route.provider {
            WidgetProviderKind::Anthropic => {
                match self
                    .dual_credential_active(route, jcode_provider_core::ActiveProvider::Claude)
                {
                    Some(ActiveCredential::OAuth) => AuthMethod::AnthropicOAuth,
                    Some(ActiveCredential::ApiKey) => AuthMethod::AnthropicApiKey,
                    None => AuthMethod::Unknown,
                }
            }
            WidgetProviderKind::OpenAI => {
                match self
                    .dual_credential_active(route, jcode_provider_core::ActiveProvider::OpenAI)
                {
                    Some(ActiveCredential::OAuth) => AuthMethod::OpenAIOAuth,
                    Some(ActiveCredential::ApiKey) => AuthMethod::OpenAIApiKey,
                    None => AuthMethod::Unknown,
                }
            }
            // Providers below have no OAuth-vs-API-key ambiguity to resolve from
            // remote credentials; remote sessions render usage via
            // `widget_usage_info`'s `is_remote` handling, so report Unknown here
            // and let the local heuristics run only for local sessions.
            _ if route.is_remote => AuthMethod::Unknown,
            WidgetProviderKind::OpenCode => crate::tui::info_widget::AuthMethod::OpenCodeApiKey,
            WidgetProviderKind::OpenRouter => {
                let runtime_provider = active_runtime_provider_key();
                let transport_state =
                    crate::provider::openrouter::OpenRouterTransportState::from_current_env(
                        runtime_provider.as_deref(),
                    );
                if transport_state.is_real_openrouter() {
                    crate::tui::info_widget::AuthMethod::OpenRouterApiKey
                } else if transport_state.accrues_user_api_key_cost() {
                    crate::tui::info_widget::AuthMethod::ApiKey
                } else {
                    crate::tui::info_widget::AuthMethod::Unknown
                }
            }
            WidgetProviderKind::CostBasedApiKey => crate::tui::info_widget::AuthMethod::ApiKey,
            WidgetProviderKind::Copilot => crate::tui::info_widget::AuthMethod::CopilotOAuth,
            WidgetProviderKind::Gemini => {
                let auth_status = crate::auth::AuthStatus::check_fast();
                if auth_status.gemini == crate::auth::AuthState::Available {
                    crate::tui::info_widget::AuthMethod::GeminiOAuth
                } else {
                    crate::tui::info_widget::AuthMethod::Unknown
                }
            }
            WidgetProviderKind::Unknown => crate::tui::info_widget::AuthMethod::Unknown,
        }
    }

    fn widget_usage_info(
        &self,
        route: WidgetRouteInfo,
        auth_method: crate::tui::info_widget::AuthMethod,
    ) -> Option<crate::tui::info_widget::UsageInfo> {
        let output_tps = if matches!(self.status, ProcessingStatus::Streaming) {
            self.compute_streaming_tps()
        } else {
            None
        };

        // On a resumed session, `token_accounting.total_*` is reset to 0 and the
        // prior usage lives in `remote_total_tokens` (restored from history). Add
        // them so the widget's "in + out" reflects the whole session, mirroring
        // the `/cache` stats path, rather than only tokens seen since resume.
        let (display_input_tokens, display_output_tokens) =
            if let Some((hist_in, hist_out)) = self.remote_total_tokens {
                (
                    hist_in.saturating_add(self.token_accounting.total_input_tokens),
                    hist_out.saturating_add(self.token_accounting.total_output_tokens),
                )
            } else {
                (
                    self.token_accounting.total_input_tokens,
                    self.token_accounting.total_output_tokens,
                )
            };

        let cost_based_usage = || crate::tui::info_widget::UsageInfo {
            provider: crate::tui::info_widget::UsageProvider::CostBased,
            five_hour: 0.0,
            five_hour_resets_at: None,
            seven_day: 0.0,
            seven_day_resets_at: None,
            spark: None,
            spark_resets_at: None,
            total_cost: self.cost.total_cost,
            input_tokens: display_input_tokens,
            output_tokens: display_output_tokens,
            cache_read_tokens: self.streaming.streaming_cache_read_tokens,
            cache_write_tokens: self.streaming.streaming_cache_creation_tokens,
            output_tps,
            available: true,
        };

        match route.provider {
            WidgetProviderKind::Copilot => Some(crate::tui::info_widget::UsageInfo {
                provider: crate::tui::info_widget::UsageProvider::Copilot,
                five_hour: 0.0,
                five_hour_resets_at: None,
                seven_day: 0.0,
                seven_day_resets_at: None,
                spark: None,
                spark_resets_at: None,
                total_cost: 0.0,
                input_tokens: display_input_tokens,
                output_tokens: display_output_tokens,
                cache_read_tokens: None,
                cache_write_tokens: None,
                output_tps,
                available: display_input_tokens > 0 || display_output_tokens > 0,
            }),
            WidgetProviderKind::Anthropic => {
                if matches!(
                    auth_method,
                    crate::tui::info_widget::AuthMethod::AnthropicApiKey
                ) {
                    return Some(cost_based_usage());
                }

                let usage = crate::usage::get_sync();
                Some(crate::tui::info_widget::UsageInfo {
                    provider: crate::tui::info_widget::UsageProvider::Anthropic,
                    five_hour: usage.five_hour,
                    five_hour_resets_at: usage.five_hour_resets_at.clone(),
                    seven_day: usage.seven_day,
                    seven_day_resets_at: usage.seven_day_resets_at.clone(),
                    spark: None,
                    spark_resets_at: None,
                    total_cost: 0.0,
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                    output_tps,
                    available: usage.last_error.is_none(),
                })
            }
            WidgetProviderKind::OpenAI => {
                if matches!(
                    auth_method,
                    crate::tui::info_widget::AuthMethod::OpenAIApiKey
                ) {
                    return Some(cost_based_usage());
                }

                let openai_usage = crate::usage::get_openai_usage_sync();
                Some(crate::tui::info_widget::UsageInfo {
                    provider: crate::tui::info_widget::UsageProvider::OpenAI,
                    five_hour: openai_usage
                        .five_hour
                        .as_ref()
                        .map(|w| w.usage_ratio)
                        .unwrap_or(0.0),
                    five_hour_resets_at: openai_usage
                        .five_hour
                        .as_ref()
                        .and_then(|w| w.resets_at.clone()),
                    seven_day: openai_usage
                        .seven_day
                        .as_ref()
                        .map(|w| w.usage_ratio)
                        .unwrap_or(0.0),
                    seven_day_resets_at: openai_usage
                        .seven_day
                        .as_ref()
                        .and_then(|w| w.resets_at.clone()),
                    spark: openai_usage.spark.as_ref().map(|w| w.usage_ratio),
                    spark_resets_at: openai_usage
                        .spark
                        .as_ref()
                        .and_then(|w| w.resets_at.clone()),
                    total_cost: 0.0,
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                    output_tps,
                    available: openai_usage.has_limits(),
                })
            }
            WidgetProviderKind::Gemini => None,
            WidgetProviderKind::OpenRouter => {
                if route.is_remote {
                    return Some(cost_based_usage());
                }

                let runtime_provider = active_runtime_provider_key();
                let transport_state =
                    crate::provider::openrouter::OpenRouterTransportState::from_current_env(
                        runtime_provider.as_deref(),
                    );
                if transport_state.accrues_user_api_key_cost() {
                    Some(cost_based_usage())
                } else {
                    None
                }
            }
            WidgetProviderKind::OpenCode | WidgetProviderKind::CostBasedApiKey => {
                Some(cost_based_usage())
            }
            WidgetProviderKind::Unknown => None,
        }
    }
}

impl crate::tui::TuiState for App {
    fn display_messages(&self) -> &[DisplayMessage] {
        &self.display_messages
    }

    fn display_user_message_count(&self) -> usize {
        self.display_user_message_count
    }

    fn compacted_hidden_user_prompts(&self) -> usize {
        self.compacted_history_lazy.hidden_user_prompts
    }

    fn has_display_edit_tool_messages(&self) -> bool {
        self.display_edit_tool_message_count > 0
    }

    fn side_pane_images(&self) -> Vec<crate::session::RenderedImage> {
        if self.is_remote {
            self.remote_side_pane_images.clone()
        } else {
            crate::session::render_images(&self.session)
        }
    }

    fn side_pane_images_signature(&self) -> (usize, u64) {
        // Recomputing the signature walks (and in local mode re-renders) every
        // image payload, so cache it per display_messages_version: image sets
        // only change when the transcript does.
        let version = self.display_messages_version;
        if let Some((cached_version, signature)) = self.side_pane_images_signature_cache.get()
            && cached_version == version
        {
            return signature;
        }
        use std::hash::{Hash, Hasher};
        let images = self.side_pane_images();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for image in &images {
            image.media_type.hash(&mut hasher);
            image.data.len().hash(&mut hasher);
            image
                .data
                .as_bytes()
                .iter()
                .take(64)
                .for_each(|b| b.hash(&mut hasher));
            crate::tui::hash_rendered_image_anchor(image.anchor.as_ref(), &mut hasher);
        }
        let signature = (images.len(), hasher.finish());
        self.side_pane_images_signature_cache
            .set(Some((version, signature)));
        signature
    }

    fn display_messages_version(&self) -> u64 {
        self.display_messages_version
    }

    fn streaming_text(&self) -> &str {
        &self.streaming.streaming_text
    }

    fn input(&self) -> &str {
        &self.input
    }

    fn cursor_pos(&self) -> usize {
        self.cursor_pos
    }

    fn is_processing(&self) -> bool {
        self.is_processing || self.pending_queued_dispatch || self.split_launch_in_flight()
    }

    fn queued_messages(&self) -> &[String] {
        &self.queued_messages
    }

    fn interleave_message(&self) -> Option<&str> {
        self.interleave_message.as_deref()
    }

    fn pending_soft_interrupts(&self) -> &[String] {
        &self.pending_soft_interrupts
    }

    fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    fn auto_scroll_paused(&self) -> bool {
        self.auto_scroll_paused
    }

    fn pending_history_anchor_lines_from_bottom(&self) -> Option<usize> {
        self.pending_history_anchor
            .map(|anchor| anchor.lines_from_bottom)
    }

    fn chat_overscroll_active(&self) -> bool {
        self.chat_overscroll_active()
    }

    fn chat_overscroll_remaining(&self) -> Option<f32> {
        self.chat_overscroll_remaining()
    }

    fn copy_selection_edge_autoscroll_active(&self) -> bool {
        self.copy_selection_edge_autoscroll.is_some() && self.copy_selection_dragging
    }

    fn provider_name(&self) -> String {
        if self.is_remote {
            self.remote_header_provider_name().unwrap_or_default()
        } else {
            self.remote_provider_name
                .clone()
                .unwrap_or_else(|| self.provider.display_name())
        }
    }

    fn provider_model(&self) -> String {
        if self.is_remote {
            self.remote_header_provider_model()
                .unwrap_or_else(|| "connecting to server…".to_string())
        } else {
            self.remote_provider_model
                .clone()
                .unwrap_or_else(|| self.provider.model().to_string())
        }
    }

    fn upstream_provider(&self) -> Option<String> {
        self.upstream_provider.clone()
    }

    fn connection_type(&self) -> Option<String> {
        self.connection_type.clone()
    }

    fn status_detail(&self) -> Option<String> {
        self.status_detail.clone()
    }

    fn mcp_servers(&self) -> Vec<(String, usize)> {
        self.mcp_server_names.clone()
    }

    fn available_skills(&self) -> Vec<String> {
        if self.is_remote && !self.remote_skills.is_empty() {
            self.remote_skills.clone()
        } else {
            self.current_skills_snapshot()
                .list()
                .iter()
                .map(|s| s.name.clone())
                .collect()
        }
    }

    fn streaming_tokens(&self) -> (u64, u64) {
        (
            self.streaming.streaming_input_tokens,
            self.streaming.streaming_output_tokens,
        )
    }

    fn streaming_cache_tokens(&self) -> (Option<u64>, Option<u64>) {
        (
            self.streaming.streaming_cache_read_tokens,
            self.streaming.streaming_cache_creation_tokens,
        )
    }

    fn output_tps(&self) -> Option<f32> {
        if !self.is_processing || !matches!(self.status, ProcessingStatus::Streaming) {
            return None;
        }
        self.compute_streaming_tps()
    }

    fn streaming_tool_calls(&self) -> Vec<ToolCall> {
        self.streaming_tool_calls.clone()
    }

    fn update_cost(&mut self) {
        self.update_cost_impl()
    }

    fn elapsed(&self) -> Option<std::time::Duration> {
        if let Some(d) = self.replay_elapsed_override {
            return Some(d);
        }
        if self.is_processing() {
            return self
                .visible_turn_started
                .or(self.processing_started)
                .map(|t| t.elapsed());
        }
        self.split_launch_in_flight()
            .then(|| self.pending_split_started_at.map(|t| t.elapsed()))
            .flatten()
    }

    fn status(&self) -> ProcessingStatus {
        if self.pending_queued_dispatch || self.split_launch_in_flight() {
            ProcessingStatus::Sending
        } else {
            self.status.clone()
        }
    }

    fn connection_phase_elapsed(&self) -> Option<std::time::Duration> {
        // Fall back to the whole-turn elapsed only if we somehow entered a
        // connecting status without recording a phase start.
        self.connection_phase_started
            .map(|t| t.elapsed())
            .or_else(|| self.elapsed())
    }

    fn command_suggestions(&self) -> Vec<(String, &'static str)> {
        App::command_suggestions(self)
    }

    fn command_suggestion_selected(&self) -> usize {
        self.command_suggestion_selected
    }

    fn active_skill(&self) -> Option<String> {
        self.active_skill.clone()
    }

    fn subagent_status(&self) -> Option<String> {
        self.subagent_status.clone()
    }

    fn batch_progress(&self) -> Option<crate::bus::BatchProgress> {
        self.batch_progress.clone()
    }

    fn time_since_activity(&self) -> Option<std::time::Duration> {
        if let Some(last_activity) = self.last_stream_activity {
            return Some(last_activity.elapsed());
        }

        // Restored/resumed clients often have a full transcript but no stream event in this
        // process yet. Treat those as already idle so reopening many historical sessions does not
        // spend the first warm-up window rerendering large static transcripts at idle FPS.
        if !self.display_messages.is_empty() && !self.is_processing {
            return Some(crate::tui::REDRAW_DEEP_IDLE_AFTER + std::time::Duration::from_secs(1));
        }

        Some(self.app_started.elapsed())
    }

    fn client_focused(&self) -> bool {
        App::client_focused(self)
    }

    fn stream_message_ended(&self) -> bool {
        self.stream_message_ended
    }

    fn has_pending_mouse_scroll_animation(&self) -> bool {
        self.mouse_scroll_queue != 0
    }

    fn total_session_tokens(&self) -> Option<(u64, u64)> {
        // In remote mode, use tokens from server
        // Independent mode doesn't currently track total tokens
        self.remote_total_tokens
    }

    fn session_compaction_count(&self) -> usize {
        if self.is_remote || !self.provider.uses_jcode_compaction() {
            return 0;
        }
        self.registry
            .compaction()
            .try_read()
            .ok()
            .map(|manager| manager.compacted_count())
            .unwrap_or(0)
    }

    fn is_remote_mode(&self) -> bool {
        self.is_remote
    }

    fn is_canary(&self) -> bool {
        if self.is_remote {
            self.remote_is_canary.unwrap_or(self.session.is_canary)
        } else {
            self.session.is_canary
        }
    }

    fn is_replay(&self) -> bool {
        self.is_replay
    }

    fn diff_mode(&self) -> crate::config::DiffDisplayMode {
        self.diff_mode
    }

    fn current_session_id(&self) -> Option<String> {
        if self.is_remote {
            self.remote_session_id.clone()
        } else {
            Some(self.session.id.clone())
        }
    }

    fn session_display_name(&self) -> Option<String> {
        if self.is_remote {
            self.remote_session_id
                .as_ref()
                .or(self.resume_session_id.as_ref())
                .as_ref()
                .and_then(|id| crate::id::extract_session_name(id))
                .map(|s| s.to_string())
        } else {
            Some(self.session.display_name().to_string())
        }
    }

    fn server_display_name(&self) -> Option<String> {
        self.remote_server_short_name.clone().or_else(|| {
            if !self.is_remote {
                return None;
            }
            crate::registry::find_server_by_socket_sync(&crate::server::socket_path())
                .map(|info| info.name)
        })
    }

    fn server_display_icon(&self) -> Option<String> {
        self.remote_server_icon.clone().or_else(|| {
            if !self.is_remote {
                return None;
            }
            crate::registry::find_server_by_socket_sync(&crate::server::socket_path())
                .map(|info| info.icon)
        })
    }

    fn server_display_version(&self) -> Option<String> {
        if !self.is_remote {
            return None;
        }
        // Prefer the live version reported by the connected server (history
        // sync); fall back to the registry record so a version is available
        // even before the first history event arrives.
        self.remote_server_version.clone().or_else(|| {
            crate::registry::find_server_by_socket_sync(&crate::server::socket_path())
                .map(|info| info.version)
                .filter(|version| !version.trim().is_empty())
        })
    }

    fn server_sessions(&self) -> Vec<String> {
        self.remote_sessions.clone()
    }

    fn connected_clients(&self) -> Option<usize> {
        self.remote_client_count
    }

    fn status_notice(&self) -> Option<String> {
        if !self.is_remote
            && self.provider.uses_jcode_compaction()
            && let Ok(manager) = self.registry.compaction().try_read()
            && manager.is_compacting()
        {
            return Some(Self::format_compaction_progress_notice(
                self.app_started.elapsed(),
            ));
        }
        self.status_notice.as_ref().and_then(|(text, at)| {
            if at.elapsed() <= Duration::from_secs(3) {
                Some(text.clone())
            } else {
                None
            }
        })
    }

    fn active_experimental_feature_notice(&self) -> Option<String> {
        self.active_experimental_feature_notice.clone()
    }

    fn remote_startup_phase_active(&self) -> bool {
        self.remote_startup_phase.is_some()
    }

    fn dictation_key_label(&self) -> Option<String> {
        self.dictation_key_label().map(|s| s.to_string())
    }

    fn animation_elapsed(&self) -> f32 {
        self.app_started.elapsed().as_secs_f32()
    }

    fn rate_limit_remaining(&self) -> Option<Duration> {
        self.rate_limit_reset.and_then(|reset_time| {
            let now = Instant::now();
            if reset_time > now {
                Some(reset_time - now)
            } else {
                None
            }
        })
    }

    fn queue_mode(&self) -> bool {
        self.queue_mode
    }

    fn next_prompt_new_session_armed(&self) -> bool {
        self.route_next_prompt_to_new_session
    }

    fn has_stashed_input(&self) -> bool {
        self.stashed_input.is_some()
    }

    fn context_snapshot(&self) -> crate::tui::ContextSnapshot {
        use crate::message::{ContentBlock, Role};
        use std::time::Instant;

        static CACHE: Mutex<Option<(Instant, CachedContextSnapshot)>> = Mutex::new(None);
        const TTL: Duration = Duration::from_millis(250);

        let session_key = if self.is_remote {
            self.remote_session_id
                .clone()
                .unwrap_or_else(|| self.session.id.clone())
        } else {
            self.session.id.clone()
        };
        let message_count = if self.is_remote {
            self.display_messages.len()
        } else {
            self.session.messages.len()
        };
        let (compaction_count, compaction_summary_chars, is_compacting, compaction_fresh) =
            if self.is_remote {
                (0, 0, false, true)
            } else if self.provider.uses_jcode_compaction() {
                match self.registry.compaction().try_read() {
                    Ok(manager) => (
                        manager.compacted_count(),
                        manager.summary_chars(),
                        manager.is_compacting(),
                        true,
                    ),
                    Err(_) => (0, 0, false, false),
                }
            } else {
                (0, 0, false, true)
            };

        if !compaction_fresh {
            return crate::tui::ContextSnapshot {
                info: None,
                revision: self.context_revision,
                fresh: false,
            };
        }

        if let Ok(cache) = CACHE.lock()
            && let Some((ts, cached)) = &*cache
            && ts.elapsed() < TTL
            && cached.session_key == session_key
            && cached.is_remote == self.is_remote
            && cached.display_messages_version == self.display_messages_version
            && cached.context_revision == self.context_revision
            && cached.message_count == message_count
            && cached.compaction_count == compaction_count
            && cached.compaction_summary_chars == compaction_summary_chars
            && cached.is_compacting == is_compacting
        {
            return cached.snapshot.clone();
        }

        let mut info = self.context_info.clone();
        info.session_context_chars = 0;

        // Compute dynamic stats from conversation
        let mut user_chars = 0usize;
        let mut user_count = 0usize;
        let mut asst_chars = 0usize;
        let mut asst_count = 0usize;
        let mut tool_call_chars = 0usize;
        let mut tool_call_count = 0usize;
        let mut tool_result_chars = 0usize;
        let mut tool_result_count = 0usize;

        if self.is_remote {
            for msg in &self.display_messages {
                match msg.role.as_str() {
                    "user" => {
                        user_count += 1;
                        user_chars += msg.content.len();
                    }
                    "assistant" => {
                        asst_count += 1;
                        asst_chars += msg.content.len();
                    }
                    "tool" => {
                        tool_result_count += 1;
                        tool_result_chars += msg.content.len();
                        if let Some(tool) = &msg.tool_data {
                            tool_call_count += 1;
                            tool_call_chars += tool.name.len() + tool.input.to_string().len();
                        }
                    }
                    _ => {}
                }
            }
        } else {
            let skip = if self.provider.uses_jcode_compaction() {
                let compaction = self.registry.compaction();
                let result = compaction
                    .try_read()
                    .ok()
                    .map(|manager| (manager.compacted_count(), manager.summary_chars()));
                if let Some((cc, sc)) = result {
                    if cc > 0 && sc > 0 {
                        user_count += 1;
                        user_chars += sc;
                    }
                    cc
                } else {
                    0
                }
            } else {
                0
            };

            for msg in self.session.messages.iter().skip(skip) {
                match msg.role {
                    Role::User => user_count += 1,
                    Role::Assistant => asst_count += 1,
                }

                for block in &msg.content {
                    match block {
                        ContentBlock::Text { text, .. } => {
                            if msg.role == Role::User
                                && text.starts_with("<system-reminder>\n# Session Context")
                            {
                                info.session_context_chars += text.len();
                                user_count = user_count.saturating_sub(1);
                            } else {
                                match msg.role {
                                    Role::User => user_chars += text.len(),
                                    Role::Assistant => asst_chars += text.len(),
                                }
                            }
                        }
                        ContentBlock::ToolUse { name, input, .. } => {
                            tool_call_count += 1;
                            tool_call_chars += name.len() + input.to_string().len();
                        }
                        ContentBlock::ToolResult { content, .. } => {
                            tool_result_count += 1;
                            tool_result_chars += content.len();
                        }
                        ContentBlock::Reasoning { text }
                        | ContentBlock::ReasoningTrace { text } => {
                            asst_chars += text.len();
                        }
                        ContentBlock::AnthropicThinking {
                            thinking,
                            signature,
                        } => {
                            asst_chars += thinking.len() + signature.len();
                        }
                        ContentBlock::OpenAIReasoning {
                            id,
                            summary,
                            encrypted_content,
                            status,
                        } => {
                            asst_chars += id.len()
                                + summary.iter().map(String::len).sum::<usize>()
                                + encrypted_content.as_ref().map(String::len).unwrap_or(0)
                                + status.as_ref().map(String::len).unwrap_or(0);
                        }
                        ContentBlock::Image { data, .. } => {
                            user_chars += data.len();
                        }
                        ContentBlock::OpenAICompaction { encrypted_content } => {
                            user_chars += encrypted_content.len();
                        }
                    }
                }
            }
        }

        // Use the last exact tool-definition measurement if available.
        // Fall back to the older rough estimate only before the first tool fetch.
        let tool_defs_count = if info.tool_defs_count > 0 {
            info.tool_defs_count
        } else {
            25
        };
        let tool_defs_chars = if info.tool_defs_chars > 0 {
            info.tool_defs_chars
        } else {
            tool_defs_count * 500
        };

        info.user_messages_chars = user_chars;
        info.user_messages_count = user_count;
        info.assistant_messages_chars = asst_chars;
        info.assistant_messages_count = asst_count;
        info.tool_calls_chars = tool_call_chars;
        info.tool_calls_count = tool_call_count;
        info.tool_results_chars = tool_result_chars;
        info.tool_results_count = tool_result_count;
        info.tool_defs_chars = tool_defs_chars;
        info.tool_defs_count = tool_defs_count;

        // Update total
        info.total_chars = info.system_prompt_chars
            + info.session_context_chars
            + info.project_agents_md_chars
            + info.global_agents_md_chars
            + info.skills_chars
            + info.selfdev_chars
            + info.memory_chars
            + info.prompt_overlay_chars
            + info.preferred_tools_chars
            + info.tool_defs_chars
            + info.user_messages_chars
            + info.assistant_messages_chars
            + info.tool_calls_chars
            + info.tool_results_chars;

        if let Ok(mut cache) = CACHE.lock() {
            *cache = Some((
                Instant::now(),
                CachedContextSnapshot {
                    session_key,
                    is_remote: self.is_remote,
                    display_messages_version: self.display_messages_version,
                    context_revision: self.context_revision,
                    message_count,
                    compaction_count,
                    compaction_summary_chars,
                    is_compacting,
                    snapshot: crate::tui::ContextSnapshot {
                        info: Some(info.clone()),
                        revision: self.context_revision,
                        fresh: true,
                    },
                },
            ));
        }

        crate::tui::ContextSnapshot {
            info: Some(info),
            revision: self.context_revision,
            fresh: true,
        }
    }

    fn context_info(&self) -> crate::prompt::ContextInfo {
        self.context_snapshot().info.unwrap_or_default()
    }

    fn context_limit(&self) -> Option<usize> {
        Some(self.context_limit as usize)
    }

    fn client_update_available(&self) -> bool {
        self.has_newer_binary()
    }

    fn server_update_available(&self) -> Option<bool> {
        if self.is_remote {
            self.remote_server_has_update
        } else {
            None
        }
    }

    fn info_widget_data(&self) -> crate::tui::info_widget::InfoWidgetData {
        let session_id = if self.is_remote {
            self.remote_session_id.as_deref()
        } else {
            Some(self.session.id.as_str())
        };

        let todos = if self.swarm_enabled && !self.swarm_plan_items.is_empty() {
            self.swarm_plan_items
                .iter()
                .map(|item| crate::todo::TodoItem {
                    content: item.content.clone(),
                    status: item.status.clone(),
                    priority: item.priority.clone(),
                    id: item.id.clone(),
                    group: None,
                    blocked_by: item.blocked_by.clone(),
                    assigned_to: item.assigned_to.clone(),
                    confidence: None,
                    completion_confidence: None,
                })
                .collect()
        } else {
            gather_todos_for_session(session_id)
        };

        let context_snapshot = self.context_snapshot();
        let context_info = if let Some(context_info) = context_snapshot.info.clone() {
            (context_info.total_chars > 0).then_some(context_info)
        } else {
            None
        };

        let uses_remote_widget_metadata = self.is_remote || self.is_replay_runtime();
        let (
            model,
            reasoning_effort,
            service_tier,
            native_compaction_mode,
            native_compaction_threshold_tokens,
        ) = if uses_remote_widget_metadata {
            (
                self.remote_provider_model.clone(),
                self.remote_reasoning_effort.clone(),
                self.remote_service_tier.clone(),
                None,
                None,
            )
        } else {
            (
                Some(self.provider.model()),
                self.provider.reasoning_effort(),
                self.provider.service_tier(),
                self.provider.native_compaction_mode(),
                self.provider.native_compaction_threshold_tokens(),
            )
        };

        let (session_count, client_count) = if self.is_remote {
            (Some(self.remote_sessions.len()), None)
        } else {
            (None, None)
        };
        let session_name = self.session_display_name().map(|name| {
            if let Some(ref srv) = self.remote_server_short_name {
                format!("{} {}", srv, name)
            } else {
                name
            }
        });

        let memory_info = gather_memory_info(self.memory_enabled);

        // Gather swarm info
        let swarm_info = if self.swarm_enabled {
            let subagent_status = self.subagent_status.clone();
            let mut members: Vec<crate::protocol::SwarmMemberStatus> = Vec::new();
            let (session_count, client_count, session_names, has_activity) = if self.is_remote {
                members = self.remote_swarm_members.clone();
                let session_names = if !members.is_empty() {
                    members
                        .iter()
                        .map(|m| {
                            m.friendly_name
                                .clone()
                                .unwrap_or_else(|| m.session_id.chars().take(8).collect())
                        })
                        .collect()
                } else {
                    self.remote_sessions.clone()
                };
                let session_count = if !members.is_empty() {
                    members.len()
                } else {
                    self.remote_sessions.len()
                };
                let has_activity = members
                    .iter()
                    .any(|m| m.status != "ready" || m.detail.is_some());
                (
                    session_count,
                    self.remote_client_count,
                    session_names,
                    has_activity,
                )
            } else {
                let (status, detail) = match &self.status {
                    ProcessingStatus::Idle => ("ready".to_string(), None),
                    ProcessingStatus::Sending => {
                        ("running".to_string(), Some("sending".to_string()))
                    }
                    ProcessingStatus::Connecting(phase) => {
                        ("running".to_string(), Some(phase.to_string()))
                    }
                    ProcessingStatus::Thinking(_) => ("thinking".to_string(), None),
                    ProcessingStatus::Streaming => {
                        ("running".to_string(), Some("streaming".to_string()))
                    }
                    ProcessingStatus::WaitingForNetwork { listener } => {
                        ("waiting_network".to_string(), Some(listener.clone()))
                    }
                    ProcessingStatus::RunningTool(name) => {
                        ("running".to_string(), Some(format!("tool: {}", name)))
                    }
                };
                let detail = subagent_status.clone().or(detail);
                let has_activity = status != "ready" || detail.is_some();
                if has_activity {
                    members.push(crate::protocol::SwarmMemberStatus {
                        session_id: self.session.id.clone(),
                        friendly_name: Some(self.session.display_name().to_string()),
                        status,
                        detail,
                        role: None,
                        is_headless: Some(false),
                        live_attachments: Some(1),
                        status_age_secs: Some(0),
                        output_tail: None,
                        report_back_to_session_id: None,
                    });
                }
                (
                    1,
                    None,
                    vec![self.session.display_name().to_string()],
                    has_activity,
                )
            };

            // Only show if there's something interesting
            if has_activity || session_count > 1 || client_count.is_some() {
                Some(crate::tui::info_widget::SwarmInfo {
                    session_count,
                    subagent_status,
                    client_count,
                    session_names,
                    members,
                })
            } else {
                None
            }
        } else {
            None
        };

        // Gather background task info
        let background_info = {
            // Get running background tasks count
            let bg_manager = crate::background::global();
            let (running_count, running_tasks, progress) = bg_manager.running_snapshot();

            if running_count > 0 {
                Some(crate::tui::info_widget::BackgroundInfo {
                    running_count,
                    running_tasks,
                    progress_summary: progress.as_ref().map(|progress| progress.label.clone()),
                    progress_detail: progress
                        .as_ref()
                        .and_then(|progress| progress.detail.clone()),
                    memory_agent_active: false,
                    memory_agent_turns: 0,
                })
            } else {
                None
            }
        };

        let route = self.widget_route_info(model.as_deref());
        let auth_method = self.widget_auth_method(route);
        let usage_info = self.widget_usage_info(route, auth_method);

        let tokens_per_second = if matches!(self.status, ProcessingStatus::Streaming) {
            self.compute_streaming_tps()
        } else {
            None
        };

        let cache_hit_info =
            (self.token_accounting.total_cache_reported_input_tokens > 0).then(|| {
                crate::tui::info_widget::CacheHitInfo {
                    reported_input_tokens: self.token_accounting.total_cache_reported_input_tokens,
                    read_tokens: self.token_accounting.total_cache_read_tokens,
                    creation_tokens: self.token_accounting.total_cache_creation_tokens,
                    optimal_input_tokens: self.token_accounting.total_cache_optimal_input_tokens,
                    last_reported_input_tokens: self
                        .token_accounting
                        .last_cache_reported_input_tokens,
                    last_read_tokens: self.token_accounting.last_cache_read_tokens,
                    last_creation_tokens: self.token_accounting.last_cache_creation_tokens,
                    last_optimal_input_tokens: self
                        .token_accounting
                        .last_cache_optimal_input_tokens,
                    miss_attributions: self
                        .kv_cache
                        .kv_cache_miss_samples
                        .iter()
                        .rev()
                        .map(|sample| crate::tui::info_widget::CacheMissAttribution {
                            turn_number: sample.turn_number,
                            call_index: sample.call_index,
                            missed_tokens: sample.missed_tokens,
                            reason: sample.reason.label().to_string(),
                        })
                        .collect(),
                }
            });

        // Get active mermaid diagrams - only for margin mode (pinned mode uses dedicated pane)
        let diagrams = if self.diagram_mode == crate::config::DiagramDisplayMode::Margin {
            crate::tui::mermaid::get_active_diagrams()
        } else {
            Vec::new()
        };

        let workspace_rows = if self.workspace_client.is_enabled() {
            let session_id = if self.is_remote {
                self.remote_session_id.as_deref()
            } else {
                Some(self.session.id.as_str())
            };
            self.workspace_client
                .visible_rows(5, session_id, self.is_processing)
        } else {
            Vec::new()
        };

        let workspace_animation_tick = self.app_started.elapsed().as_millis() as u64 / 180;

        let compaction_info = if !self.is_remote && self.provider.uses_jcode_compaction() {
            let compaction = self.registry.compaction();
            compaction.try_read().ok().and_then(|manager| {
                let compacted_messages = manager.compacted_count();
                let summary_chars = manager.summary_chars();
                let is_compacting = manager.is_compacting();
                (is_compacting || compacted_messages > 0 || summary_chars > 0).then(|| {
                    crate::tui::info_widget::CompactionInfo {
                        is_compacting,
                        compacted_messages,
                        active_messages: manager.active_messages_count(),
                        summary_chars,
                        mode: manager.mode().as_str().to_string(),
                    }
                })
            })
        } else {
            None
        };

        crate::tui::info_widget::InfoWidgetData {
            todos,
            context_info,
            context_info_stale: !context_snapshot.fresh,
            queue_mode: Some(self.queue_mode),
            context_limit: Some(self.context_limit as usize),
            model,
            reasoning_effort,
            service_tier,
            native_compaction_mode,
            native_compaction_threshold_tokens,
            session_count,
            session_name,
            working_dir: self.session.working_dir.clone(),
            client_count,
            memory_info,
            swarm_info,
            background_info,
            usage_info,
            tokens_per_second,
            provider_name: if uses_remote_widget_metadata {
                self.remote_provider_name
                    .clone()
                    .or_else(|| Some(self.provider.display_name()))
            } else {
                Some(self.provider.display_name())
            },
            auth_method,
            upstream_provider: self.upstream_provider.clone(),
            connection_type: self.connection_type.clone(),
            diagrams,
            workspace_rows,
            workspace_animation_tick,
            ambient_info: gather_ambient_info(crate::config::config().ambient.enabled),
            observed_context_tokens: self.current_stream_context_tokens(),
            cache_hit_info,
            compaction_info,
            is_compacting: if !self.is_remote && self.provider.uses_jcode_compaction() {
                let compaction = self.registry.compaction();
                compaction
                    .try_read()
                    .map(|m| m.is_compacting())
                    .unwrap_or(false)
            } else {
                false
            },
            git_info: gather_git_info(),
        }
    }

    fn workspace_mode_enabled(&self) -> bool {
        self.workspace_client.is_enabled()
    }

    fn workspace_map_rows(&self) -> Vec<crate::tui::workspace_map::VisibleWorkspaceRow> {
        let session_id = if self.is_remote {
            self.remote_session_id.as_deref()
        } else {
            Some(self.session.id.as_str())
        };
        self.workspace_client
            .visible_rows(5, session_id, self.is_processing)
    }

    fn workspace_animation_tick(&self) -> u64 {
        self.app_started.elapsed().as_millis() as u64 / 180
    }

    fn render_streaming_markdown(&self, width: usize) -> Vec<ratatui::text::Line<'static>> {
        let mut renderer = self.streaming_md_renderer.borrow_mut();
        renderer.set_width(Some(width));
        renderer.update(&self.streaming.streaming_text)
    }

    fn centered_mode(&self) -> bool {
        self.centered
    }

    fn auth_status(&self) -> crate::auth::AuthStatus {
        crate::auth::AuthStatus::check_fast()
    }

    fn diagram_mode(&self) -> crate::config::DiagramDisplayMode {
        self.diagram_mode
    }

    fn inline_swarm_gallery_active(&self) -> bool {
        if self.debug_force_inline_gallery {
            return !self.inline_swarm_members().is_empty();
        }
        self.swarm_enabled
            && matches!(
                crate::config::config().agents.swarm_spawn_mode,
                crate::config::SwarmSpawnMode::Inline
            )
            && !self.inline_swarm_members().is_empty()
    }

    fn inline_swarm_members(&self) -> Vec<crate::protocol::SwarmMemberStatus> {
        if self.debug_force_inline_gallery {
            return self.remote_swarm_members.clone();
        }
        if !self.swarm_enabled {
            return Vec::new();
        }
        // Scope the inline gallery to the subtree this session actually spawned.
        // Other sessions can share the same swarm (e.g. same repo) without this
        // session having spawned them; showing those would be noise. The spawn
        // tree is reconstructed from each member's `report_back_to_session_id`
        // parent edge.
        let self_id = if self.is_remote {
            self.remote_session_id.as_deref()
        } else {
            Some(self.session.id.as_str())
        };
        match self_id {
            Some(self_id) => filter_inline_swarm_subtree(&self.remote_swarm_members, self_id),
            None => self.remote_swarm_members.clone(),
        }
    }

    fn swarm_panel_selected(&self) -> usize {
        let count = self.inline_swarm_members().len();
        if count == 0 {
            0
        } else {
            self.swarm_panel_selected.min(count - 1)
        }
    }

    fn swarm_panel_focused(&self) -> bool {
        self.swarm_panel_focused
    }

    fn diagram_focus(&self) -> bool {
        self.diagram_focus
    }

    fn diagram_index(&self) -> usize {
        self.diagram_index
    }

    fn diagram_scroll(&self) -> (i32, i32) {
        (self.diagram_scroll_x, self.diagram_scroll_y)
    }

    fn diagram_pane_ratio(&self) -> u8 {
        self.animated_diagram_pane_ratio()
    }

    fn diagram_pane_ratio_user_adjusted(&self) -> bool {
        self.diagram_pane_ratio_user_adjusted
    }

    fn diagram_pane_animating(&self) -> bool {
        self.diagram_pane_anim_start
            .map(|s| s.elapsed().as_secs_f32() < Self::DIAGRAM_PANE_ANIM_DURATION)
            .unwrap_or(false)
    }

    fn diagram_pane_enabled(&self) -> bool {
        self.diagram_pane_enabled
    }

    fn diagram_pane_position(&self) -> crate::config::DiagramPanePosition {
        self.diagram_pane_position
    }

    fn diagram_zoom(&self) -> u8 {
        self.diagram_zoom
    }
    fn diff_pane_scroll(&self) -> usize {
        self.diff_pane_scroll
    }
    fn diff_pane_scroll_x(&self) -> i32 {
        self.diff_pane_scroll_x
    }
    fn side_panel_image_zoom_percent(&self) -> u8 {
        self.side_panel_image_zoom_percent
    }
    fn diff_pane_focus(&self) -> bool {
        self.diff_pane_focus
    }
    fn side_panel(&self) -> &crate::side_panel::SidePanelSnapshot {
        &self.side_panel
    }
    fn pin_images(&self) -> bool {
        self.pin_images && !self.side_panel_user_hidden
    }

    fn inline_images_visible(&self) -> bool {
        self.inline_images_visible
    }
    fn image_expand_level(
        &self,
        image_id: u64,
    ) -> crate::tui::ui::inline_image_ui::ImageExpandLevel {
        self.expanded_images
            .get(&image_id)
            .copied()
            .unwrap_or_default()
    }
    fn expanded_images_version(&self) -> u64 {
        self.expanded_images_version
    }
    fn pinned_images_auto_hide_remaining_secs(&self) -> Option<u64> {
        if self.side_panel_user_hidden
            || self.side_panel.focused_page().is_some()
            || self.diff_mode.is_file()
        {
            return None;
        }
        self.pinned_images_auto_hide_deadline.map(|deadline| {
            deadline
                .saturating_duration_since(std::time::Instant::now())
                .as_secs()
                .saturating_add(1)
        })
    }
    fn chat_native_scrollbar(&self) -> bool {
        self.chat_native_scrollbar
    }
    fn side_panel_native_scrollbar(&self) -> bool {
        self.side_panel_native_scrollbar
    }
    fn diff_line_wrap(&self) -> bool {
        crate::config::config().display.diff_line_wrap
    }
    fn inline_interactive_state(&self) -> Option<&crate::tui::InlineInteractiveState> {
        self.inline_interactive_state.as_ref()
    }

    fn inline_view_state(&self) -> Option<&crate::tui::InlineViewState> {
        self.inline_view_state.as_ref()
    }

    fn changelog_scroll(&self) -> Option<usize> {
        self.changelog_scroll
    }

    fn help_scroll(&self) -> Option<usize> {
        self.help_scroll
    }

    fn model_status_overlay(&self) -> Option<(usize, &str)> {
        self.model_status_scroll
            .map(|scroll| (scroll, self.model_status_content.as_str()))
    }

    fn session_picker_overlay(
        &self,
    ) -> Option<&RefCell<crate::tui::session_picker::SessionPicker>> {
        self.session_picker_overlay.as_ref()
    }

    fn login_picker_overlay(&self) -> Option<&RefCell<crate::tui::login_picker::LoginPicker>> {
        self.login_picker_overlay.as_ref()
    }

    fn account_picker_overlay(
        &self,
    ) -> Option<&RefCell<crate::tui::account_picker::AccountPicker>> {
        self.account_picker_overlay.as_ref()
    }

    fn usage_overlay(&self) -> Option<&RefCell<crate::tui::usage_overlay::UsageOverlay>> {
        self.usage_overlay.as_ref()
    }

    fn working_dir(&self) -> Option<String> {
        self.session.working_dir.clone()
    }

    fn now_millis(&self) -> u64 {
        self.app_started.elapsed().as_millis() as u64
    }

    fn copy_badge_ui(&self) -> crate::tui::CopyBadgeUiState {
        self.copy_badge_ui.clone()
    }

    fn copy_selection_mode(&self) -> bool {
        self.copy_selection_mode
    }

    fn copy_selection_range(&self) -> Option<crate::tui::CopySelectionRange> {
        self.normalized_copy_selection()
    }

    fn copy_selection_status(&self) -> Option<crate::tui::CopySelectionStatus> {
        if !self.copy_selection_mode {
            return None;
        }

        // Compute selection metrics without building the full selected string,
        // which previously re-allocated the entire selection on every render
        // frame and drag move (O(selection) per frame; a "select all" rebuilt
        // the whole transcript text repeatedly).
        let (selected_chars, selected_lines) = self
            .normalized_copy_selection()
            .and_then(crate::tui::ui::copy_selection_metrics)
            .unwrap_or((0, 0));
        let has_selection = selected_chars > 0;
        Some(crate::tui::CopySelectionStatus {
            pane: self
                .current_copy_selection_pane()
                .unwrap_or(crate::tui::CopySelectionPane::Chat),
            has_action: has_selection,
            selected_chars,
            selected_lines: if has_selection {
                selected_lines.max(1)
            } else {
                0
            },
            dragging: self.copy_selection_dragging,
        })
    }

    fn onboarding_preview_mode(&self) -> bool {
        self.onboarding_preview_mode
    }

    fn onboarding_welcome_active(&self) -> bool {
        App::onboarding_welcome_active(self)
    }

    fn onboarding_welcome_kind(&self) -> crate::tui::OnboardingWelcomeKind {
        App::onboarding_welcome_kind(self)
    }

    fn suggestion_prompts(&self) -> Vec<(String, String)> {
        App::suggestion_prompts(self)
    }

    fn cache_ttl_status(&self) -> Option<crate::tui::CacheTtlInfo> {
        let last_completed = self.last_api_completed?;
        let provider = self.provider_name();
        let model = self.provider_model();
        let last_provider = self.last_api_completed_provider.as_deref()?;
        let last_model = self.last_api_completed_model.as_deref()?;
        if last_provider != provider || last_model != model {
            return None;
        }
        let ttl_secs = crate::tui::cache_ttl_for_provider_model(provider, Some(&model))?;
        let elapsed = last_completed.elapsed().as_secs();
        let remaining = ttl_secs.saturating_sub(elapsed);
        Some(crate::tui::CacheTtlInfo {
            remaining_secs: remaining,
            ttl_secs,
            is_cold: remaining == 0,
            cached_tokens: self.last_turn_input_tokens,
        })
    }
}

impl App {
    /// Toggle keyboard focus on the inline swarm panel. Returns the new state.
    /// Focus is only meaningful while the panel is actually visible.
    pub(crate) fn toggle_swarm_panel_focus(&mut self) -> bool {
        if !self.inline_swarm_gallery_active() {
            self.swarm_panel_focused = false;
            return false;
        }
        self.swarm_panel_focused = !self.swarm_panel_focused;
        if self.swarm_panel_focused {
            // Clamp selection on entry.
            let count = self.inline_swarm_members().len();
            if count > 0 {
                self.swarm_panel_selected = self.swarm_panel_selected.min(count - 1);
            }
        }
        self.swarm_panel_focused
    }

    #[allow(dead_code)]
    pub(crate) fn set_swarm_panel_focus(&mut self, focused: bool) {
        self.swarm_panel_focused = focused && self.inline_swarm_gallery_active();
    }

    /// Move the swarm panel selection by `delta` (e.g. +1 for next, -1 for
    /// previous), saturating at the ends.
    pub(crate) fn move_swarm_panel_selection(&mut self, delta: isize) {
        let count = self.inline_swarm_members().len();
        if count == 0 {
            return;
        }
        let cur = self.swarm_panel_selected.min(count - 1) as isize;
        let next = (cur + delta).clamp(0, count as isize - 1);
        self.swarm_panel_selected = next as usize;
    }

    /// Handle a key while the swarm panel is focused. Returns true if the key was
    /// consumed.
    pub(crate) fn handle_swarm_panel_key(
        &mut self,
        code: crossterm::event::KeyCode,
        modifiers: crossterm::event::KeyModifiers,
    ) -> bool {
        use crossterm::event::KeyCode;
        if !self.swarm_panel_focused || !self.inline_swarm_gallery_active() {
            return false;
        }
        if modifiers.contains(crossterm::event::KeyModifiers::CONTROL) {
            return false;
        }
        match code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.move_swarm_panel_selection(1);
                true
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.move_swarm_panel_selection(-1);
                true
            }
            KeyCode::Char('g') | KeyCode::Home => {
                self.swarm_panel_selected = 0;
                true
            }
            KeyCode::Char('G') | KeyCode::End => {
                let count = self.inline_swarm_members().len();
                self.swarm_panel_selected = count.saturating_sub(1);
                true
            }
            KeyCode::Char('o') | KeyCode::Enter => {
                self.pop_out_selected_swarm_agent();
                true
            }
            KeyCode::Esc => {
                self.swarm_panel_focused = false;
                true
            }
            _ => false,
        }
    }

    /// Open the currently selected swarm agent's session in a new terminal
    /// window (pop-out), reusing the resume-in-new-terminal launcher.
    pub(crate) fn pop_out_selected_swarm_agent(&mut self) {
        let members = self.inline_swarm_members();
        if members.is_empty() {
            self.set_status_notice("No swarm agents to open");
            return;
        }
        let order = crate::tui::info_widget::swarm_gallery::members_display_order(&members);
        let idx = self.swarm_panel_selected.min(order.len().saturating_sub(1));
        let Some(session_id) = order.get(idx).cloned() else {
            self.set_status_notice("No swarm agent selected");
            return;
        };
        let label = members
            .iter()
            .find(|m| m.session_id == session_id)
            .and_then(|m| m.friendly_name.clone())
            .unwrap_or_else(|| session_id.chars().take(8).collect());

        let exe = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("jcode"));
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        match jcode_app_core::session_launch::spawn_resume_in_new_terminal(&exe, &session_id, &cwd)
        {
            Ok(true) => self.set_status_notice(format!("Opened {label} in a new window")),
            Ok(false) => self.set_status_notice(format!(
                "Could not open a terminal for {label} (no emulator found)"
            )),
            Err(e) => self.set_status_notice(format!("Failed to open {label}: {e}")),
        }
    }
}

/// Restrict swarm members to the subtree rooted at `self_id`: `self_id` itself
/// plus every member transitively spawned by it (reachable by following the
/// `report_back_to_session_id` parent edge upward to `self_id`).
///
/// This keeps the inline swarm gallery scoped to the agents a session actually
/// spawned, rather than every member that happens to share the swarm (for
/// example, unrelated sessions in the same repository).
///
/// If no member identifies as `self_id` and none reports back to it (the common
/// case for a plain session that has not spawned anyone), the result is empty.
pub(crate) fn filter_inline_swarm_subtree(
    members: &[crate::protocol::SwarmMemberStatus],
    self_id: &str,
) -> Vec<crate::protocol::SwarmMemberStatus> {
    use std::collections::{HashMap, HashSet};

    let parent_of: HashMap<&str, Option<&str>> = members
        .iter()
        .map(|m| {
            (
                m.session_id.as_str(),
                m.report_back_to_session_id.as_deref(),
            )
        })
        .collect();

    // A member is in-subtree if walking its parent chain reaches `self_id`.
    let in_subtree = |start: &str| -> bool {
        if start == self_id {
            return true;
        }
        let mut visited: HashSet<&str> = HashSet::new();
        let mut current = start;
        while let Some(Some(parent)) = parent_of.get(current) {
            if !visited.insert(current) {
                break; // cycle guard
            }
            if *parent == self_id {
                return true;
            }
            current = parent;
        }
        false
    };

    members
        .iter()
        .filter(|m| in_subtree(m.session_id.as_str()))
        .cloned()
        .collect()
}

#[cfg(test)]
mod inline_swarm_subtree_tests {
    use super::filter_inline_swarm_subtree;
    use crate::protocol::SwarmMemberStatus;

    fn member(id: &str, parent: Option<&str>) -> SwarmMemberStatus {
        SwarmMemberStatus {
            session_id: id.to_string(),
            friendly_name: Some(id.to_string()),
            status: "running".to_string(),
            detail: None,
            role: None,
            is_headless: Some(true),
            live_attachments: None,
            status_age_secs: Some(1),
            output_tail: None,
            report_back_to_session_id: parent.map(str::to_string),
        }
    }

    fn ids(members: Vec<SwarmMemberStatus>) -> Vec<String> {
        let mut v: Vec<String> = members.into_iter().map(|m| m.session_id).collect();
        v.sort();
        v
    }

    #[test]
    fn includes_self_and_direct_children() {
        let members = vec![
            member("me", None),
            member("child_a", Some("me")),
            member("child_b", Some("me")),
            member("stranger", None),
        ];
        assert_eq!(
            ids(filter_inline_swarm_subtree(&members, "me")),
            vec!["child_a", "child_b", "me"]
        );
    }

    #[test]
    fn includes_transitive_descendants() {
        let members = vec![
            member("me", None),
            member("child", Some("me")),
            member("grandchild", Some("child")),
        ];
        assert_eq!(
            ids(filter_inline_swarm_subtree(&members, "me")),
            vec!["child", "grandchild", "me"]
        );
    }

    #[test]
    fn excludes_siblings_and_unrelated_sessions() {
        // Two coordinators sharing one swarm. Each should only see its own kids.
        let members = vec![
            member("coord_a", None),
            member("a_child", Some("coord_a")),
            member("coord_b", None),
            member("b_child", Some("coord_b")),
        ];
        assert_eq!(
            ids(filter_inline_swarm_subtree(&members, "coord_a")),
            vec!["a_child", "coord_a"]
        );
        assert_eq!(
            ids(filter_inline_swarm_subtree(&members, "coord_b")),
            vec!["b_child", "coord_b"]
        );
    }

    #[test]
    fn plain_session_with_no_children_sees_only_itself_if_present() {
        let members = vec![member("stranger", None), member("other", None)];
        assert!(filter_inline_swarm_subtree(&members, "me").is_empty());
    }

    #[test]
    fn cycle_is_guarded() {
        // Pathological parent cycle must not loop forever.
        let members = vec![
            member("a", Some("b")),
            member("b", Some("a")),
            member("me", None),
            member("child", Some("me")),
        ];
        assert_eq!(
            ids(filter_inline_swarm_subtree(&members, "me")),
            vec!["child", "me"]
        );
    }
}
