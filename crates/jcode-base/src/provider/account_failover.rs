use super::ActiveProvider;

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

pub(super) fn same_provider_account_candidates(provider: ActiveProvider) -> Vec<String> {
    let prefix = match provider {
        ActiveProvider::Claude => "claude",
        ActiveProvider::OpenAI => "openai",
        _ => return Vec::new(),
    };
    let current_label = active_account_label_for_provider(provider);
    let mut labels: Vec<String> = Vec::new();
    let mut exhausted: Vec<String> = Vec::new();
    let push_unique = |labels: &mut Vec<String>, label: String| {
        if !labels.contains(&label) {
            labels.push(label);
        }
    };

    if let Some(probe) = account_usage_probe(provider) {
        for account in probe.accounts {
            if account.exhausted || account.error.is_some() {
                exhausted.push(account.label.clone());
            }
            push_unique(&mut labels, account.label);
        }
    }
    let stored: Vec<String> = match provider {
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
    for label in stored {
        push_unique(&mut labels, label);
    }

    // Only auto-switch pool members rotate, cycling in the user's order from
    // the account after the current one. Known-exhausted accounts go last.
    let rotation = crate::auth::account_pool::AccountPool::load().rotation(
        prefix,
        current_label.as_deref(),
        &labels,
    );
    let (ready, spent): (Vec<_>, Vec<_>) = rotation
        .into_iter()
        .partition(|label| !exhausted.contains(label));
    ready.into_iter().chain(spent).collect()
}

pub(super) fn account_switch_guidance(provider: ActiveProvider) -> Option<String> {
    let probe = account_usage_probe(provider)?;
    probe.switch_guidance().or_else(|| {
        (probe.current_exhausted() && probe.all_accounts_exhausted()).then(|| {
            format!(
                "All {} accounts appear exhausted. Use `/usage` to inspect reset times.",
                probe.provider.display_name()
            )
        })
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
