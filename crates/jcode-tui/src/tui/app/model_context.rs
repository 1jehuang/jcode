#[path = "model_context_commands.rs"]
mod model_commands;
pub(super) use model_commands::*;

use super::*;

/// Reroute target offered after a provider guardrail/refusal stop. Guardrail
/// refusals are model-side policy stops, so retrying the same model rarely
/// helps; hopping to the strongest Anthropic route often does.
const GUARDRAIL_REROUTE_MODEL: &str = "claude-opus-4-8";

impl App {
    /// Keep the actionable cause visible without dumping nested transport errors
    /// or repeated request/configuration advice into the transcript.
    pub(super) fn compact_provider_failure(reason: &str) -> String {
        let reason = reason.split_whitespace().collect::<Vec<_>>().join(" ");
        let compact = reason.find('{').and_then(|start| {
            let mut values = serde_json::Deserializer::from_str(&reason[start..])
                .into_iter::<serde_json::Value>();
            let value = values.next()?.ok()?;
            let message = value
                .pointer("/error/message")
                .or_else(|| value.get("message"))
                .and_then(serde_json::Value::as_str)?;
            Some(format!("{}{}", &reason[..start], message))
        });
        crate::util::truncate_str(compact.as_deref().unwrap_or(&reason), 240).to_string()
    }

    /// Shared post-switch bookkeeping for every local model/provider switch
    /// path (/model, model cycling, failover, post-login activation).
    ///
    /// Centralized so all paths agree on what a switch means: reset provider
    /// session ids, drop upstream/status details, invalidate the model picker
    /// cache, update the context limit, recompute the session provider key,
    /// and persist the session. Returns the active model after the switch.
    ///
    /// `model_request` is the original request string (it may carry an
    /// explicit provider prefix like `openrouter:`); for provider-level
    /// switches without a model request, pass the active model name.
    pub(super) fn finalize_model_switch(&mut self, model_request: &str) -> String {
        self.provider_session_id = None;
        self.session.provider_session_id = None;
        self.upstream_provider = None;
        self.status_detail = None;
        self.invalidate_model_picker_cache();
        let active_model = self.provider.model();
        self.update_context_limit_for_model(&active_model, None);
        self.session.provider_key =
            crate::provider::MultiProvider::session_provider_key_after_model_switch(
                model_request,
                self.provider.name(),
                self.session.provider_key.as_deref(),
            );
        if let Some(label) = self.provider.oauth_account_label() {
            let method = if self.provider.name().eq_ignore_ascii_case("openai") {
                "openai-oauth"
            } else {
                "claude-oauth"
            };
            self.session.route_api_method = Some(format!("{method}@{label}"));
        } else if self
            .session
            .route_api_method
            .as_deref()
            .is_some_and(|method| jcode_provider_core::oauth_account_label(method).is_some())
        {
            self.session.route_api_method = None;
        }
        self.session.model = Some(active_model.clone());
        if let Err(error) = self.session.save() {
            self.push_display_message(DisplayMessage::error(format!(
                "Model switched for this session, but the saved route could not be persisted: {error}"
            )));
        }
        active_model
    }

    fn apply_provider_switch_for_failover(
        &mut self,
        prompt: &crate::provider::ProviderFailoverPrompt,
    ) -> anyhow::Result<String> {
        // Pool edits during the countdown apply to both ends. Derive the
        // source from the live provider, never stale prompt/session metadata.
        let provider = self.provider.name().to_ascii_lowercase();
        let provider = match provider.as_str() {
            "anthropic" | "claude" => "claude",
            "openai" => "openai",
            "copilot" | "gemini" | "cursor" | "antigravity" | "bedrock" | "openrouter" => {
                provider.as_str()
            }
            _ => anyhow::bail!("Source identity is unknown. Use /model to switch manually."),
        };
        let credential = self.provider.active_resolved_credential();
        let source_key = match (provider, credential) {
            ("claude" | "openai", Some(jcode_provider_core::ResolvedCredential::Oauth)) => {
                let label = self.provider.oauth_account_label().ok_or_else(|| {
                    anyhow::anyhow!(
                        "Source account identity is unknown. Use /model to switch manually."
                    )
                })?;
                let exists = if provider == "openai" {
                    crate::auth::codex::list_accounts()
                        .unwrap_or_default()
                        .iter()
                        .any(|account| account.label == label)
                } else {
                    crate::auth::claude::list_accounts()
                        .unwrap_or_default()
                        .iter()
                        .any(|account| account.label == label)
                };
                anyhow::ensure!(
                    exists,
                    "Source account no longer exists. Use /model to switch manually."
                );
                crate::auth::account_pool::account_key(provider, Some(&label))
            }
            ("claude", Some(jcode_provider_core::ResolvedCredential::ApiKey)) => {
                "anthropic-api".to_string()
            }
            ("openai", Some(jcode_provider_core::ResolvedCredential::ApiKey)) => {
                "openai-api".to_string()
            }
            ("claude" | "openai", _) => {
                anyhow::bail!("Source credential is unknown. Use /model to switch manually.")
            }
            _ => provider.to_string(),
        };
        let api_key = source_key.ends_with("-api")
            || credential == Some(jcode_provider_core::ResolvedCredential::ApiKey)
            || crate::provider_catalog::login_providers()
                .iter()
                .any(|login| {
                    login.id == source_key
                        && login.auth_kind == crate::provider_catalog::LoginProviderAuthKind::ApiKey
                });
        let pool = crate::auth::account_pool::AccountPool::load();
        anyhow::ensure!(
            !pool.excluded.contains(&source_key)
                && pool.is_member(
                    &source_key,
                    crate::auth::account_pool::default_member(&source_key, api_key)
                ),
            "Source is no longer in the auto-switch pool. Use /model to switch manually."
        );
        let route = prompt.target_route.as_deref().ok_or_else(|| {
            anyhow::anyhow!(
                "No pool-approved target route was supplied. Use /model to switch manually."
            )
        })?;
        anyhow::ensure!(
            crate::provider::MultiProvider::is_failover_route_pool_approved(route),
            "Target is no longer in the auto-switch pool. Use /model to switch manually."
        );
        self.provider.set_model(route)?;
        Ok(self.finalize_model_switch(route))
    }

    pub(super) fn restore_failover_payload(&mut self, payload: super::FallbackResendPayload) {
        if self.input.is_empty() {
            self.input = payload.raw_input.unwrap_or(payload.content);
            self.cursor_pos = self.input.len();
        }
        for image in payload.images {
            if !self.pending_images.contains(&image) {
                self.pending_images.push(image);
            }
        }
    }

    pub(super) fn cancel_pending_provider_failover(&mut self, notice: impl Into<String>) {
        if self.pending_automatic_failover_route.take().is_some()
            && let Some(payload) = self.pending_fallback_resend.take()
        {
            self.restore_failover_payload(payload);
        }
        let Some(pending) = self.pending_provider_failover.take() else {
            return;
        };
        if let Some(payload) = pending.remote_resend {
            self.restore_failover_payload(payload);
        }
        self.push_display_message(DisplayMessage::system(format!(
            "Auto-switch canceled. Kept {} active. Use /model to switch manually.",
            pending.prompt.from_label,
        )));
        self.set_status_notice(notice);
    }

