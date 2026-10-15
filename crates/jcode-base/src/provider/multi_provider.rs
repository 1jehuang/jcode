use super::*;
use anyhow::Result;

impl MultiProvider {
    /// Check the exact billing identity again when a countdown expires. A
    /// removed login or a newly excluded account must never be selected.
    pub fn is_failover_route_pool_approved(route: &str) -> bool {
        let Some((method, model)) = route.split_once(':') else {
            return false;
        };
        let provider = match method.split('@').next().unwrap_or(method) {
            "openai" | "openai-oauth" | "openai-api" | "openai-api-key" => "openai",
            "claude" | "claude-oauth" | "claude-api" | "anthropic-api" | "anthropic-api-key" => {
                "claude"
            }
            other => other,
        };
        if !models::is_agent_model_for_failover(model, provider) {
            return false;
        }
        let key = if let Some((method, label)) = method.split_once('@') {
            let provider = match method {
                "openai-oauth" => "openai",
                "claude-oauth" => "claude",
                _ => return false,
            };
            let labels = saved_account_labels(provider);
            if !labels.iter().any(|saved| saved == label)
                || !saved_account_can_serve_model(provider, label, model)
                || models::provider_unavailability_detail_for_account_label(provider, Some(label))
                    .is_some()
            {
                return false;
            }
            crate::auth::account_pool::account_key(provider, Some(label))
        } else {
            match method {
                "openai-api" | "openai-api-key" => "openai-api".to_owned(),
                "claude-api" | "anthropic-api" | "anthropic-api-key" => "anthropic-api".to_owned(),
                // Unqualified OAuth routes do not identify a billing account.
                "openai" | "claude" | "openai-oauth" | "claude-oauth" => return false,
                other => other.to_owned(),
            }
        };
        if !key.contains(':')
            && models::provider_unavailability_detail_for_account_label(&key, None).is_some()
        {
            return false;
        }
        let api_key = if key.ends_with("-api") {
            true
        } else if key.contains(':') {
            false
        } else {
            let Some(login) = crate::provider_catalog::login_providers()
                .iter()
                .find(|login| login.id == key)
            else {
                return false;
            };
            login.auth_kind == crate::provider_catalog::LoginProviderAuthKind::ApiKey
        };
        let default = crate::auth::account_pool::default_member(&key, api_key);
        let pool = crate::auth::account_pool::AccountPool::load();
        !pool.excluded.contains(&key) && pool.is_member(&key, default)
    }

    fn account_failover_prompt(
        &self,
        provider: ActiveProvider,
        current: &str,
        reason: &str,
        messages: &[Message],
        tools: &[ToolDefinition],
        mode: CompletionMode<'_>,
    ) -> Option<ProviderFailoverPrompt> {
        let key = Self::provider_key(provider);
        let labels = saved_account_labels(key);
        // Missing pins cannot heal to a different account. Neither may an
        // explicitly manual account silently enroll itself in the pool.
        let pool = crate::auth::account_pool::AccountPool::load();
        if !labels.iter().any(|label| label == current)
            || !pool.is_member(
                &crate::auth::account_pool::account_key(key, Some(current)),
                true,
            )
        {
            return None;
        }
        let (chars, tokens) = Self::estimate_request_input(messages, tools, mode);
        self.cross_provider_pool_prompt(provider, provider, reason.to_owned(), chars, tokens)
    }

