//! Per-session same-provider account failover.
//!
//! When the active Claude or OpenAI account of *this session* runs out of
//! subscription usage, move only this session to the next stored account and
//! resend the same request. Other sessions (their pins) and the stored
//! default account are never touched: the move is `set_account_pin` on this
//! provider instance.
//!
//! The runtimes report the usage-limit 429 inside the stream, so the stream
//! is peeked before anything reaches the consumer. A resend only happens when
//! the request failed before any output.

use super::account_failover::{
    account_exhausted, account_failover_return_home_config, account_kind, account_kind_display,
    account_label_prefix, account_resets_at, account_rotation, all_accounts_exhausted_reason,
    default_account_label, format_reset_clock, pin_for_label, record_account_exhausted,
    same_provider_account_failover_config, stored_accounts,
};
use super::stream_peek::{Peeked, peek_before_output};
use super::*;
use jcode_provider_core::{AccountPin, AccountProviderKind};

/// Outcome of a completion on a multi-account provider with failover on.
pub(super) enum AccountAttempt {
    /// A stream to hand to the consumer (success, or an error that is not
    /// account exhaustion, replayed untouched).
    Stream(EventStream),
    /// The request failed synchronously (before a stream existed).
    Error(anyhow::Error),
    /// This account and every alternative are out of usage (or failed).
    /// The reason names the earliest reset.
    Exhausted(String),
}

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
            let kind = if provider == "openai" {
                AccountProviderKind::OpenAi
            } else {
                AccountProviderKind::Claude
            };
            if !labels.iter().any(|saved| saved == label)
                || account_exhausted(kind, label).is_some()
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

    pub(super) fn cross_provider_pool_prompt(
        &self,
        from: ActiveProvider,
        _to: ActiveProvider,
        reason: String,
        chars: usize,
        tokens: usize,
    ) -> Option<ProviderFailoverPrompt> {
        let from_provider = Self::provider_key(from);
        // This window's account (its pin, else the login it resolved to).
        let from_label = self.selected_account_for_provider(from).or_else(|| {
            self.provider_uses_oauth_accounts(from)
                .then(|| account_kind(from).and_then(|kind| self.session_account_label(kind)))
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
            .filter(|_| !context_only && self.account_failover_enabled())
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
            if target_provider == from && (context_only || !self.account_failover_enabled()) {
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

    /// Effective failover setting: the session toggle, else config.
    pub(super) fn account_failover_enabled(&self) -> bool {
        self.account_failover
            .enabled()
            .unwrap_or_else(same_provider_account_failover_config)
    }

    /// Label this session uses for `kind`: its pin, else the label the last
    /// request resolved to, else the stored default.
    pub(super) fn session_account_label(&self, kind: AccountProviderKind) -> Option<String> {
        let explicit = match kind {
            AccountProviderKind::Claude => {
                self.selected_account_for_provider(ActiveProvider::Claude)
            }
            AccountProviderKind::OpenAi => {
                self.selected_account_for_provider(ActiveProvider::OpenAI)
            }
            AccountProviderKind::ClaudeCode => None,
        };
        explicit
            .or_else(|| self.account_pin(kind).map(|pin| pin.label))
            .or_else(|| self.resolved_account_label(kind))
            .or_else(|| default_account_label(kind))
    }

    /// Stored labels of `kind` when failover can move this session: enabled
    /// for this session, and the pool has another member to rotate to.
    pub(super) fn account_failover_labels(
        &self,
        provider: ActiveProvider,
    ) -> Option<(AccountProviderKind, String, Vec<String>)> {
        let kind = account_kind(provider)?;
        if !self.account_failover_enabled() {
            return None;
        }
        let labels: Vec<String> = stored_accounts(kind)
            .into_iter()
            .map(|(label, _)| label)
            .collect();
        if labels.len() < 2 {
            return None;
        }
        let current = self.session_account_label(kind)?;
        let pool = crate::auth::account_pool::AccountPool::load();
        // Only auto-switch pool members rotate, on both ends: an account the
        // user moved out of the pool stays manual and never moves on its own.
        let current_key =
            crate::auth::account_pool::account_key(account_label_prefix(kind), Some(&current));
        if pool.excluded.contains(&current_key) || !pool.is_member(&current_key, true) {
            return None;
        }
        let has_alternative = !pool
            .rotation(account_label_prefix(kind), Some(&current), &labels)
            .is_empty();
        has_alternative.then_some((kind, current, labels))
    }

    /// True when this session's account for `provider` is known to be out of
    /// usage: recorded by a usage-limit response (any session), or, for an
    /// unpinned Claude session, the default account's usage probe.
    pub(super) fn session_account_exhausted(&self, provider: ActiveProvider) -> Option<String> {
        let kind = account_kind(provider)?;
        let label = self.session_account_label(kind)?;
        if account_exhausted(kind, &label).is_none()
            && let Some(usage_kind) = super::account_failover::multi_account_provider_kind(provider)
            && let Some(resets_at) =
                crate::usage::account_label_usage_exhausted_sync(usage_kind, &label)
        {
            // The label's cached usage (5h and weekly windows spent) says it
            // is out: remember it like a usage-limit response would.
            record_account_exhausted(kind, &label, resets_at);
        }
        if let Some(until) = account_exhausted(kind, &label) {
            return Some(format!(
                "{label} is out of usage (resets {})",
                format_reset_clock(until)
            ));
        }
        if provider == ActiveProvider::Claude
            && self.account_pin(kind).is_none()
            && self.is_claude_usage_exhausted()
        {
            return Some(super::account_failover::usage_exhausted_reason(provider));
        }
        None
    }

    /// Complete on the active Claude/OpenAI account with same-provider
    /// account failover. Only called when `account_failover_labels` allows it.
    /// `known_unavailable` is set when this session's account is already
    /// marked unavailable, so no request is sent to it.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn complete_with_account_failover(
        &self,
        provider: ActiveProvider,
        (kind, current, labels): (AccountProviderKind, String, Vec<String>),
        known_unavailable: Option<String>,
        messages: &[Message],
        tools: &[ToolDefinition],
        mode: CompletionMode<'_>,
        resume_session_id: Option<&str>,
        notes: &mut Vec<String>,
    ) -> AccountAttempt {
        let initial_reason = match known_unavailable
            .or_else(|| self.session_account_exhausted(provider))
        {
            // Known to be out: don't send a doomed request.
            Some(reason) => reason,
            None => {
                let peeked = match self
                    .complete_candidate(provider, messages, tools, mode, resume_session_id)
                    .await
                {
                    Ok(stream) => peek_before_output(stream).await,
                    Err(err) => {
                        let text = format!("{err:#}");
                        match jcode_provider_core::classify_account_usage_limit(&text) {
                            Some(limit) => Peeked::Failed {
                                error: err,
                                out_of_credit: false,
                                usage_limit: Some(limit),
                                replay: Box::pin(futures::stream::empty()),
                            },
                            None if account_local_failure(&text, false) => Peeked::Failed {
                                error: err,
                                out_of_credit: false,
                                usage_limit: None,
                                replay: Box::pin(futures::stream::empty()),
                            },
                            None => return AccountAttempt::Error(err),
                        }
                    }
                };
                match peeked {
                    Peeked::Stream(stream) => return AccountAttempt::Stream(stream),
                    Peeked::Failed {
                        usage_limit: Some(limit),
                        error,
                        ..
                    } => {
                        // Label that actually served the failed request.
                        let label = self.resolved_account_label(kind).unwrap_or(current.clone());
                        record_account_exhausted(kind, &label, limit.resets_at);
                        crate::logging::warn(&format!(
                            "{} account {} is out of usage{}: {}",
                            account_kind_display(kind),
                            label,
                            mode.log_suffix(),
                            Self::summarize_error(&error)
                        ));
                        Self::summarize_error(&error)
                    }
                    // A rejected login or an out-of-credit account before any
                    // output moves this window too. A short 429 does not: the
                    // runtime owns those retries and the account recovers.
                    Peeked::Failed {
                        error,
                        out_of_credit,
                        ..
                    } if account_local_failure(&format!("{error:#}"), out_of_credit) => {
                        let label = self.resolved_account_label(kind).unwrap_or(current.clone());
                        let summary = Self::summarize_error(&error);
                        record_provider_unavailable_for_account_label(
                            Self::provider_key(provider),
                            Some(&label),
                            &summary,
                        );
                        summary
                    }
                    // Anything else: the normal error paths own it.
                    Peeked::Failed { replay, .. } => return AccountAttempt::Stream(replay),
                }
            }
        };
        notes.push(format!(
            "{} account {}: {}",
            Self::provider_label(provider),
            current,
            initial_reason
        ));

        match self
            .try_same_provider_account_failover(
                provider,
                kind,
                &current,
                &labels,
                messages,
                tools,
                mode,
                resume_session_id,
                notes,
            )
            .await
        {
            Some(stream) => AccountAttempt::Stream(stream),
            None => AccountAttempt::Exhausted(self.accounts_exhausted_reason(
                kind,
                &labels,
                &initial_reason,
            )),
        }
    }

    /// Reason when no account could take over. Names the earliest reset when
    /// every account is known to be out of usage.
    pub(super) fn accounts_exhausted_reason(
        &self,
        kind: AccountProviderKind,
        labels: &[String],
        initial_reason: &str,
    ) -> String {
        if labels
            .iter()
            .all(|label| account_exhausted(kind, label).is_some())
        {
            all_accounts_exhausted_reason(kind, labels)
        } else {
            format!(
                "{initial_reason}. No other {} account could take over",
                account_kind_display(kind)
            )
        }
    }

    /// Move this session to the next account in the rotation that serves the
    /// request. Only this provider instance is re-pinned. Returns `None` (and
    /// restores the original pin) when no account could take over.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn try_same_provider_account_failover(
        &self,
        provider: ActiveProvider,
        kind: AccountProviderKind,
        current: &str,
        labels: &[String],
        messages: &[Message],
        tools: &[ToolDefinition],
        mode: CompletionMode<'_>,
        resume_session_id: Option<&str>,
        notes: &mut Vec<String>,
    ) -> Option<EventStream> {
        let original_pin = self.account_pin(kind);
        let provider_key = Self::provider_key(provider);
        let provider_label = Self::provider_label(provider);

        // Hard cap: each stored label is tried at most once per call (the
        // current one was already tried or known out), and never more
        // attempts than there are stored labels. No second pass.
        let mut tried: std::collections::HashSet<String> =
            std::iter::once(current.to_string()).collect();
        let max_attempts = labels.len().saturating_sub(1);
        let mut attempts = 0usize;
        if kind == AccountProviderKind::OpenAi {
            for label in labels.iter().filter(|label| label.as_str() != current) {
                if account_exhausted(kind, label).is_some() {
                    crate::usage::revalidate_openai_account_exhaustion(label).await;
                }
            }
        }
        for label in account_rotation(kind, Some(current), labels) {
            if attempts >= max_attempts || !tried.insert(label.clone()) {
                continue;
            }
            // Another session may have marked it out since the rotation was built.
            if account_exhausted(kind, &label).is_some() {
                continue;
            }
            attempts += 1;
            crate::logging::info(&format!(
                "Same-provider failover{}: moving this session's {} request to account '{}'",
                mode.log_suffix(),
                provider_label,
                label
            ));
            if let Err(err) = self.set_account_pin(kind, Some(pin_for_label(kind, &label))) {
                notes.push(format!("{provider_label} account {label}: {err}"));
                continue;
            }
            clear_provider_unavailable_for_label(provider_key, &label);

            let failure = match self
                .complete_candidate(provider, messages, tools, mode, resume_session_id)
                .await
            {
                Ok(stream) => match peek_before_output(stream).await {
                    Peeked::Stream(stream) => {
                        self.record_account_move(kind, original_pin.as_ref(), current, &label);
                        return Some(stream);
                    }
                    Peeked::Failed {
                        usage_limit: Some(limit),
                        error,
                        ..
                    } => {
                        record_account_exhausted(kind, &label, limit.resets_at);
                        error
                    }
                    Peeked::Failed { error, .. } => {
                        if Self::classify_failover_error(&error).should_mark_provider_unavailable()
                        {
                            record_provider_unavailable_for_account_label(
                                provider_key,
                                Some(&label),
                                &Self::summarize_error(&error),
                            );
                        }
                        error
                    }
                },
                Err(error) => {
                    if Self::classify_failover_error(&error).should_mark_provider_unavailable() {
                        record_provider_unavailable_for_account_label(
                            provider_key,
                            Some(&label),
                            &Self::summarize_error(&error),
                        );
                    }
                    error
                }
            };
            let summary = Self::summarize_error(&failure);
            crate::logging::info(&format!(
                "Same-provider account {} failed{}: {}",
                label,
                mode.log_suffix(),
                summary
            ));
            notes.push(format!("{provider_label} account {label}: {summary}"));
        }

        if let Err(err) = self.set_account_pin(kind, original_pin) {
            crate::logging::warn(&format!(
                "Failed to restore {provider_label} account pin after failover: {err}"
            ));
        }
        crate::logging::info(&format!(
            "Same-provider failover{} found no {} account that can serve this request",
            mode.log_suffix(),
            provider_label
        ));
        None
    }

    /// After a successful move: remember where to return, and tell the user.
    fn record_account_move(
        &self,
        kind: AccountProviderKind,
        original_pin: Option<&AccountPin>,
        from: &str,
        to: &str,
    ) {
        if self.account_failover.home(kind).is_none() {
            let home = original_pin
                .cloned()
                .unwrap_or_else(|| pin_for_label(kind, from));
            self.account_failover.set_home(kind, Some(home));
        }
        let reset = match account_resets_at(kind, from) {
            Some(at) => format!("{from} resets {}", format_reset_clock(at)),
            None => format!("{from} is out of usage"),
        };
        self.startup_notices
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(format!(
                "⚡ This window moved from {from} to {to} ({reset})"
            ));
    }

    /// Turn-start check: re-pin the preferred account once its recorded
    /// usage limit has reset. Never runs mid-turn (the agent calls it).
    pub(super) fn return_home_if_reset(&self) -> Vec<AccountProviderKind> {
        if !account_failover_return_home_config() {
            return Vec::new();
        }
        let mut moved = Vec::new();
        for kind in AccountProviderKind::ALL {
            let Some(home) = self.account_failover.home(kind) else {
                continue;
            };
            if self.session_account_label(kind).as_deref() == Some(home.label.as_str()) {
                self.account_failover.set_home(kind, None);
                continue;
            }
            if account_exhausted(kind, &home.label).is_some() {
                continue;
            }
            match self.set_account_pin(kind, Some(home.clone())) {
                Ok(()) => {
                    self.account_failover.set_home(kind, None);
                    crate::logging::info(&format!(
                        "{} account {} reset; this session is back on it",
                        account_kind_display(kind),
                        home.label
                    ));
                    moved.push(kind);
                }
                Err(err) => crate::logging::warn(&format!(
                    "Failed to return to preferred account {}: {err}",
                    home.label
                )),
            }
        }
        moved
    }
}

/// Failures tied to this window's login that another account can escape:
/// a rejected or revoked credential (including a cached refresh rejection) or
/// an account out of credit.
fn account_local_failure(error: &str, out_of_credit: bool) -> bool {
    out_of_credit || jcode_provider_core::error_looks_like_credential_failure(error)
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