    pub(super) fn maybe_progress_provider_failover_countdown(&mut self) -> bool {
        let Some(pending) = self.pending_provider_failover.clone() else {
            return false;
        };
        if self.is_processing {
            return false;
        }
        if crate::config::Config::load()
            .provider
            .cross_provider_failover
            != crate::config::CrossProviderFailoverMode::Countdown
        {
            self.cancel_pending_provider_failover("Auto-switch disabled");
            return true;
        }
        let now = Instant::now();
        if now < pending.deadline {
            let remaining = pending
                .deadline
                .saturating_duration_since(now)
                .as_secs()
                .saturating_add(1)
                .min(3);
            self.set_status_notice(format!(
                "Provider auto-switch → {} in {}s (Esc to cancel)",
                pending.prompt.to_label, remaining
            ));
            return true;
        }

        self.pending_provider_failover = None;
        if self.is_remote {
            let selection = pending.prompt.target_route.as_deref().and_then(|target| {
                self.fallback_candidate_routes()
                    .iter()
                    .filter(|route| route.available)
                    .map(crate::provider::RouteSelection::from_model_route)
                    .find(|selection| selection.routed_model_spec() == target)
            });
            let Some(selection) = selection else {
                if let Some(payload) = pending.remote_resend {
                    self.restore_failover_payload(payload);
                }
                self.push_display_message(DisplayMessage::error(
                    "Auto-switch target is no longer available. Use /model to switch manually."
                        .to_string(),
                ));
                return true;
            };
            self.pending_automatic_failover_route = Some(selection);
            self.pending_fallback_resend = pending.remote_resend;
            self.set_status_notice(format!(
                "Switching → {} (will retry)",
                pending.prompt.to_label
            ));
            return true;
        }
        match self.apply_provider_switch_for_failover(&pending.prompt) {
            Ok(_active_model) => {
                self.push_display_message(DisplayMessage::system(format!(
                    "Switched {} → {}. Retrying.",
                    pending.prompt.from_label, pending.prompt.to_label,
                )));
                self.set_status_notice(format!(
                    "Provider → {} (retrying)",
                    pending.prompt.to_label
                ));
                self.pending_turn = true;
                true
            }
            Err(error) => {
                self.push_display_message(DisplayMessage::error(format!(
                    "Failed to switch provider to {}: {}",
                    pending.prompt.to_label, error
                )));
                self.set_status_notice("Provider switch failed");
                true
            }
        }
    }

    fn handle_provider_failover_prompt(&mut self, prompt: crate::provider::ProviderFailoverPrompt) {
        // The provider already exhausted its usable accounts. Do not let todo
        // pokes or held retries resend on the same unavailable route while a
        // manual switch or configured countdown awaits a decision.
        super::commands::disable_auto_poke(self);
        self.overnight_auto_poke = None;
        let remote_resend = if self.is_remote {
            self.rate_limit_pending_message
                .as_ref()
                .map(|pending| super::FallbackResendPayload {
                    content: pending.content.clone(),
                    images: pending.images.clone(),
                    is_system: pending.is_system,
                    auto_retry: pending.auto_retry,
                    system_reminder: pending.system_reminder.clone(),
                    raw_input: self.last_submitted_input.clone(),
                })
        } else {
            None
        };
        self.handle_provider_failover_prompt_with_payload(prompt, remote_resend);
    }

    fn handle_provider_failover_prompt_with_payload(
        &mut self,
        prompt: crate::provider::ProviderFailoverPrompt,
        remote_resend: Option<super::FallbackResendPayload>,
    ) {
        super::commands::disable_auto_poke(self);
        self.overnight_auto_poke = None;
        self.clear_pending_remote_retry();
        self.pending_provider_failover = None;
        self.pending_automatic_failover_route = None;
        self.clear_pending_fallback_offer();
        let reason = Self::compact_provider_failure(&prompt.reason);
        let manual_message = if prompt.target_route.is_none() && prompt.to_provider.contains(':') {
            format!(
                "{} unavailable: {}\nUse /model {} to switch and resend. Metered profiles require a manual switch.",
                prompt.from_label, reason, prompt.to_provider,
            )
        } else {
            format!(
                "{} unavailable: {}\nUse /model to switch to {} and resend.",
                prompt.from_label, reason, prompt.to_label,
            )
        };

        match crate::config::Config::load()
            .provider
            .cross_provider_failover
        {
            crate::config::CrossProviderFailoverMode::Manual => {
                self.push_display_message(DisplayMessage::system(manual_message));
                self.set_status_notice(format!(
                    "{} unavailable; switch manually if desired",
                    prompt.from_label
                ));
            }
            crate::config::CrossProviderFailoverMode::Countdown
                if prompt.target_route.as_deref().is_some_and(|route| {
                    self.is_remote
                        || crate::provider::MultiProvider::is_failover_route_pool_approved(route)
                }) && (!self.is_remote || remote_resend.is_some()) =>
            {
                self.pending_provider_failover = Some(super::PendingProviderFailover {
                    prompt: prompt.clone(),
                    deadline: Instant::now() + Duration::from_secs(3),
                    remote_resend,
                });
                self.push_display_message(DisplayMessage::system(format!(
                    "{} unavailable: {}\nSwitching to {} and retrying in 3s. Esc to cancel.",
                    prompt.from_label, reason, prompt.to_label,
                )));
                self.set_status_notice(format!(
                    "Provider auto-switch → {} in 3s (Esc to cancel)",
                    prompt.to_label
                ));
            }
            _ => {
                self.push_display_message(DisplayMessage::system(manual_message));
                self.set_status_notice(format!(
                    "{} unavailable; manual switch suggested",
                    prompt.from_label
                ));
            }
        }
    }

    /// The model routes to consider when computing an error fallback, working in
    /// both local and remote sessions. Mirrors the model-picker's route source so
    /// the offered fallback matches what `/model` would show.
    fn fallback_candidate_routes(&self) -> Vec<crate::provider::ModelRoute> {
        if self.is_remote {
            if !self.remote_model_options.is_empty() {
                self.remote_model_options.clone()
            } else {
                self.build_remote_model_routes_fallback()
            }
        } else {
            self.provider.model_routes()
        }
    }

    /// The api_method string of the route currently in use, used to exclude the
    /// failed route and to recognize same-model/different-method alternatives.
    pub(super) fn current_route_api_method(&self) -> Option<String> {
        // Session route metadata records a past selection, not necessarily the
        // credential now in use (e.g. after /account or a remote route switch).
        // Prefer the same authoritative credential that drives billing identity
        // so we never offer the active route as its own fallback.
        let provider =
            jcode_provider_core::parse_provider_hint(&self.current_provider_label_for_fallback());
        let credential = if self.is_remote {
            self.remote_resolved_credential
        } else {
            self.provider.active_resolved_credential()
        };
        use jcode_provider_core::{ActiveProvider, ResolvedCredential};
        let account_method = |base: &str| {
            if !self.is_remote
                && let Some(label) = self.provider.oauth_account_label()
            {
                return format!("{base}@{label}");
            }
            if let Some(method) = self.session.route_api_method.as_deref()
                && method
                    .split_once('@')
                    .is_some_and(|(prefix, _)| prefix == base)
                && jcode_provider_core::oauth_account_label(method).is_some()
            {
                return method.to_string();
            }
            let label = if self.is_remote {
                None
            } else if base == "claude-oauth" {
                crate::auth::claude::active_account_label()
            } else {
                crate::auth::codex::active_account_label()
            };
            label
                .map(|label| format!("{base}@{label}"))
                .unwrap_or_else(|| base.to_string())
        };
        match (provider, credential) {
            (Some(ActiveProvider::Claude), Some(ResolvedCredential::Oauth)) => {
                Some(account_method("claude-oauth"))
            }
            (Some(ActiveProvider::Claude), Some(ResolvedCredential::ApiKey)) => {
                Some("claude-api".to_string())
            }
            (Some(ActiveProvider::OpenAI), Some(ResolvedCredential::Oauth)) => {
                Some(account_method("openai-oauth"))
            }
            (Some(ActiveProvider::OpenAI), Some(ResolvedCredential::ApiKey)) => {
                Some("openai-api".to_string())
            }
            _ => self.session.route_api_method.clone(),
        }
    }

    fn current_provider_label_for_fallback(&self) -> String {
        if self.is_remote {
            self.remote_provider_name
                .clone()
                .unwrap_or_else(|| "remote".to_string())
        } else {
            self.provider.name().to_string()
        }
    }

    fn current_model_for_fallback(&self) -> String {
        if self.is_remote {
            self.remote_provider_model
                .clone()
                .unwrap_or_else(|| self.provider.model())
        } else {
            self.provider.model()
        }
    }

