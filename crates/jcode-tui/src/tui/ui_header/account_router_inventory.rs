//! Account inventory and width-constrained rows.

use super::*;

pub(super) fn inventory_lines(
    auth: &AuthStatus,
    active: ActiveCredentialOverrides,
    inventory: &Inventory,
    provider_name: &str,
    selected_label: Option<&str>,
) -> Vec<Line<'static>> {
    use crate::auth::{ActiveCredential, resolve_dual_credential_auth};
    use jcode_provider_core::ActiveProvider;
    let mut entries = Vec::new();
    let mut quota_rows = std::collections::BTreeMap::new();
    let runtime = std::env::var("JCODE_RUNTIME_PROVIDER").ok();
    let current_provider = jcode_provider_core::parse_provider_hint(provider_name);
    // Preserve the stored login order for defaulted members. Explicit pool
    // positions below take precedence across providers, including API keys.
    for account in &inventory.logins {
        let (name, provider, oauth_state, usage_provider) = if account.provider == "claude" {
            (
                "anthropic",
                ActiveProvider::Claude,
                auth.anthropic.oauth_state,
                crate::usage::MultiAccountProviderKind::Anthropic,
            )
        } else {
            (
                "openai",
                ActiveProvider::OpenAI,
                auth.openai_oauth_state,
                crate::usage::MultiAccountProviderKind::OpenAI,
            )
        };
        let resolved = resolve_dual_credential_auth(provider, auth, runtime.as_deref());
        let credential = active.get(provider).or(resolved.map(|r| r.active));
        let bound = (current_provider == Some(provider))
            .then_some(selected_label)
            .flatten();
        let selected = bound.map_or(account.active, |label| account.label == label);
        let expired = inventory
            .expired
            .iter()
            .any(|(p, label)| *p == account.provider && label == &account.label)
            || (selected && bound.is_none() && oauth_state == AuthState::Expired);
        let is_current = current_provider == Some(provider)
            && selected
            && credential == Some(ActiveCredential::OAuth);
        let usage =
            crate::usage::cached_account_usage_status_snapshot(usage_provider, &account.label);
        let quotas = match usage.as_ref() {
            Some(usage) => {
                remaining_bars(usage, crate::config::config().display.usage_display_used())
            }
            // Accounts without cached usage have no quota rows yet.
            None => Vec::new(),
        };
        let mut row = detail(
            oauth_credential_status(expired, usage.as_ref()),
            format!("{name:<9} {}", masked_identity(account.email.as_deref())),
            is_current,
        );
        if quotas.is_empty() || usage.as_ref().is_some_and(|usage| usage.error.is_some()) {
            row.spans
                .push(Span::styled(" · ", Style::default().fg(dim_color())));
            row.spans.push(Span::styled(
                usage_status_label(usage.as_ref()),
                Style::default().fg(dim_color()),
            ));
        }
        if let Some(banked) = header_account_banked_resets(usage_provider, &account.label)
            .and_then(|banked| banked_resets_span(&banked))
        {
            row.spans
                .push(Span::styled(" · ", Style::default().fg(dim_color())));
            row.spans.push(banked);
        }
        quota_rows.insert(account_key(account.provider, Some(&account.label)), quotas);
        entries.push((
            account_key(account.provider, Some(&account.label)),
            true,
            row,
        ));
    }
    for (name, key, provider) in [
        ("anthropic", "anthropic-api", ActiveProvider::Claude),
        ("openai", "openai-api", ActiveProvider::OpenAI),
    ] {
        if let Some(resolved) = resolve_dual_credential_auth(provider, auth, runtime.as_deref())
            && resolved.has_api_key
        {
            let credential = active.get(provider).unwrap_or(resolved.active);
            let is_current =
                current_provider == Some(provider) && credential == ActiveCredential::ApiKey;
            entries.push((
                key.into(),
                false,
                detail(
                    AuthState::Available,
                    format!("{name:<9} API key"),
                    is_current,
                ),
            ));
        }
    }
    for (name, key, state, api_key) in [
        ("jcode", "jcode", auth.jcode, false),
        ("azure", "azure", auth.azure, true),
        ("bedrock", "bedrock", auth.bedrock, true),
        ("grok build", "grok-build", auth.grok_build, true),
        (
            "OpenAI-compatible profiles",
            "openai-compatible",
            auth.openai_compatible_any,
            true,
        ),
        ("openrouter", "openrouter", auth.openrouter, true),
        ("cursor", "cursor", auth.cursor, false),
        ("copilot", "copilot", auth.copilot, false),
        // Code Assist OAuth only serves paid Cloud projects now; personal
        // Google accounts route through the Antigravity row instead.
        (
            "gemini",
            "gemini",
            if std::env::var_os("GOOGLE_CLOUD_PROJECT").is_some() {
                auth.gemini
            } else {
                AuthState::NotConfigured
            },
            false,
        ),
    ] {
        if state != AuthState::NotConfigured {
            let is_current = current_provider.is_some()
                && current_provider == jcode_provider_core::parse_provider_hint(key);
            let label = if api_key {
                format!("{name:<9} API key")
            } else {
                name.into()
            };
            entries.push((
                key.into(),
                crate::auth::account_pool::default_member(key, api_key),
                detail(state, label, is_current),
            ));
        }
    }
    if auth.antigravity != AuthState::NotConfigured {
        let key = "antigravity";
        let is_current = current_provider.is_some()
            && current_provider == jcode_provider_core::parse_provider_hint(key);
        let quota = match inventory.antigravity.as_ref() {
            Some(quota) => quota.clone(),
            None => AntigravityQuota::default(),
        };
        let mut row = detail(
            auth.antigravity,
            format!(
                "{:<9} {}",
                "google",
                masked_identity(quota.email.as_deref())
            ),
            is_current,
        );
        if quota.pools.is_empty() {
            row.spans
                .push(Span::styled(" · ", Style::default().fg(dim_color())));
            row.spans.push(Span::styled(
                "quota unknown",
                Style::default().fg(dim_color()),
            ));
        }
        let display_used = crate::config::config().display.usage_display_used();
        let quotas = quota
            .pools
            .iter()
            .map(|(label, used_milli, _)| {
                let used_pct = ((*used_milli as u32 + 5) / 10).min(100) as u8;
                let mut bar = crate::tui::info_widget::render_usage_limit_bar(
                    label,
                    used_pct,
                    100u8.saturating_sub(used_pct),
                    None,
                    22,
                    display_used,
                );
                if let Some(label_span) = bar.spans.first_mut() {
                    label_span.content = format!("{label} ").into();
                }
                bar
            })
            .collect::<Vec<_>>();
        quota_rows.insert(key.to_string(), quotas);
        entries.push((
            key.into(),
            crate::auth::account_pool::default_member(key, false),
            row,
        ));
    }
    let keys: Vec<_> = entries
        .iter()
        .map(|(key, default, _)| (key.clone(), *default))
        .collect();
    let (ordered, _) = inventory.pool.partition(&keys);
    let mut tiers: Vec<_> = ordered
        .iter()
        .map(|key| inventory.pool.priority(key))
        .collect();
    tiers.sort_unstable();
    tiers.dedup();
    let mut lines: Vec<_> = ordered
        .into_iter()
        .filter_map(|key| {
            entries
                .iter()
                .find(|(candidate, _, _)| candidate == key)
                .map(|(_, _, line)| {
                    let row = priority_row(line.clone(), inventory.pool.priority(key), &tiers);
                    match quota_rows.remove(key) {
                        Some(quotas) => inline_account_row(row, quotas),
                        None => row,
                    }
                })
        })
        .collect();
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "No auto-routing accounts · /login to add credential",
            Style::default().fg(dim_color()),
        )));
    }
    lines
}