    pub(super) fn cross_provider_pool_prompt(
        &self,
        from: ActiveProvider,
        _to: ActiveProvider,
        reason: String,
        chars: usize,
        tokens: usize,
    ) -> Option<ProviderFailoverPrompt> {
        let from_provider = Self::provider_key(from);
        let from_label = self.selected_account_for_provider(from).or_else(|| {
            self.provider_uses_oauth_accounts(from)
                .then(|| active_account_label_for_provider(from))
                .flatten()
        });
        let source_key = match from {
            ActiveProvider::OpenAI if from_label.is_none() => "openai-api".to_owned(),
            ActiveProvider::Claude if from_label.is_none() => "anthropic-api".to_owned(),
            _ => crate::auth::account_pool::account_key(from_provider, from_label.as_deref()),
        };
        let pool = crate::auth::account_pool::AccountPool::load();
        let source_default = !source_key.ends_with("-api");
        if pool.excluded.contains(&source_key) || !pool.is_member(&source_key, source_default) {
            return None;
        }
        let mut candidates: Vec<(String, ActiveProvider, String, String)> = Vec::new();
        // Preserve the current model when rotating to another saved account.
        // This path never consults or mutates the global active login.
        let context_only = jcode_provider_core::classify_failover_error_message(&reason)
            == jcode_provider_core::FailoverDecision::RetryNextProvider;
        let required_context = if context_only {
            tokens.max(self.context_window().saturating_add(1))
        } else {
            tokens
        };
        if let Some(current) = from_label
            .as_deref()
            .filter(|_| !context_only && same_provider_account_failover_enabled())
        {
            let method = if from == ActiveProvider::OpenAI {
                "openai-oauth"
            } else {
                "claude-oauth"
            };
            for label in saved_account_labels(from_provider)
                .into_iter()
                .filter(|label| label != current)
            {
                let spec = format!("{method}@{label}:{}", self.model());
                if Self::is_failover_route_pool_approved(&spec)
                    && Self::failover_model_fits(&self.model(), from_provider, required_context)
                {
                    candidates.push((
                        crate::auth::account_pool::account_key(from_provider, Some(&label)),
                        from,
                        spec,
                        format!("{} ({label})", Self::provider_label(from)),
                    ));
                }
            }
        }
        for route in self
            .model_routes()
            .into_iter()
            .filter(|route| route.available)
        {
            let selection = jcode_provider_core::RouteSelection::from_model_route(&route);
            let spec = selection.routed_model_spec();
            if !Self::is_failover_route_pool_approved(&spec) {
                continue;
            }
            let target_provider = match &selection.runtime_key {
                jcode_provider_core::RuntimeKey::ClaudeOAuth
                | jcode_provider_core::RuntimeKey::AnthropicApiKey => ActiveProvider::Claude,
                jcode_provider_core::RuntimeKey::OpenAIOAuth
                | jcode_provider_core::RuntimeKey::OpenAIApiKey => ActiveProvider::OpenAI,
                jcode_provider_core::RuntimeKey::Copilot => ActiveProvider::Copilot,
                jcode_provider_core::RuntimeKey::Gemini
                | jcode_provider_core::RuntimeKey::CodeAssistOAuth => ActiveProvider::Gemini,
                jcode_provider_core::RuntimeKey::Cursor => ActiveProvider::Cursor,
                jcode_provider_core::RuntimeKey::Bedrock => ActiveProvider::Bedrock,
                jcode_provider_core::RuntimeKey::Antigravity => ActiveProvider::Antigravity,
                jcode_provider_core::RuntimeKey::OpenRouter => ActiveProvider::OpenRouter,
                _ => continue,
            };
            if !Self::failover_model_fits(
                &route.model,
                Self::provider_key(target_provider),
                required_context,
            ) || route.detail.contains("no tools")
            {
                continue;
            }
            if target_provider == from
                && (context_only || !same_provider_account_failover_enabled())
            {
                continue;
            }
            let label = jcode_provider_core::oauth_account_label(&route.api_method);
            let target_key = match target_provider {
                ActiveProvider::OpenAI if label.is_none() => "openai-api".to_owned(),
                ActiveProvider::Claude if label.is_none() => "anthropic-api".to_owned(),
                _ => crate::auth::account_pool::account_key(
                    Self::provider_key(target_provider),
                    label,
                ),
            };
            let display = label
                .map(|label| format!("{} ({label})", Self::provider_label(target_provider)))
                .unwrap_or_else(|| {
                    if target_key.ends_with("-api") {
                        format!("{} API key", Self::provider_label(target_provider))
                    } else {
                        Self::provider_label(target_provider).to_owned()
                    }
                });
            candidates.push((target_key, target_provider, spec, display));
        }
        let mut keys = vec![(source_key.clone(), true)];
        for (key, _, _, _) in &candidates {
            if !keys.iter().any(|(existing, _)| existing == key) {
                keys.push((key.clone(), true));
            }
        }
        let pool = crate::auth::account_pool::AccountPool::load();
        let mut order = pool.partition(&keys).0;
        if let Some(position) = order.iter().position(|key| *key == source_key) {
            order.rotate_left(position + 1);
        }
        let configured_model = crate::config::Config::load().provider.default_model;
        let target = order
            .into_iter()
            .filter(|key| *key != source_key)
            .find_map(|key| {
                let group: Vec<_> = candidates
                    .iter()
                    .filter(|(candidate, _, _, _)| candidate.as_str() == key)
                    .collect();
                let first = *group.first()?;
                let current_model = self.model();
                if !context_only
                    && first.1 == from
                    && let Some(preserved) = group
                        .iter()
                        .find(|candidate| candidate_model(candidate) == current_model)
                {
                    return Some(*preserved);
                }
                if let Some(configured) = configured_model.as_deref()
                    && let Some(preferred) = group
                        .iter()
                        .find(|candidate| candidate_matches_configured_model(candidate, configured))
                {
                    return Some(*preferred);
                }
                let models: Vec<&str> = group
                    .iter()
                    .map(|candidate| candidate_model(candidate))
                    .collect();
                if let Some(preferred) = crate::auth::lifecycle::preferred_model_for_provider(
                    Self::provider_key(first.1),
                    &models,
                ) && let Some(flagship) = group
                    .iter()
                    .find(|candidate| candidate_model(candidate) == preferred)
                {
                    return Some(*flagship);
                }
                // Capacity is eligibility, not quality. Never pick an unknown
                // model by maximum context, catalog position or lexical order.
                None
            })?;
        let mut prompt = self.build_failover_prompt(from, target.1, reason, chars, tokens);
        prompt.target_route = Some(target.2.clone());
        prompt.to_label = target.3.clone();
        if let Some(label) = from_label {
            prompt.from_label = format!("{} ({label})", Self::provider_label(from));
        }
        Some(prompt)
    }