    /// Short label for a route, e.g. "claude-sonnet-4 via OAuth (Anthropic)".
    fn describe_route(route: &crate::provider::ModelRoute) -> String {
        let method = crate::provider::ModelRouteApiMethod::parse(&route.api_method).display_label();
        format!("{} via {} ({})", route.model, method, route.provider)
    }

    /// After a provider turn error, compute the next best available route and, if
    /// one exists, arm an interactive offer the user can accept with a keypress to
    /// switch and resend. Returns true when an offer was armed.
    ///
    /// Retryable failures first consider automatic pool-approved account routes
    /// when countdown mode is enabled. If no eligible automatic route exists,
    /// preserve an unrestricted manual offer, including another auth method on
    /// the same provider.
    ///
    /// Works in both local and remote sessions. Remote offers capture the failed
    /// turn's payload (from the pending retry slot) so accepting the offer can
    /// resend it after the server confirms the route switch; callers must arm the
    /// offer *before* clearing the pending retry state.
    pub(super) fn offer_fallback_after_error(&mut self, error: &str) -> bool {
        // Remote sessions resend through the server: capture the failed turn's
        // payload from the pending retry slot while it is still populated.
        let remote_resend = if self.is_remote {
            self.rate_limit_pending_message
                .as_ref()
                .map(|pending| super::FallbackResendPayload {
                    content: pending.content.clone(),
                    images: pending.images.clone(),
                    is_system: pending.is_system,
                    auto_retry: pending.auto_retry,
                    system_reminder: pending.system_reminder.clone(),
                    raw_input: self.last_submitted_input.clone(),
                })
        } else {
            None
        };
        self.offer_fallback_after_error_with_payload(error, remote_resend)
    }

    /// The pool identity is independent of model availability. A failed source
    /// may be cooling down, but its explicit membership still governs consent.
    fn fallback_pool_identity(api_method: &str) -> Option<(String, bool)> {
        if let Some((method, label)) = api_method.split_once('@') {
            let provider = match method {
                "claude-oauth" => "claude",
                "openai-oauth" => "openai",
                _ => return None,
            };
            return Some((
                crate::auth::account_pool::account_key(provider, Some(label)),
                false,
            ));
        }
        match api_method {
            "claude-oauth" | "openai-oauth" | "" => None,
            "claude-api" | "anthropic-api" | "anthropic-api-key" => {
                Some(("anthropic-api".into(), true))
            }
            "openai-api" | "openai-api-key" => Some(("openai-api".into(), true)),
            other => Some((other.into(), true)),
        }
    }

    /// [`Self::offer_fallback_after_error`] with an explicitly captured resend
    /// payload, for callers whose error-cleanup already cleared the pending
    /// retry slot before the offer could be armed.
    pub(super) fn offer_fallback_after_error_with_payload(
        &mut self,
        error: &str,
        remote_resend: Option<super::FallbackResendPayload>,
    ) -> bool {
        // Never compete with the automatic countdown switcher.
        if self.pending_provider_failover.is_some() {
            return false;
        }
        let routes = self.fallback_candidate_routes();
        let current_model = self.current_model_for_fallback();
        let current_provider = self.current_provider_label_for_fallback();
        let current_api_method = self.current_route_api_method().unwrap_or_default();

        // A credential failure (expired OAuth session, failed token refresh,
        // broken API key) breaks every route behind that credential, so widen
        // the exclusion set: offering a sibling model on the same broken
        // credential would fail identically.
        let failover_decision = jcode_provider_core::classify_failover_error_message(error);
        let options = crate::provider::FallbackPickOptions {
            credential_failure: crate::provider::error_looks_like_credential_failure(error)
                || failover_decision.should_mark_provider_unavailable()
                || crate::provider::parse_failover_prompt_message(error).is_some(),
        };
        if !self.is_remote
            && failover_decision.should_mark_provider_unavailable()
            && let Some((method, label)) = current_api_method.split_once('@')
        {
            let provider = match method {
                "claude-oauth" => Some("claude"),
                "openai-oauth" => Some("openai"),
                _ => None,
            };
            if let Some(provider) = provider {
                crate::provider::models::record_provider_unavailable_for_account_label(
                    provider,
                    Some(label),
                    error,
                );
            }
        }
        if routes.is_empty() {
            return false;
        }
        let pool = crate::auth::account_pool::AccountPool::load();
        let source_identity = Self::fallback_pool_identity(&current_api_method);
        let source_is_pooled = source_identity.as_ref().is_some_and(|(key, api_key)| {
            !pool.excluded.contains(key)
                && pool.is_member(
                    key,
                    crate::auth::account_pool::default_member(key, *api_key),
                )
        });
        let automatic = crate::provider::parse_failover_prompt_message(error).is_none()
            && (options.credential_failure || failover_decision.should_failover())
            && source_is_pooled
            && !crate::tui::is_ssh_remote()
            && crate::config::Config::load()
                .provider
                .cross_provider_failover
                == crate::config::CrossProviderFailoverMode::Countdown;
        if automatic {
            // Never let an excluded preferred route hide an approved later one.
            // The pool's explicit account order wins over catalog/model ranking.
            let mut approved: Vec<_> = routes
                .iter()
                .filter(|route| {
                    let selection = crate::provider::RouteSelection::from_model_route(route);
                    let identity = Self::fallback_pool_identity(&route.api_method);
                    route.available
                        && crate::provider::MultiProvider::is_failover_route_pool_approved(
                            &selection.routed_model_spec(),
                        )
                        && identity != source_identity
                })
                .collect();
            approved.sort_by_key(|route| {
                Self::fallback_pool_identity(&route.api_method)
                    .and_then(|(key, _)| pool.members.iter().position(|member| member == &key))
                    .unwrap_or(usize::MAX)
            });
            if let Some(route) = approved.first() {
                let target_route =
                    crate::provider::RouteSelection::from_model_route(route).routed_model_spec();
                self.handle_provider_failover_prompt_with_payload(
                    crate::provider::ProviderFailoverPrompt {
                        from_provider: current_provider.clone(),
                        from_label: current_provider,
                        to_provider: route.provider.clone(),
                        to_label: Self::describe_route(route),
                        target_route: Some(target_route),
                        reason: error.to_string(),
                        estimated_input_chars: 0,
                        estimated_input_tokens: 0,
                    },
                    remote_resend,
                );
                return true;
            }
        }
        // No approved automatic target: preserve unrestricted manual choice.
        let Some(index) = crate::provider::pick_next_fallback_route_with_options(
            &routes,
            &current_model,
            &current_provider,
            &current_api_method,
            options,
        ) else {
            return false;
        };
        let route = routes[index].clone();
        let target_label = Self::describe_route(&route);
        let from_method = crate::provider::ModelRouteApiMethod::parse(&current_api_method);
        let from_label = if current_api_method.is_empty() {
            current_provider.clone()
        } else {
            format!("{} via {}", current_provider, from_method.display_label())
        };

        let remote_resend = if self.is_remote { remote_resend } else { None };

        // The raw error was already shown by the caller just above this offer,
        // so only name the failed route here instead of echoing the error text
        // a second time.
        let key_label = crate::tui::keybind::fallback_switch_key_label();
        self.push_display_message(DisplayMessage::system(format!(
            "↪ {} failed. Fallback available: press {} to switch to {} and resend.",
            from_label, key_label, target_label,
        )));
        self.set_status_notice(format!("Press {} to switch to {}", key_label, route.model));
        self.pending_fallback_offer = Some(super::PendingFallbackOffer {
            selection: crate::provider::RouteSelection::from_model_route(&route),
            target_label,
            from_label,
            remote_resend,
        });
        true
    }

    pub(super) fn clear_pending_fallback_offer(&mut self) {
        self.pending_fallback_offer = None;
        self.pending_fallback_resend = None;
    }

    /// Whether `model` already is the guardrail reroute target
    /// (`claude-opus-4-8`), tolerating case, `[1m]` suffixes, and dated ids.
    fn is_guardrail_reroute_model(model: &str) -> bool {
        let canonical = jcode_provider_core::model_id::canonical(model);
        jcode_provider_core::model_id::strip_date_suffix(&canonical) == GUARDRAIL_REROUTE_MODEL
    }