/// Account rows remain one logical row at every width. Drop decoration before
/// quota labels/percentages, then truncate by terminal cells, never bytes.
pub(in super::super) fn fit_account_row(mut row: Line<'static>, budget: usize) -> Line<'static> {
    if row.width() <= budget
        || row
            .spans
            .get(3)
            .is_none_or(|s| s.content != QUOTA_SEPARATOR)
    {
        return row;
    }
    let mut end = 3;
    while row
        .spans
        .get(end)
        .is_some_and(|s| s.content == QUOTA_SEPARATOR)
        && row.spans.len() >= end + 5
    {
        end += 5;
    }
    let suffix = row.spans.split_off(end);
    let original_identity = row.spans[2].clone();
    // Prefer a short identity over losing any limit, retaining the provider.
    let provider = original_identity
        .content
        .split_whitespace()
        .next()
        .unwrap_or("");
    row.spans[2].content = format!(" {provider}").into();
    if row.width() > budget {
        for quota in row.spans[3..].as_chunks_mut::<5>().0 {
            // These are the shared renderer's filled and empty spans. Preserve
            // its colors and convention, shrinking only their cell counts.
            for span in &mut quota[2..4] {
                span.content = span.content.chars().take(1).collect::<String>().into();
            }
        }
    }
    if row.width() > budget {
        for (index, quota) in row.spans[3..].as_chunks_mut::<5>().0.iter_mut().enumerate() {
            quota[0].content = " ".into();
            quota[2].content = "".into();
            quota[3].content = "".into();
            // One explicit convention applies to the whole compact row.
            if index == 0 {
                continue;
            }
            let percent = quota[4]
                .content
                .trim()
                .split(' ')
                .next()
                .unwrap_or("")
                .to_owned();
            quota[4].content = percent.into();
        }
    }
    // Restore as much identity as the quotas permit, using cell-aware ellipsis.
    let identity_budget = budget.saturating_sub(row.width() - row.spans[2].width());
    let fitted_identity = jcode_tui_render::truncate_line_with_ellipsis_to_width(
        &Line::from(vec![original_identity.clone()]),
        identity_budget,
    );
    row.spans[2] = Span::styled(
        fitted_identity
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>(),
        original_identity.style,
    );
    let with_suffix = Line::from(row.spans.iter().cloned().chain(suffix).collect::<Vec<_>>());
    if with_suffix.width() <= budget {
        row = with_suffix;
    }
    jcode_tui_render::truncate_line_with_ellipsis_to_width(&row, budget)
}

pub(super) fn load_inventory() -> Inventory {
    // Local auth files only. Retain metadata, never access/refresh tokens.
    let now = chrono::Utc::now().timestamp_millis();
    let mut expired = Vec::new();
    let claude_accounts = match crate::auth::claude::list_accounts() {
        Ok(accounts) => accounts,
        Err(error) => {
            crate::logging::debug(&format!(
                "Header Claude account inventory unavailable: {error}"
            ));
            Vec::new()
        }
    };
    for account in claude_accounts {
        if account.expires <= now {
            expired.push(("claude", account.label));
        }
    }
    let codex_accounts = match crate::auth::codex::list_accounts() {
        Ok(accounts) => accounts,
        Err(error) => {
            crate::logging::debug(&format!(
                "Header Codex account inventory unavailable: {error}"
            ));
            Vec::new()
        }
    };
    for account in codex_accounts {
        if account.expires_at.is_some_and(|expires| expires <= now) {
            expired.push(("openai", account.label));
        }
    }
    let logins = crate::auth::account_pool::oauth_logins();
    let mut pool = AccountPool::load();
    let keys: std::collections::BTreeSet<_> = logins
        .iter()
        .map(|login| account_key(login.provider, Some(&login.label)))
        .chain(
            crate::provider_catalog::login_providers()
                .iter()
                .map(|provider| provider.id.to_string()),
        )
        .chain(pool.members.iter().cloned())
        .chain(pool.excluded.iter().cloned())
        .chain(["openai-compatible".to_string()])
        .collect();
    // This is an in-memory render snapshot, not saved overrides. Resolve local
    // subscription metadata on this background worker, never on each frame.
    for key in keys {
        let priority = pool.priority(&key);
        pool.priorities.insert(key, priority);
    }
    Inventory {
        logins,
        pool,
        expired,
        antigravity: load_antigravity_quota(),
    }
}

/// Summarize Antigravity's per-model quotas into the shared pools Google
/// meters them by. Models that share a remaining fraction and reset time draw
/// from one pool, so a row shows a handful of limits instead of every model.
fn load_antigravity_quota() -> Option<AntigravityQuota> {
    if !crate::auth::antigravity::has_cached_auth() {
        return None;
    }
    let email = match crate::auth::antigravity::load_tokens() {
        Ok(tokens) => tokens.email,
        Err(error) => {
            crate::logging::debug(&format!("Header Antigravity tokens unavailable: {error}"));
            None
        }
    };
    let models = match crate::provider::antigravity::load_persisted_catalog() {
        Some(catalog) => catalog.models,
        None => Vec::new(),
    };
    Some(AntigravityQuota {
        email,
        pools: antigravity_quota_pools(&models),
    })
}

pub(super) fn antigravity_quota_pools(
    models: &[crate::provider::antigravity::CatalogModel],
) -> Vec<(String, u16, Option<String>)> {
    type QuotaPoolGroup<'a> = (
        (u16, Option<String>),
        Vec<&'a crate::provider::antigravity::CatalogModel>,
    );
    let mut groups: Vec<QuotaPoolGroup<'_>> = Vec::new();
    for model in models {
        let Some(remaining) = model.remaining_fraction_milli else {
            continue;
        };
        if model.id.starts_with("tab_") || model.id.starts_with("chat_") {
            continue;
        }
        let key = (remaining, model.reset_time.clone());
        match groups.iter_mut().find(|(candidate, _)| *candidate == key) {
            Some((_, members)) => members.push(model),
            None => groups.push((key, vec![model])),
        }
    }
    let mut pools: Vec<_> = groups
        .into_iter()
        .map(|((remaining, reset), members)| {
            let has = |prefix: &str| members.iter().any(|m| m.id.starts_with(prefix));
            let label = if has("claude") {
                "Claude"
            } else if members.iter().all(|m| m.id.starts_with("gemini-4-argon")) {
                "Argon"
            } else if has("gemini") {
                "Gemini"
            } else {
                "Other"
            };
            (
                label.to_string(),
                1000u16.saturating_sub(remaining.min(1000)),
                reset,
            )
        })
        .collect();
    let order = |label: &str| match label {
        "Gemini" => 0,
        "Argon" => 1,
        "Claude" => 2,
        _ => 3,
    };
    pools.sort_by_key(|(label, _, _)| order(label));
    pools
}

