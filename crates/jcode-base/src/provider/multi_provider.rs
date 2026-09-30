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
    /// Effective failover setting: the session toggle, else config.
    pub(super) fn account_failover_enabled(&self) -> bool {
        self.account_failover
            .enabled()
            .unwrap_or_else(same_provider_account_failover_config)
    }

    /// Label this session uses for `kind`: its pin, else the label the last
    /// request resolved to, else the stored default.
    pub(super) fn session_account_label(&self, kind: AccountProviderKind) -> Option<String> {
        self.account_pin(kind)
            .map(|pin| pin.label)
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
        let has_alternative = !crate::auth::account_pool::AccountPool::load()
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
                    // Not account exhaustion: the normal error paths own it.
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
                            record_provider_unavailable_for_label(
                                provider_key,
                                &label,
                                &Self::summarize_error(&error),
                            );
                        }
                        error
                    }
                },
                Err(error) => {
                    if Self::classify_failover_error(&error).should_mark_provider_unavailable() {
                        record_provider_unavailable_for_label(
                            provider_key,
                            &label,
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