    /// Pick the best available `claude-opus-4-8` route for a guardrail
    /// reroute: native Anthropic OAuth first, then Anthropic API key, then any
    /// other route (aggregators) in catalog order.
    fn pick_guardrail_reroute_route(
        routes: &[crate::provider::ModelRoute],
    ) -> Option<&crate::provider::ModelRoute> {
        let mut best: Option<(&crate::provider::ModelRoute, u8)> = None;
        for route in routes {
            if !route.available || !Self::is_guardrail_reroute_model(&route.model) {
                continue;
            }
            let tier = match crate::provider::ModelRouteApiMethod::parse(&route.api_method) {
                crate::provider::ModelRouteApiMethod::ClaudeOAuth => 0,
                crate::provider::ModelRouteApiMethod::AnthropicApiKey => 1,
                _ => 2,
            };
            if best.is_none_or(|(_, best_tier)| tier < best_tier) {
                best = Some((route, tier));
            }
        }
        best.map(|(route, _)| route)
    }

    /// After a provider guardrail/refusal stop, arm a one-keypress offer to
    /// reroute to the strongest Anthropic route (`claude-opus-4-8`) and resend
    /// the refused request. Guardrail stops are model-side policy refusals:
    /// retrying the same model usually refuses again, while a stronger model
    /// often handles the same legitimate request. Returns true when an offer
    /// was armed (the offer sets its own status notice).
    pub(super) fn offer_guardrail_reroute(&mut self) -> bool {
        // Never compete with the automatic countdown switcher or an offer
        // already armed by the error path.
        if self.pending_provider_failover.is_some() || self.pending_fallback_offer.is_some() {
            return false;
        }
        let current_model = self.current_model_for_fallback();
        // Already on the reroute target: nothing stronger to offer.
        if Self::is_guardrail_reroute_model(&current_model) {
            return false;
        }
        let routes = self.fallback_candidate_routes();
        let Some(route) = Self::pick_guardrail_reroute_route(&routes).cloned() else {
            return false;
        };

        // Capture the refused turn's payload so accepting the offer can
        // resend it after the route switch (remote sessions resend through
        // the server; local sessions resend via pending_turn).
        let remote_resend = if self.is_remote {
            self.rate_limit_pending_message
                .as_ref()
                .map(|pending| super::FallbackResendPayload {
                    content: pending.content.clone(),
                    images: pending.images.clone(),
                    is_system: pending.is_system,
                    auto_retry: pending.auto_retry,
                    system_reminder: pending.system_reminder.clone(),
                    raw_input: self.last_submitted_input.clone(),
                })
        } else {
            None
        };

        let target_label = Self::describe_route(&route);
        let current_provider = self.current_provider_label_for_fallback();
        let current_api_method = self.current_route_api_method().unwrap_or_default();
        let from_method = crate::provider::ModelRouteApiMethod::parse(&current_api_method);
        let from_label = if current_api_method.is_empty() {
            format!("{} ({})", current_model, current_provider)
        } else {
            format!(
                "{} ({} via {})",
                current_model,
                current_provider,
                from_method.display_label()
            )
        };

        let key_label = crate::tui::keybind::fallback_switch_key_label();
        self.push_display_message(DisplayMessage::system(format!(
            "↪ Reroute available: press {} to switch to {} and resend this request.\n\nGuardrail refusals are model-side; a stronger model often handles the same request (was {}).",
            key_label, target_label, from_label,
        )));
        self.set_status_notice(format!("Press {} to reroute to {}", key_label, route.model));
        self.pending_fallback_offer = Some(super::PendingFallbackOffer {
            selection: crate::provider::RouteSelection::from_model_route(&route),
            target_label,
            from_label,
            remote_resend,
        });
        true
    }

    /// Apply the armed fallback offer: switch to the alternative route and resend
    /// the failed turn. Returns true when an offer was present and consumed.
    ///
    /// Local sessions switch the provider route directly and queue a local
    /// resend. Remote sessions stage the route selection for the remote
    /// dispatcher (which sends `SetRoute` to the server) and stage the failed
    /// payload for resend once the server confirms the switch.
    pub(super) fn apply_pending_fallback_offer(&mut self) -> bool {
        let Some(offer) = self.pending_fallback_offer.take() else {
            return false;
        };

        if self.is_remote {
            self.upstream_provider = None;
            self.status_detail = None;
            // Track the method we are switching to so subsequent fallback picks
            // know the active credential path (remote sessions have no other
            // client-side route bookkeeping).
            self.session.route_api_method = Some(offer.selection.api_method.clone());
            self.pending_route_selection = Some(offer.selection);
            self.pending_fallback_resend = offer.remote_resend;
            self.push_display_message(DisplayMessage::system(format!(
                "↪ Switching to {} and resending (was {}).",
                offer.target_label, offer.from_label,
            )));
            self.set_status_notice(format!("Switching → {} (will retry)", offer.target_label));
            return true;
        }

        match self.provider.set_route_selection(&offer.selection) {
            Ok(()) => {
                let spec = offer.selection.routed_model_spec();
                self.provider_session_id = None;
                self.session.provider_session_id = None;
                self.upstream_provider = None;
                self.status_detail = None;
                self.invalidate_model_picker_cache();
                let active_model = self.provider.model();
                self.update_context_limit_for_model(&active_model, None);
                self.session.provider_key =
                    crate::provider::MultiProvider::session_provider_key_after_model_switch(
                        &spec,
                        self.provider.name(),
                        self.session.provider_key.as_deref(),
                    );
                self.session.model = Some(active_model.clone());
                self.session.route_api_method = Some(offer.selection.api_method.clone());
                if let Err(error) = self.session.save() {
                    self.push_display_message(DisplayMessage::error(format!(
                "Model switched for this session, but the saved route could not be persisted: {error}"
            )));
                }
                self.push_display_message(DisplayMessage::system(format!(
                    "↪ Switched to {} and resending (was {}).",
                    offer.target_label, offer.from_label,
                )));
                self.set_status_notice(format!("Switched → {} (retrying)", active_model));
                self.pending_turn = true;
                true
            }
            Err(error) => {
                self.push_display_message(DisplayMessage::error(format!(
                    "Failed to switch to {}: {}",
                    offer.target_label, error
                )));
                self.set_status_notice("Fallback switch failed");
                true
            }
        }
    }

    pub(super) fn cycle_model(&mut self, direction: i8) {
        let models = self.provider.available_models_for_switching();
        if models.is_empty() {
            self.push_display_message(DisplayMessage::error(
                "Model switching is not available for this provider.",
            ));
            self.set_status_notice("Model switching not available");
            return;
        }

        let current = self.provider.model();
        let current_index = models.iter().position(|m| *m == current).unwrap_or(0);

        let len = models.len();
        let next_index = if direction >= 0 {
            (current_index + 1) % len
        } else {
            (current_index + len - 1) % len
        };
        let next_model = models[next_index].clone();

        match self.provider.set_model(&next_model) {
            Ok(()) => {
                self.finalize_model_switch(&next_model);
                let auth_suffix = self
                    .provider
                    .active_auth_method_label()
                    .map(|method| format!(" (via {})", method))
                    .unwrap_or_default();
                self.push_display_message(DisplayMessage::system(format!(
                    "✓ Switched to model: {}{}",
                    next_model, auth_suffix
                )));
                self.set_status_notice(format!("Model → {}", next_model));
            }
            Err(e) => {
                self.push_display_message(DisplayMessage::error(format!(
                    "Failed to switch model: {}",
                    e
                )));
                self.set_status_notice("Model switch failed");
            }
        }
    }