    pub(super) async fn try_same_provider_account_failover(
        &self,
        provider: ActiveProvider,
        messages: &[Message],
        tools: &[ToolDefinition],
        mode: CompletionMode<'_>,
        initial_reason: &str,
        _notes: &mut Vec<String>,
    ) -> Result<Option<EventStream>> {
        if !self.provider_uses_oauth_accounts(provider) {
            return Ok(None);
        }
        let current = self
            .selected_account_for_provider(provider)
            .or_else(|| active_account_label_for_provider(provider));
        if let Some(prompt) = current.as_deref().and_then(|current| {
            self.account_failover_prompt(provider, current, initial_reason, messages, tools, mode)
        }) {
            // No alternate request, runtime mutation or global override before
            // the caller's cancellable countdown has accepted this exact route.
            return Err(anyhow!(prompt.to_error_message()));
        }
        if let Some(current) = self.selected_account_for_provider(provider) {
            let key = Self::provider_key(provider);
            let pool = crate::auth::account_pool::AccountPool::load();
            if saved_account_labels(key).contains(&current)
                && pool.is_member(
                    &crate::auth::account_pool::account_key(key, Some(&current)),
                    true,
                )
            {
                let (chars, tokens) = Self::estimate_request_input(messages, tools, mode);
                for target in Self::fallback_sequence(provider)
                    .into_iter()
                    .filter(|target| *target != provider)
                {
                    if let Some(prompt) = self.cross_provider_pool_prompt(
                        provider,
                        target,
                        initial_reason.to_owned(),
                        chars,
                        tokens,
                    ) {
                        return Err(anyhow!(prompt.to_error_message()));
                    }
                }
            }
        }
        Ok(None)
    }
}