/// Render every reported quota through the info widget's shared limit renderer.
pub(super) fn remaining_bars(
    usage: &crate::usage::AccountUsageSnapshot,
    display_used: bool,
) -> Vec<Line<'static>> {
    let windows = [
        (
            usage.five_hour_ratio,
            usage.primary_label.as_deref().unwrap_or("5h"),
        ),
        (
            usage.seven_day_ratio,
            usage.secondary_label.as_deref().unwrap_or("7d"),
        ),
    ]
    .into_iter()
    .filter_map(|(ratio, label)| ratio.map(|ratio| (ratio, label)))
    .chain(
        usage
            .additional_windows
            .iter()
            .map(|window| (window.usage_ratio, window.label.as_str())),
    );
    let mut rows = Vec::new();
    for (used, window) in windows {
        if !used.is_finite() || !(0.0..=1.0).contains(&used) {
            continue;
        }
        let label = match window {
            "5-hour" | "5-hour window" => "5h",
            "7-day" | "7-day window" => "7d",
            label => label,
        };
        let used_pct = (used * 100.0).round() as u8;
        let mut bar = crate::tui::info_widget::render_usage_limit_bar(
            label,
            used_pct,
            100u8.saturating_sub(used_pct),
            None,
            22,
            display_used,
        );
        // Inline quotas do not need the info widget's fixed-width label column.
        if let Some(label_span) = bar.spans.first_mut() {
            label_span.content = format!(
                "{}{} ",
                label,
                if usage.error.is_some() { " stale" } else { "" }
            )
            .into();
        }
        rows.push(bar);
    }
    rows
}