    pub(super) fn cycle_effort(&mut self, direction: i8) {
        // Remote/self-dev sessions infer the level list from provider+model (the
        // same source the model picker uses), since `self.provider` is a local
        // stand-in. Local sessions read the real provider. This keeps the cycle
        // and the picker consistent (both expose swarm / swarm-deep).
        let efforts = if self.is_remote {
            let (provider_name, provider_model) = self.remote_effort_identity();
            inferred_reasoning_efforts(provider_name.as_deref(), provider_model.as_deref())
        } else {
            self.provider.available_efforts()
        };
        if efforts.is_empty() {
            self.set_status_notice("Reasoning effort not available for this provider");
            return;
        }

        let current = if self.is_remote {
            self.remote_reasoning_effort_hint()
        } else {
            self.provider.reasoning_effort()
        };
        let current_index = current
            .as_ref()
            .and_then(|c| efforts.iter().position(|e| *e == c.as_str()))
            .unwrap_or(efforts.len() - 1); // default to last (highest)

        let len = efforts.len();
        let next_index = if direction > 0 {
            if current_index + 1 >= len {
                current_index // already at max
            } else {
                current_index + 1
            }
        } else if current_index == 0 {
            0 // already at min
        } else {
            current_index - 1
        };

        let next_effort = efforts[next_index];
        if Some(next_effort.to_string()) == current {
            let label = effort_display_label(next_effort);
            self.set_status_notice(format!(
                "Effort: {} (already at {})",
                label,
                if direction > 0 { "max" } else { "min" }
            ));
            return;
        }

        match self.provider.set_reasoning_effort(next_effort) {
            Ok(()) => {
                let label = effort_display_label(next_effort);
                let bar = effort_bar(next_index, len);
                self.set_status_notice(format!("Effort: {} {}", label, bar));
            }
            Err(e) => {
                self.set_status_notice(format!("Effort switch failed: {}", e));
            }
        }
    }

    /// Step the speed tier (Standard -> Fast -> Ultrafast) for a local session.
    pub(super) fn cycle_speed_tier(&mut self, direction: i8) {
        let provider_name = self.provider.name().to_string();
        let model = self.provider.model();
        let ladder = jcode_provider_core::service_tier::speed_tier_ladder(
            Some(&provider_name),
            Some(&model),
        );
        let current = self.provider.service_tier();
        let Some((index, next, at_end)) = jcode_provider_core::service_tier::step_speed_tier(
            &ladder,
            current.as_deref(),
            direction,
        ) else {
            self.set_status_notice("Speed tiers not available for this model");
            return;
        };
        if at_end {
            self.set_status_notice(speed_tier_notice(
                next,
                index,
                ladder.len(),
                Some(direction),
            ));
            return;
        }
        match self.provider.set_service_tier(next) {
            Ok(()) => {
                let applied = self.provider.service_tier();
                let applied =
                    jcode_provider_core::service_tier::canonical_speed_tier(applied.as_deref());
                let index = ladder.iter().position(|t| *t == applied).unwrap_or(index);
                let mut notice = speed_tier_notice(applied, index, ladder.len(), None);
                if self.is_processing {
                    notice.push_str(" (next request)");
                }
                self.set_status_notice(notice);
            }
            Err(e) => self.set_status_notice(format!("Speed switch failed: {}", e)),
        }
    }

    pub(super) fn update_context_limit_for_model(
        &mut self,
        model: &str,
        server_context_window: Option<u64>,
    ) {
        // Prefer the window the server resolved. A remote client's own provider is
        // an inert placeholder with no model catalog, and the static resolver has
        // no entry for a model it does not recognise, so both answer the generic
        // default. Measured: `stealth/space-bunny-alpha@Stealth` has a catalog
        // entry with context_length 1000000, and the panel showed 200000 because
        // the number never crossed the wire.
        let limit = match server_context_window {
            Some(window) => window as usize,
            None if self.is_remote => {
                // The static catalog resolves the models it knows, so keep using
                // it. What must not happen is the final fallback: for a model
                // chosen server-side it is not in the catalog, and the client's
                // own provider is an inert placeholder whose context_window()
                // is the generic 200_000 default. Substituting that over a value
                // the server already reported correctly is how a 1M route went
                // back to displaying 200000. The server is the source of truth
                // for an unknown model, so leave the last reported value alone
                // instead of overwriting it with the default.
                crate::provider::context_limit_for_model_with_provider(
                    model,
                    self.remote_provider_name.as_deref(),
                )
                .unwrap_or(self.context_limit as usize)
            }
            None => self.provider.context_window(),
        };
        self.context_limit = limit as u64;
        self.context_warning_shown = false;

        // Also update compaction manager's budget
        {
            let compaction = self.registry.compaction();
            if let Ok(mut manager) = compaction.try_write() {
                manager.set_budget(limit);
            };
        }
    }

    /// Assign the panel's context window and keep the compaction budget in step
    /// with it.
    ///
    /// Assigning `context_limit` directly updates the panel but skips the
    /// budget sync, so the two can disagree: a correct panel with a stale
    /// compaction trigger that fires about five times too early on a 1M route.
    /// Every direct assignment should come through here.
    pub(super) fn set_context_limit_and_sync_budget(&mut self, limit: usize) {
        self.context_limit = limit as u64;
        self.context_warning_shown = false;
        let compaction = self.registry.compaction();
        if let Ok(mut manager) = compaction.try_write() {
            manager.set_budget(limit);
        };
    }

    pub(super) fn effective_context_tokens_from_usage(
        &self,
        input_tokens: u64,
        cache_read_input_tokens: Option<u64>,
        cache_creation_input_tokens: Option<u64>,
    ) -> u64 {
        let provider_name = if self.is_remote {
            self.remote_provider_name.clone().unwrap_or_default()
        } else {
            self.provider.name().to_string()
        };

        // Shared heuristic (jcode-compaction-core): keeps the sidebar figure
        // consistent with the compaction manager's observed-token feed.
        crate::compaction::effective_context_tokens_from_usage(
            &provider_name,
            input_tokens,
            cache_read_input_tokens,
            cache_creation_input_tokens,
        )
    }

    pub(super) fn current_stream_context_tokens(&self) -> Option<u64> {
        if self.streaming.streaming_input_tokens == 0 {
            return None;
        }
        // After a compaction event the last usage report describes the old,
        // pre-compaction message list; report "unknown" so the UI falls back
        // to the local estimate over the new active messages (issue #441).
        if self.streaming.streaming_context_stale {
            return None;
        }
        Some(self.effective_context_tokens_from_usage(
            self.streaming.streaming_input_tokens,
            self.streaming.streaming_cache_read_tokens,
            self.streaming.streaming_cache_creation_tokens,
        ))
    }

    /// Apply a provider usage report's input/cache counters to the streaming
    /// state, with per-call reset semantics.
    ///
    /// Providers merge `Option` cache counters across repeated snapshots of
    /// the *same* call, but the first report of a *new* call must replace the
    /// counters wholesale: some gateways (e.g. deepseek via opencode-go) only
    /// report `cache_read_input_tokens` on some calls, and a stale cache-read
    /// figure from a previous call would otherwise be added on top of the new
    /// call's input tokens, inflating the context display (issue #441).
    ///
    /// Returns true when any input/cache counter changed.
    pub(super) fn apply_stream_usage_input_report(
        &mut self,
        input_tokens: Option<u64>,
        cache_read_input_tokens: Option<u64>,
        cache_creation_input_tokens: Option<u64>,
    ) -> bool {
        let mut usage_changed = false;
        let has_any_report = input_tokens.is_some()
            || cache_read_input_tokens.is_some()
            || cache_creation_input_tokens.is_some();
        if self.streaming.streaming_usage_call_reset_pending && has_any_report {
            self.streaming.streaming_usage_call_reset_pending = false;
            self.streaming.streaming_cache_read_tokens = None;
            self.streaming.streaming_cache_creation_tokens = None;
            usage_changed = true;
        }
        if let Some(input) = input_tokens {
            self.streaming.streaming_input_tokens = input;
            usage_changed = true;
        }
        if cache_read_input_tokens.is_some() {
            self.streaming.streaming_cache_read_tokens = cache_read_input_tokens;
            usage_changed = true;
        }
        if cache_creation_input_tokens.is_some() {
            self.streaming.streaming_cache_creation_tokens = cache_creation_input_tokens;
            usage_changed = true;
        }
        if usage_changed {
            // Fresh provider usage describes the current active message list
            // again, so the stream-derived context figure is trustworthy.
            self.streaming.streaming_context_stale = false;
        }
        usage_changed
    }