fn candidate_model(candidate: &(String, ActiveProvider, String, String)) -> &str {
    candidate
        .2
        .split_once(':')
        .map(|(_, model)| model)
        .unwrap_or("")
}

fn candidate_matches_configured_model(
    candidate: &(String, ActiveProvider, String, String),
    configured: &str,
) -> bool {
    let configured = configured.trim();
    if candidate_model(candidate) == configured {
        return true;
    }
    let Some((method, model)) = configured.split_once(':') else {
        return false;
    };
    let Some((candidate_method, candidate_model)) = candidate.2.split_once(':') else {
        return false;
    };
    // A routed default is a credential preference too. OAuth/API modes must
    // match, and a saved-account pin may not leak to another billing identity.
    jcode_provider_core::ModelRouteApiMethod::parse(method)
        == jcode_provider_core::ModelRouteApiMethod::parse(candidate_method)
        && jcode_provider_core::oauth_account_label(method).is_none_or(|label| {
            jcode_provider_core::oauth_account_label(candidate_method) == Some(label)
        })
        && candidate_model == model
}

fn saved_account_can_serve_model(provider: &str, label: &str, model: &str) -> bool {
    let now = chrono::Utc::now().timestamp_millis();
    let (access_present, refresh, expired, known_model) = match provider {
        "openai" => {
            let Ok(credentials) = crate::auth::codex::load_credentials_for_account(label) else {
                return false;
            };
            let known = models::known_openai_model_ids_for_scope(label)
                .iter()
                .any(|known| known == model)
                && models::model_availability_for_scope(label, model).state
                    != models::AccountModelAvailabilityState::Unavailable
                && !jcode_provider_core::is_openai_api_only_pro_model(model);
            (
                !credentials.access_token.is_empty(),
                credentials.refresh_token,
                credentials.expires_at.is_some_and(|expires| expires <= now),
                known,
            )
        }
        "claude" => {
            let Ok(credentials) = crate::auth::claude::load_credentials_for_account(label) else {
                return false;
            };
            let scope = format!("oauth::{label}");
            let cached = models::cached_anthropic_model_ids_for_scope(&scope).is_some();
            let pro = credentials
                .subscription_type
                .as_deref()
                .is_some_and(|plan| plan.eq_ignore_ascii_case("pro"));
            let known = models::known_anthropic_model_ids_for_scope(&scope)
                .iter()
                .any(|known| known == model)
                && !(model.contains("opus") && pro && !cached)
                && (!model.ends_with("[1m]") || cached);
            (
                !credentials.access_token.is_empty(),
                credentials.refresh_token,
                credentials.expires_at <= now,
                known,
            )
        }
        _ => return false,
    };
    known_model
        && !((!access_present || expired) && refresh.is_empty())
        && (refresh.is_empty()
            || !crate::auth::refresh_state::refresh_token_is_known_rejected(provider, &refresh))
}

fn saved_account_labels(provider: &str) -> Vec<String> {
    match provider {
        "openai" => match crate::auth::codex::list_accounts() {
            Ok(accounts) => accounts.into_iter().map(|account| account.label).collect(),
            Err(error) => {
                crate::logging::warn(&format!("Cannot list saved openai accounts: {error}"));
                Vec::new()
            }
        },
        "claude" => match crate::auth::claude::list_accounts() {
            Ok(accounts) => accounts.into_iter().map(|account| account.label).collect(),
            Err(error) => {
                crate::logging::warn(&format!("Cannot list saved claude accounts: {error}"));
                Vec::new()
            }
        },
        _ => Vec::new(),
    }
}
