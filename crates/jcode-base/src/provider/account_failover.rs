use super::ActiveProvider;

impl super::MultiProvider {
    /// A stored OAuth login does not imply this request uses it. Explicit API
    /// routes, and Auto requests that resolved to an API key, must never rotate
    /// OAuth accounts or poison their cooldowns.
    pub(super) fn provider_uses_oauth_accounts(&self, provider: ActiveProvider) -> bool {
        let runtime = match provider {
            ActiveProvider::Claude => self.anthropic_provider(),
            ActiveProvider::OpenAI => self.openai_provider(),
            _ => None,
        };
        let Some(runtime) = runtime else {
            return false;
        };
        if let Some(credential) = runtime.active_resolved_credential() {
            return credential.is_subscription();
        }
        match runtime.credential_mode() {
            super::CredentialMode::OAuth => true,
            super::CredentialMode::ApiKey => false,
            super::CredentialMode::Auto => match provider {
                ActiveProvider::Claude => crate::auth::claude::load_credentials().is_ok(),
                ActiveProvider::OpenAI => crate::auth::codex::load_oauth_credentials().is_ok(),
                _ => false,
            },
        }
    }

    pub(super) fn provider_uses_account_cooldown(&self, provider: ActiveProvider) -> bool {
        multi_account_provider_kind(provider).is_none()
            || self.provider_uses_oauth_accounts(provider)
    }
}

pub(super) fn multi_account_provider_kind(
    provider: ActiveProvider,
) -> Option<crate::usage::MultiAccountProviderKind> {
    match provider {
        ActiveProvider::Claude => Some(crate::usage::MultiAccountProviderKind::Anthropic),
        ActiveProvider::OpenAI => Some(crate::usage::MultiAccountProviderKind::OpenAI),
        _ => None,
    }
}

pub(super) fn account_usage_probe(
    provider: ActiveProvider,
) -> Option<crate::usage::AccountUsageProbe> {
    let kind = multi_account_provider_kind(provider)?;
    crate::usage::account_usage_probe_sync(kind)
}

pub(super) fn same_provider_account_failover_enabled() -> bool {
    crate::config::Config::load()
        .provider
        .same_provider_account_failover
}

pub(super) fn active_account_label_for_provider(provider: ActiveProvider) -> Option<String> {
    match provider {
        ActiveProvider::Claude => crate::auth::claude::active_account_label(),
        ActiveProvider::OpenAI => crate::auth::codex::active_account_label(),
        _ => None,
    }
}

pub(super) fn set_account_override_for_provider(provider: ActiveProvider, label: Option<String>) {
    match provider {
        ActiveProvider::Claude => crate::auth::claude::set_active_account_override(label),
        ActiveProvider::OpenAI => crate::auth::codex::set_active_account_override(label),
        _ => {}
    }
}

fn same_provider_account_rotation(provider: ActiveProvider) -> Vec<String> {
    let prefix = match provider {
        ActiveProvider::Claude => "claude",
        ActiveProvider::OpenAI => "openai",
        _ => return Vec::new(),
    };
    let current_label = active_account_label_for_provider(provider);
    // Routing must not synchronously query every login's usage endpoint. The
    // live request is the authoritative probe and failures establish a scoped
    // cooldown, which we can inspect without network IO or switching accounts.
    let labels: Vec<String> = match provider {
        ActiveProvider::Claude => crate::auth::claude::list_accounts()
            .unwrap_or_default()
            .into_iter()
            .map(|account| account.label)
            .collect(),
        ActiveProvider::OpenAI => crate::auth::codex::list_accounts()
            .unwrap_or_default()
            .into_iter()
            .map(|account| account.label)
            .collect(),
        _ => Vec::new(),
    };
    // Only auto-switch pool members rotate, cycling in the user's order from
    // the account after the current one.
    crate::auth::account_pool::AccountPool::load().rotation(
        prefix,
        current_label.as_deref(),
        &labels,
    )
}

pub(super) fn same_provider_account_candidates(provider: ActiveProvider) -> Vec<String> {
    // Never clear a known cooldown just to try the same spent login again.
    same_provider_account_rotation(provider)
        .into_iter()
        .filter(|label| {
            super::models::provider_unavailability_detail_for_account_label(
                super::MultiProvider::provider_key(provider),
                Some(label),
            )
            .is_none()
        })
        .collect()
}

/// Whether another pool login is ready, based on local account cooldowns only.
pub fn has_eligible_alternate_account(provider: &str) -> bool {
    let provider = match provider {
        "claude" | "anthropic" => ActiveProvider::Claude,
        "openai" => ActiveProvider::OpenAI,
        _ => return false,
    };
    same_provider_account_failover_enabled()
        && active_account_label_for_provider(provider).is_some()
        && !same_provider_account_candidates(provider).is_empty()
}

/// Whether the multi-account router owns an OAuth account failure. Includes
/// cooling-down pool members so the final login in a rotation also returns its
/// failure immediately rather than sleeping before reporting pool exhaustion.
pub fn has_alternate_account_in_pool(provider: &str) -> bool {
    let provider = match provider {
        "claude" | "anthropic" => ActiveProvider::Claude,
        "openai" => ActiveProvider::OpenAI,
        _ => return false,
    };
    same_provider_account_failover_enabled()
        && active_account_label_for_provider(provider).is_some()
        && !same_provider_account_rotation(provider).is_empty()
}

pub(super) fn account_switch_guidance(provider: ActiveProvider) -> Option<String> {
    let kind = multi_account_provider_kind(provider)?;
    active_account_label_for_provider(provider)?;
    let alternatives = same_provider_account_candidates(provider);
    Some(if alternatives.is_empty() {
        format!(
            "No alternate {} accounts are ready in the auto-switch pool. Use `/usage` to inspect limits and reset times.",
            kind.display_name()
        )
    } else {
        format!(
            "Alternate {} accounts in the auto-switch pool: {}. Use `/usage` to inspect limits.",
            kind.display_name(),
            alternatives.join(", ")
        )
    })
}

pub(super) fn usage_exhausted_reason(provider: ActiveProvider) -> String {
    let mut reason = "OAuth usage exhausted".to_string();
    if let Some(guidance) = account_switch_guidance(provider) {
        reason.push_str(". ");
        reason.push_str(&guidance);
    }
    reason
}

fn error_looks_like_usage_limit(summary: &str) -> bool {
    let lower = summary.to_ascii_lowercase();
    [
        "quota",
        "insufficient_quota",
        "rate limit",
        "rate_limit",
        "rate_limit_exceeded",
        "too many requests",
        "billing",
        "credit",
        "payment required",
        "usage exhausted",
        "limit reached",
        "429",
        "402",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

pub(super) fn maybe_annotate_limit_summary(provider: ActiveProvider, summary: String) -> String {
    if !error_looks_like_usage_limit(&summary) {
        return summary;
    }
    let Some(guidance) = account_switch_guidance(provider) else {
        return summary;
    };
    if summary.contains(&guidance) {
        return summary;
    }
    format!("{}. {}", summary, guidance)
}