    /// Mark the start of a new provider API call for usage accounting: the
    /// next usage report replaces (rather than merges into) the cache
    /// counters. See [`Self::apply_stream_usage_input_report`].
    pub(super) fn mark_stream_usage_call_boundary(&mut self) {
        self.streaming.streaming_usage_call_reset_pending = true;
    }

    pub(super) fn update_compaction_usage_from_stream(&mut self) {
        if self.is_remote || !self.provider.uses_jcode_compaction() {
            return;
        }
        let Some(tokens) = self.current_stream_context_tokens() else {
            return;
        };
        let compaction = self.registry.compaction();
        if let Ok(mut manager) = compaction.try_write() {
            manager.update_observed_input_tokens(tokens);
        };
    }

    /// Put the prompt that started the failed turn back into the input box so the
    /// user does not lose what they typed when a turn errors out (for example a
    /// "token refresh needed" / auth error). Only restores when the input box is
    /// empty, so we never clobber text the user has already started typing.
    pub(super) fn restore_failed_input_to_box(&mut self) {
        let Some(prompt) = self.last_submitted_input.take() else {
            return;
        };
        if prompt.trim().is_empty() {
            return;
        }
        if !self.input.is_empty() {
            // User already started a new prompt; do not overwrite it.
            return;
        }
        self.input = prompt;
        self.cursor_pos = self.input.len();
        self.reset_tab_completion();
        self.sync_model_picker_preview_from_input();
        self.set_status_notice("Prompt restored to input after error");
    }

    pub(super) fn handle_turn_error(&mut self, error: impl Into<String>) {
        let error = error.into();
        self.refresh_openai_usage_after_quota_error(&error);
        self.last_stream_error = Some(error.clone());

        if let Some(prompt) = crate::provider::parse_failover_prompt_message(&error) {
            self.handle_provider_failover_prompt(prompt);
            self.restore_failed_input_to_box();
            return;
        }
        self.restore_failed_input_to_box();

        let visible_error = Self::compact_provider_failure(&error);
        if is_request_payload_too_large_error(&error) {
            // 413 is a request body-size rejection driven by inline images.
            // Strip oversized images now so a manual resubmit (or auto-poke
            // retry) goes through, and keep auto-poke alive.
            let stripped = self
                .session
                .strip_oversized_images(crate::compaction::PAYLOAD_IMAGE_CHAR_BUDGET);
            if stripped > 0 {
                self.messages.clear();
                self.reseed_compaction_from_provider_messages();
                self.push_display_message(DisplayMessage::error(format!(
                    "Error: {} Dropped {} oversized image(s); you can retry.",
                    visible_error, stripped
                )));
            } else {
                self.push_display_message(DisplayMessage::error(format!(
                    "Error: {} Request body was too large but no inline images could be dropped. Run /fix to try manual recovery.",
                    visible_error
                )));
                super::commands::stop_auto_poke_for_non_retryable_error(self, &error);
                self.stop_overnight_auto_poke_for_non_retryable_error(&error);
            }
            return;
        }

        if is_context_limit_error(&error) {
            let recovery = self.auto_recover_context_limit();
            let should_stop_auto_poke = recovery.is_none();
            let hint = match recovery {
                Some(msg) => format!(" {}", msg),
                None => " Context limit exceeded but auto-recovery failed. Run /fix to try manual recovery.".to_string(),
            };
            self.push_display_message(DisplayMessage::error(format!(
                "Error: {}{}",
                visible_error, hint
            )));
            if should_stop_auto_poke {
                super::commands::stop_auto_poke_for_non_retryable_error(self, &error);
                self.stop_overnight_auto_poke_for_non_retryable_error(&error);
            }
        } else {
            // Offer a one-keypress switch to the next best model/auth-method
            // (e.g. broken API key -> working OAuth login) before giving up. The
            // offer is informational; auto-poke still stops so an unattended loop
            // does not silently keep retrying a path that needs a human decision.
            let offered = self.offer_fallback_after_error(&error);
            if offered && self.pending_provider_failover.is_some() {
                // The countdown notice already includes the compact cause.
            } else if offered {
                self.push_display_message(DisplayMessage::error(format!(
                    "Error: {}",
                    visible_error
                )));
            } else {
                self.push_display_message(DisplayMessage::error(format!(
                    "Error: {} Run /fix to attempt recovery.",
                    visible_error
                )));
            }
            super::commands::stop_auto_poke_for_non_retryable_error(self, &error);
            self.stop_overnight_auto_poke_for_non_retryable_error(&error);
        }
    }

    pub(super) fn auto_recover_context_limit(&mut self) -> Option<String> {
        if self.is_remote || !self.provider.supports_compaction() {
            return None;
        }
        let compaction = self.registry.compaction();
        let mut manager = compaction.try_write().ok()?;
        let mut provider_messages = self.materialized_provider_messages();

        let usage = manager.context_usage_with(&provider_messages);
        if usage > 1.5 {
            let recovery = manager.recover_within_budget(&mut provider_messages);
            if recovery.did_anything() {
                self.messages = provider_messages.clone();
                self.sync_session_compaction_state_from_manager(&manager);
                return Some(format!(
                    "{} You can continue.",
                    recovery.summary_line(usage)
                ));
            }
        }

        let observed_tokens = self
            .current_stream_context_tokens()
            .unwrap_or(self.context_limit);
        manager.update_observed_input_tokens(observed_tokens);

        match manager.force_compact_with(&provider_messages, self.provider.clone()) {
            Ok(()) => Some(
                "⚡ Auto-compaction started - summarizing old messages in background. Retry in a moment."
                    .to_string(),
            ),
            Err(reason) => {
                crate::logging::error(&format!(
                    "[auto_recover] force_compact failed: {}",
                    reason
                ));
                match manager.hard_compact_with(&provider_messages) {
                    Ok(dropped) => {
                        self.sync_session_compaction_state_from_manager(&manager);
                        Some(format!(
                            "⚡ Emergency compaction: dropped {} old messages. You can continue.",
                            dropped
                        ))
                    }
                    Err(_) => {
                        let truncated = manager.emergency_truncate_with(&mut provider_messages);
                        if truncated > 0 {
                            self.messages = provider_messages;
                            Some(format!(
                                "⚡ Emergency truncation: shortened {} large tool result(s) to fit context. You can continue.",
                                truncated
                            ))
                        } else {
                            None
                        }
                    }
                }
            }
        }
    }

    /// Attempt recovery after a provider HTTP 413 "request too large" error by
    /// stripping oversized inline images (oldest-first) from the persisted
    /// transcript, then retrying the turn. Returns true if the retry succeeded.
    ///
    /// This is the byte-size counterpart to `try_auto_compact_and_retry`: 413 is
    /// driven by base64 image payload size, which token-budget compaction
    /// deliberately undercounts, so ordinary compaction would not shrink the
    /// request and the retry would 413 again.
    pub(super) async fn try_recover_payload_too_large_and_retry(
        &mut self,
        terminal: &mut DefaultTerminal,
        event_stream: &mut EventStream,
    ) -> bool {
        if self.is_remote {
            return false;
        }

        let stripped = self
            .session
            .strip_oversized_images(crate::compaction::PAYLOAD_IMAGE_CHAR_BUDGET);
        if stripped == 0 {
            return false;
        }

        // Transcript changed: drop the local materialized scratch copy so the
        // next API call rebuilds from the reduced session, and reseed compaction
        // bookkeeping from the new provider view.
        self.messages.clear();
        self.reseed_compaction_from_provider_messages();

        self.push_display_message(DisplayMessage::system(format!(
            "⚡ Request was too large; dropped {} oversized image(s) and retrying...",
            stripped
        )));

        self.reset_state_for_compaction_retry();
        self.run_compaction_retry_turn(terminal, event_stream).await
    }