pub(super) fn remote_inventory_lines(
    anthropic: &[crate::usage::AccountUsageSnapshot],
    openai: &[crate::usage::AccountUsageSnapshot],
    provider_name: &str,
    selected_label: Option<&str>,
) -> Vec<Line<'static>> {
    use jcode_provider_core::ActiveProvider;
    let current_provider = jcode_provider_core::parse_provider_hint(provider_name);
    let display_used = crate::config::config().display.usage_display_used();
    let mut lines = Vec::new();
    for (name, provider, accounts) in [
        ("openai", ActiveProvider::OpenAI, openai),
        ("anthropic", ActiveProvider::Claude, anthropic),
    ] {
        for usage in accounts {
            let account = usage;
            let is_current = current_provider == Some(provider)
                && selected_label.is_some_and(|label| label == account.label);
            let mut row = detail(
                oauth_credential_status(false, Some(usage)),
                format!("{name:<9} {}", masked_identity(account.email.as_deref())),
                is_current,
            );
            let quotas = remaining_bars(usage, display_used);
            if quotas.is_empty() || usage.error.is_some() {
                row.spans
                    .push(Span::styled(" · ", Style::default().fg(dim_color())));
                row.spans.push(Span::styled(
                    usage_status_label(Some(usage)),
                    Style::default().fg(dim_color()),
                ));
            }
            let provider_kind = if provider == ActiveProvider::OpenAI {
                crate::usage::MultiAccountProviderKind::OpenAI
            } else {
                crate::usage::MultiAccountProviderKind::Anthropic
            };
            if let Some(banked) =
                crate::usage::remote_account_banked_resets(provider_kind, &account.label)
                    .and_then(|banked| banked_resets_span(&banked))
            {
                row.spans
                    .push(Span::styled(" · ", Style::default().fg(dim_color())));
                row.spans.push(banked);
            }
            lines.push(inline_account_row(row, quotas));
        }
    }
    lines
}