    /// Reset session and streaming state so a turn can be safely retried after
    /// an emergency compaction or truncation changed the context.
    ///
    /// Every auto-recovery path used to inline this same ~15-line block; keeping
    /// it in one place means they can no longer drift apart (e.g. one path
    /// forgetting to clear `streaming_cache_creation_tokens`). Callers that hold
    /// the compaction manager lock must `drop` it before calling this.
    fn reset_state_for_compaction_retry(&mut self) {
        self.provider_session_id = None;
        self.session.provider_session_id = None;
        self.context_warning_shown = false;
        self.clear_streaming_render_state();
        self.stream_buffer.clear();
        self.streaming_tool_calls.clear();
        self.streaming.streaming_input_tokens = 0;
        self.streaming.streaming_output_tokens = 0;
        self.streaming.streaming_cache_read_tokens = None;
        self.streaming.streaming_cache_creation_tokens = None;
        self.kv_cache.current_api_usage_recorded = false;
        self.thought_line_inserted = false;
        self.thinking_prefix_emitted = false;
        self.thinking_buffer.clear();
        self.status = ProcessingStatus::Sending;
    }

    /// Run a retry turn after compaction and report whether it succeeded,
    /// clearing the materialized message scratch buffer and recording/handling
    /// any turn error. Shared by every compaction auto-retry path.
    async fn run_compaction_retry_turn(
        &mut self,
        terminal: &mut DefaultTerminal,
        event_stream: &mut EventStream,
    ) -> bool {
        let retry_result = self
            .run_turn_interactive(terminal, event_stream, None)
            .await;
        self.messages.clear();
        match retry_result {
            Ok(()) => {
                self.last_stream_error = None;
                true
            }
            Err(e) => {
                self.handle_turn_error(crate::util::format_error_chain(&e));
                false
            }
        }
    }

    /// Attempt automatic compaction and retry when context limit is exceeded.
    /// Returns true if the retry succeeded.
    pub(super) async fn try_auto_compact_and_retry(
        &mut self,
        terminal: &mut DefaultTerminal,
        event_stream: &mut EventStream,
    ) -> bool {
        if self.is_remote || !self.provider.supports_compaction() {
            return false;
        }

        self.push_display_message(DisplayMessage::system(
            "⚠️ Context limit exceeded - auto-compacting and retrying...".to_string(),
        ));

        // Force the compaction manager to think we're at the limit
        let compaction = self.registry.compaction();
        let compact_started = match compaction.try_write() {
            Ok(mut manager) => {
                let mut provider_messages = self.materialized_provider_messages();
                manager.update_observed_input_tokens(self.context_limit);
                let usage = manager.context_usage_with(&provider_messages);
                if usage > 1.5 {
                    let recovery = manager.recover_within_budget(&mut provider_messages);
                    if recovery.did_anything() {
                        self.messages = provider_messages;
                        self.sync_session_compaction_state_from_manager(&manager);
                        drop(manager);
                        self.reset_state_for_compaction_retry();

                        self.push_display_message(DisplayMessage::system(format!(
                            "{} Retrying...",
                            recovery.summary_line(usage)
                        )));
                        return self.run_compaction_retry_turn(terminal, event_stream).await;
                    }
                    false
                } else {
                    match manager.force_compact_with(&provider_messages, self.provider.clone()) {
                        Ok(()) => true,
                        Err(_) => match manager.hard_compact_with(&provider_messages) {
                            Ok(_) => {
                                self.sync_session_compaction_state_from_manager(&manager);
                                drop(manager);
                                self.reset_state_for_compaction_retry();

                                self.push_display_message(DisplayMessage::system(
                                    "✓ Context compacted (emergency). Retrying...".to_string(),
                                ));
                                return self
                                    .run_compaction_retry_turn(terminal, event_stream)
                                    .await;
                            }
                            Err(_) => false,
                        },
                    }
                }
            }
            Err(_) => false,
        };

        if !compact_started {
            return false;
        }

        // Wait for compaction to finish (up to 60s), reacting to Bus event
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        self.status = ProcessingStatus::RunningTool("compacting context...".to_string());
        let mut bus_rx = Bus::global().subscribe();

        loop {
            if std::time::Instant::now() >= deadline {
                self.push_display_message(DisplayMessage::error(
                    "Auto-compaction timed out.".to_string(),
                ));
                return false;
            }

            // Redraw UI while we wait
            let _ = terminal.draw(|frame| crate::tui::ui::draw(frame, self));

            let compaction = self.registry.compaction();
            let done = if let Ok(mut manager) = compaction.try_write() {
                let provider_messages = self.materialized_provider_messages();
                if let Some(event) = manager.poll_compaction_event_with(&provider_messages) {
                    self.sync_session_compaction_state_from_manager(&manager);
                    self.handle_compaction_event(event);
                    true
                } else {
                    false
                }
            } else {
                false
            };

            if done {
                break;
            }

            // Wait for Bus notification or timeout (instead of sleep-polling)
            let timeout = tokio::time::sleep(Duration::from_secs(1));
            tokio::select! {
                _ = bus_rx.recv() => {}
                _ = timeout => {}
            }
        }

        self.push_display_message(DisplayMessage::system(
            "✓ Context compacted. Retrying...".to_string(),
        ));

        self.reset_state_for_compaction_retry();

        // Retry the turn
        self.run_compaction_retry_turn(terminal, event_stream).await
    }

    pub(super) fn handle_usage_report(&mut self, results: Vec<crate::usage::ProviderUsage>) {
        self.usage_report_refreshing = false;
        self.clear_usage_transient_ui();
        self.upsert_usage_display_card(Self::format_usage_display_card(
            &results,
            false,
            results.len(),
            results.len(),
            false,
        ));
        if results.is_empty() {
            self.set_status_notice("Usage → no connected providers");
        } else {
            self.set_status_notice("Usage → updated");
        }
    }

    pub(super) fn handle_usage_report_progress(
        &mut self,
        progress: crate::usage::ProviderUsageProgress,
    ) {
        self.usage_report_refreshing = !progress.done;
        self.clear_usage_transient_ui();
        self.upsert_usage_display_card(Self::format_usage_display_card(
            &progress.results,
            !progress.done,
            progress.completed,
            progress.total,
            progress.from_cache,
        ));

        if progress.done {
            if progress.results.is_empty() {
                self.set_status_notice("Usage → no connected providers");
            } else {
                self.set_status_notice("Usage → updated");
            }
        } else if progress.from_cache && progress.total == 0 {
            self.set_status_notice("Usage → showing cached data, refreshing");
        } else if progress.total > 0 {
            self.set_status_notice(format!(
                "Usage → refreshing {}/{}",
                progress.completed.min(progress.total),
                progress.total
            ));
        } else {
            self.set_status_notice("Usage → refreshing");
        }
    }

    pub(super) fn push_usage_loading_card(&mut self) {
        self.clear_usage_transient_ui();
        self.push_display_message(DisplayMessage::usage(Self::format_usage_display_card(
            &[],
            true,
            0,
            0,
            false,
        )));
    }

    fn clear_usage_transient_ui(&mut self) {
        self.inline_view_state = None;
        self.usage_overlay = None;
        if self
            .inline_interactive_state
            .as_ref()
            .map(|picker| picker.kind == crate::tui::PickerKind::Usage)
            .unwrap_or(false)
        {
            self.inline_interactive_state = None;
        }
    }

    fn upsert_usage_display_card(&mut self, content: String) {
        let existing = self.display_messages.iter().rposition(|message| {
            message.role == "usage" && message.title.as_deref() == Some("Usage")
        });
        if let Some(idx) = existing {
            self.replace_display_message_title_and_content(idx, Some("Usage".to_string()), content);
        } else {
            self.push_display_message(DisplayMessage::usage(content));
        }
    }

    fn format_usage_display_card(
        reports: &[crate::usage::ProviderUsage],
        refreshing: bool,
        completed: usize,
        total: usize,
        from_cache: bool,
    ) -> String {
        let mut lines = Vec::new();

        if refreshing {
            if total > 0 {
                lines.push(format!(
                    "# Refreshing usage ({}/{})",
                    completed.min(total),
                    total
                ));
            } else if from_cache {
                lines.push("# Showing cached usage while refreshing".to_string());
            } else {
                lines.push("# Refreshing usage".to_string());
            }
            lines.push("Checking connected provider limits...".to_string());
            if !reports.is_empty() {
                lines.push(String::new());
            }
        } else if reports.is_empty() {
            lines.push("# No connected providers".to_string());
            lines.push(
                "Use `/login claude` or `/login openai`, then run `/usage` again.".to_string(),
            );
            return lines.join("\n");
        } else {
            lines.push(format!("# Usage updated · {} source(s)", reports.len()));
            lines.push(String::new());
        }

        // One width for every provider block so bars align across sections,
        // not just within them.
        let name_width = reports
            .iter()
            .flat_map(|provider| provider.limits.iter())
            .map(|limit| limit.name.chars().count())
            .max()
            .unwrap_or(0);

        for (idx, provider) in reports.iter().enumerate() {
            if idx > 0 {
                lines.push(String::new());
            }
            lines.push(Self::format_usage_provider_summary(provider));

            if let Some(error) = &provider.error {
                lines.push(format!("  error: {}", error));
                continue;
            }

            if provider.hard_limit_reached {
                lines.push("  hard limit reached".to_string());
            }

            if provider.limits.is_empty() && provider.extra_info.is_empty() {
                lines.push("  no usage data available".to_string());
                continue;
            }

            for limit in &provider.limits {
                let reset = limit
                    .resets_at
                    .as_deref()
                    .map(crate::usage::format_reset_time)
                    .map(|value| format!(" · resets in {}", value))
                    .unwrap_or_default();
                lines.push(format!(
                    "  {:<width$}: {}{}",
                    limit.name,
                    crate::usage::format_usage_bar(limit.usage_percent, 14),
                    reset,
                    width = name_width
                ));
            }

            for (key, value) in &provider.extra_info {
                lines.push(format!("  {}: {}", key, value));
            }
        }

        lines.join("\n")
    }

    fn format_usage_provider_summary(provider: &crate::usage::ProviderUsage) -> String {
        if provider.error.is_some() {
            return format!("! {} - error", provider.provider_name);
        }
        if provider.hard_limit_reached {
            return format!("! {} - hard limit", provider.provider_name);
        }

        let max_percent = provider
            .limits
            .iter()
            .map(|limit| limit.usage_percent)
            .fold(0.0_f32, f32::max);
        if max_percent >= 90.0 {
            format!("! {} - {:.0}% used", provider.provider_name, max_percent)
        } else if max_percent >= 70.0 {
            format!("~ {} - {:.0}% used", provider.provider_name, max_percent)
        } else if provider.limits.is_empty() && provider.extra_info.is_empty() {
            format!("{} - no data", provider.provider_name)
        } else if max_percent > 0.0 {
            format!("+ {} - {:.0}% used", provider.provider_name, max_percent)
        } else {
            format!("+ {} - available", provider.provider_name)
        }
    }

    pub(super) fn run_fix_command(&mut self) {
        let mut actions: Vec<String> = Vec::new();
        let mut notes: Vec<String> = Vec::new();
        let last_error = self.last_stream_error.clone();
        let context_error = last_error
            .as_deref()
            .map(is_context_limit_error)
            .unwrap_or(false);

        let repaired = self.repair_missing_tool_outputs();
        if repaired > 0 {
            actions.push(format!("Recovered {} missing tool output(s).", repaired));
        }

        if self.summarize_tool_results_missing().is_some() {
            self.recover_session_without_tools();
            actions.push("Created a recovery session with text-only history.".to_string());
        }

        if self.provider_session_id.is_some() || self.session.provider_session_id.is_some() {
            self.provider_session_id = None;
            self.session.provider_session_id = None;
            actions.push("Reset provider session resume state.".to_string());
        }

        if !self.is_remote && self.provider.supports_compaction() {
            let observed_tokens = self
                .current_stream_context_tokens()
                .or_else(|| context_error.then_some(self.context_limit));
            let compaction = self.registry.compaction();
            match compaction.try_write() {
                Ok(mut manager) => {
                    let mut provider_messages = self.materialized_provider_messages();
                    if let Some(tokens) = observed_tokens {
                        manager.update_observed_input_tokens(tokens);
                    }
                    let usage = manager.context_usage_with(&provider_messages);
                    if usage > 1.5 {
                        let recovery = manager.recover_within_budget(&mut provider_messages);
                        match recovery.dropped {
                            Some(dropped) if dropped > 0 => {
                                self.sync_session_compaction_state_from_manager(&manager);
                                actions.push(format!(
                                    "Emergency compaction: dropped {} old messages (context was at {:.0}%).",
                                    dropped,
                                    usage * 100.0
                                ));
                            }
                            Some(_) => {}
                            None => {
                                notes.push("Hard compaction failed.".to_string());
                            }
                        }
                        if recovery.truncated > 0 {
                            self.messages = provider_messages.clone();
                            actions.push(format!(
                                "Emergency truncation: shortened {} large tool result(s) to fit context.",
                                recovery.truncated
                            ));
                        }
                    } else {
                        match manager.force_compact_with(&provider_messages, self.provider.clone())
                        {
                            Ok(()) => {
                                actions.push("Started background context compaction.".to_string())
                            }
                            Err(reason) => match manager.hard_compact_with(&provider_messages) {
                                Ok(dropped) => {
                                    self.sync_session_compaction_state_from_manager(&manager);
                                    actions.push(format!(
                                            "Emergency compaction: dropped {} old messages (normal compaction failed: {}).",
                                            dropped, reason
                                        ));
                                }
                                Err(hard_reason) => {
                                    notes.push(format!(
                                        "Compaction not started: {}. Emergency fallback: {}",
                                        reason, hard_reason
                                    ));
                                }
                            },
                        }
                    }
                }
                Err(_) => notes.push("Could not access compaction manager (busy).".to_string()),
            };
        } else {
            notes.push("Compaction is unavailable for this provider.".to_string());
        }

        self.context_warning_shown = false;
        self.last_stream_error = None;
        self.set_status_notice("Fix applied");

        let mut content = String::from("Fix Results:\n");
        if actions.is_empty() {
            content.push_str("• No structural issues detected.\n");
        } else {
            for action in &actions {
                content.push_str(&format!("• {}\n", action));
            }
        }
        for note in &notes {
            content.push_str(&format!("• {}\n", note));
        }
        if let Some(last_error) = &last_error {
            content.push_str(&format!(
                "\nLast error: {}",
                crate::util::truncate_str(last_error, 200)
            ));
        }
        self.push_display_message(DisplayMessage::system(content));
    }
}

impl App {
    pub(super) fn handle_model_refresh_completed(
        &mut self,
        completed: crate::bus::ModelRefreshCompleted,
    ) {
        let completion_matches_active =
            self.active_client_session_id() == Some(completed.session_id.as_str());
        let completion_matches_local = completed.session_id == self.session.id;
        if !completion_matches_active && !completion_matches_local {
            return;
        }
        match completed.result {
            Ok(summary) => {
                self.invalidate_model_picker_cache();
                self.finish_background_task(
                    "refresh-model-list".to_string(),
                    "Model list refresh".to_string(),
                    crate::tui::BackgroundTaskRowStatus::Completed,
                );
                self.push_display_message(DisplayMessage::system(format_model_refresh_summary(
                    &summary,
                )));
                self.set_status_notice(format!(
                    "Model list refreshed: +{} models, +{} routes, ~{} changed",
                    summary.models_added, summary.routes_added, summary.routes_changed
                ));
            }
            Err(error) => {
                self.finish_background_task(
                    "refresh-model-list".to_string(),
                    "Model list refresh".to_string(),
                    crate::tui::BackgroundTaskRowStatus::Failed,
                );
                self.push_display_message(DisplayMessage::error(format!(
                    "Failed to refresh model list: {}",
                    error
                )));
                self.set_status_notice("Model list refresh failed");
            }
        }
    }
}

#[path = "model_context_messages.rs"]
mod messages;
pub(super) use messages::*;
