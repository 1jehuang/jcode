//! Startup-only credential inventory. No credential probes or usage refreshes.
use super::*;
use crate::auth::account_pool::{AccountPool, OAuthLogin, account_key};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

static GENERATION: AtomicU64 = AtomicU64::new(0);

pub(super) fn generation() -> u64 {
    let _ = cached_inventory();
    GENERATION.load(Ordering::Acquire)
}

#[derive(Clone, Default, PartialEq, Eq)]
struct Inventory {
    logins: Vec<OAuthLogin>,
    pool: AccountPool,
    expired: Vec<(&'static str, String)>,
}

#[derive(Default)]
struct InventoryCache {
    inventory: Inventory,
    updated: Option<Instant>,
    refreshing: bool,
}

fn cached_inventory() -> Inventory {
    static CACHE: OnceLock<Mutex<InventoryCache>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let Ok(mut state) = cache.try_lock() else {
        return Inventory::default();
    };
    let inventory = state.inventory.clone();
    if !state.refreshing
        && state
            .updated
            .is_none_or(|when| when.elapsed() >= Duration::from_secs(2))
    {
        state.refreshing = true;
        // Never read auth files, migrate credentials, or probe on the render thread.
        let spawned = std::thread::Builder::new()
            .name("header-account-inventory".into())
            .spawn(move || {
                let inventory = load_inventory();
                if let Ok(mut state) = cache.lock() {
                    if state.inventory != inventory {
                        state.inventory = inventory;
                        GENERATION.fetch_add(1, Ordering::Release);
                    }
                    state.updated = Some(Instant::now());
                    state.refreshing = false;
                }
            });
        if spawned.is_err() {
            state.refreshing = false;
        }
    }
    inventory
}

fn load_inventory() -> Inventory {
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
    }
}

fn masked_identity(email: Option<&str>) -> String {
    let Some((local, domain)) = email.and_then(|email| email.split_once('@')) else {
        return "saved account".into();
    };
    // Do not echo malformed identities or terminal control characters.
    if domain.is_empty() || domain.chars().any(char::is_control) {
        return "saved account".into();
    }
    let chars: Vec<_> = local.chars().filter(|c| !c.is_control()).collect();
    let local = match chars.as_slice() {
        [] => "?".into(),
        [first] | [first, _] => format!("{first}*"),
        [first, .., last] => format!("{first}***{last}"),
    };
    format!("{local}@{domain}")
}

pub(super) fn lines(
    auth: &AuthStatus,
    active: ActiveCredentialOverrides,
    provider_name: &str,
    selected_label: Option<&str>,
) -> Vec<Line<'static>> {
    inventory_lines(
        auth,
        active,
        &cached_inventory(),
        provider_name,
        selected_label,
    )
}

/// Rows for a remote (SSH) daemon, built only from its credential-free
/// `AccountUsage` report. `None` until the daemon has answered.
pub(super) fn remote_lines(
    provider_name: &str,
    selected_label: Option<&str>,
) -> Option<Vec<Line<'static>>> {
    let (anthropic, openai) = crate::usage::remote_account_list()?;
    Some(remote_inventory_lines(
        &anthropic,
        &openai,
        provider_name,
        selected_label,
    ))
}

fn remote_inventory_lines(
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

fn inventory_lines(
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
        ("gemini", "gemini", auth.gemini, false),
        ("antigravity", "antigravity", auth.antigravity, false),
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
        lines.push(Line::raw(
            "No auto-routing accounts · /login to add credential",
        ));
    }
    lines
}

const QUOTA_SEPARATOR: &str = " │ ";

fn inline_account_row(mut identity: Line<'static>, quotas: Vec<Line<'static>>) -> Line<'static> {
    if quotas.is_empty() {
        return identity;
    }
    // Keep status/error and banked-reset telemetry after the quotas, so fitting
    // cannot sacrifice a model window just to retain a countdown.
    let suffix = identity.spans.split_off(3);
    for quota in quotas {
        identity.spans.push(Span::styled(
            QUOTA_SEPARATOR,
            Style::default().fg(dim_color()),
        ));
        identity.spans.extend(quota.spans);
    }
    identity.spans.extend(suffix);
    identity
}

/// Account rows remain one logical row at every width. Drop decoration before
/// quota labels/percentages, then truncate by terminal cells, never bytes.
pub(super) fn fit_account_row(mut row: Line<'static>, budget: usize) -> Line<'static> {
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

fn priority_row(mut row: Line<'static>, priority: u32, tiers: &[u32]) -> Line<'static> {
    let rank = tiers.binary_search(&priority).unwrap_or(0);
    let dimming = if tiers.len() < 2 {
        0
    } else {
        rank * 40 / (tiers.len() - 1)
    };
    // Only decorate the identity span. Arrows, status dots and the info widget's
    // usage-limit bars retain their established glyphs and colors.
    if let Some(identity) = row.spans.get_mut(2)
        && dimming > 0
        && let Some(base) = identity.style.fg
    {
        let (r, g, b) = match base {
            Color::Rgb(r, g, b) => (r, g, b),
            Color::Indexed(index) => jcode_tui_style::indexed_to_rgb(index),
            _ => return row,
        };
        let scale = |value: u8| (usize::from(value) * (100 - dimming) / 100) as u8;
        identity.style = identity.style.fg(Color::Rgb(scale(r), scale(g), scale(b)));
    }
    row
}

// Quotas already use the daemon mirror for connected local clients. Resets must
// use the same source rather than an unrelated, process-local endpoint cache.
fn header_account_banked_resets(
    provider: crate::usage::MultiAccountProviderKind,
    label: &str,
) -> Option<crate::usage::AccountBankedResets> {
    if crate::usage::remote_account_usage_received() {
        crate::usage::remote_account_banked_resets(provider, label)
    } else {
        crate::usage::cached_account_banked_resets(provider, label)
    }
}

/// Warmer colors mean less time to spend a reset. Unknown expiry stays blue.
fn banked_expiry_color(expires_at: Option<&str>, now: chrono::DateTime<chrono::Utc>) -> Color {
    let Some(expiry) = expires_at.and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
    else {
        return Color::Rgb(120, 190, 255);
    };
    let remaining = expiry.signed_duration_since(now);
    if remaining <= chrono::Duration::hours(24) {
        Color::Rgb(255, 100, 100)
    } else if remaining <= chrono::Duration::days(3) {
        Color::Rgb(255, 180, 80)
    } else if remaining <= chrono::Duration::days(7) {
        Color::Rgb(230, 210, 100)
    } else {
        Color::Rgb(100, 210, 130)
    }
}

/// Compact banked-reset suffix: "2 resets" when claimable, otherwise when the
/// next one unlocks. Nothing is shown when no reset is known or upcoming.
fn banked_resets_span(banked: &crate::usage::AccountBankedResets) -> Option<Span<'static>> {
    if banked.available > 0 {
        let noun = if banked.available == 1 {
            "reset"
        } else {
            "resets"
        };
        let expiry = banked
            .expires_at
            .as_deref()
            .map(|at| format!(" · exp {}", crate::usage::format_reset_time(at)))
            .unwrap_or_default();
        return Some(Span::styled(
            format!("↻ {} {noun}{expiry}", banked.available),
            Style::default().fg(banked_expiry_color(
                banked.expires_at.as_deref(),
                chrono::Utc::now(),
            )),
        ));
    }
    let at = banked.next_available_at.as_deref()?;
    Some(Span::styled(
        format!("↻ in {}", crate::usage::format_reset_time(at)),
        Style::default().fg(dim_color()),
    ))
}

/// Explain missing bars without exposing raw endpoint errors or implying zero usage.
fn usage_status_label(usage: Option<&crate::usage::AccountUsageSnapshot>) -> String {
    match usage {
        None => "limits pending".into(),
        Some(usage) => match usage.error.as_deref() {
            Some(error) if error.contains("429") => {
                match crate::usage::usage_error_retry_at(error).map(crate::usage::format_reset_time)
                {
                    Some(wait) if wait != "now" => format!("usage throttled, retry in {wait}"),
                    _ => "usage throttled, retrying soon".into(),
                }
            }
            Some(error) if error.contains("401") => "usage fetch needs login".into(),
            Some(_) => "usage fetch failed".into(),
            None => "limits not reported".into(),
        },
    }
}

/// Render every reported quota through the info widget's shared limit renderer.
fn remaining_bars(
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

#[cfg(test)]
fn remaining_bar(
    usage: &crate::usage::AccountUsageSnapshot,
    display_used: bool,
) -> Option<Line<'static>> {
    let rows = remaining_bars(usage, display_used);
    if rows.is_empty() {
        return None;
    }
    let mut spans: Vec<_> = rows.into_iter().flat_map(|row| row.spans).collect();
    if usage.error.is_some() {
        spans.push(Span::styled(
            format!(" · {}", usage_status_label(Some(usage))),
            Style::default().fg(dim_color()),
        ));
    }
    Some(Line::from(spans))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CredentialStatus {
    Available,
    Attention,
    Blocked,
    Unknown,
}

impl From<AuthState> for CredentialStatus {
    fn from(state: AuthState) -> Self {
        match state {
            AuthState::Available => Self::Available,
            AuthState::Expired => Self::Attention,
            AuthState::NotConfigured => Self::Unknown,
        }
    }
}

fn oauth_credential_status(
    expired: bool,
    usage: Option<&crate::usage::AccountUsageSnapshot>,
) -> CredentialStatus {
    if expired {
        return CredentialStatus::Attention;
    }
    let Some(usage) = usage else {
        return CredentialStatus::Unknown;
    };
    if let Some(error) = usage.error.as_deref() {
        // Only an explicit HTTP authentication rejection is conclusive. A
        // timeout, 403 quota-scope denial, or 429 usage-endpoint throttle is not.
        let unauthorized = error == "Usage authentication failed (401)"
            || ["Usage API error (401", "API error (401", "HTTP 401"]
                .iter()
                .filter_map(|prefix| error.strip_prefix(prefix))
                .any(|suffix| suffix.starts_with([' ', ')', ':']) || suffix.is_empty());
        return if unauthorized {
            CredentialStatus::Blocked
        } else {
            CredentialStatus::Unknown
        };
    }
    if usage.exhausted
        || [usage.five_hour_ratio, usage.seven_day_ratio]
            .into_iter()
            .flatten()
            .any(|ratio| ratio == 1.0)
    {
        return CredentialStatus::Blocked;
    }
    if [usage.five_hour_ratio, usage.seven_day_ratio]
        .into_iter()
        .flatten()
        .chain(
            usage
                .additional_windows
                .iter()
                .map(|window| window.usage_ratio),
        )
        .any(|ratio| ratio.is_finite() && (0.0..=1.0).contains(&ratio))
    {
        CredentialStatus::Available
    } else {
        CredentialStatus::Unknown
    }
}

fn detail(state: impl Into<CredentialStatus>, label: String, selected: bool) -> Line<'static> {
    let (dot, color) = match state.into() {
        CredentialStatus::Available => ("●", auth_dot_color(AuthState::Available)),
        CredentialStatus::Attention => ("◐", auth_dot_color(AuthState::Expired)),
        CredentialStatus::Blocked => ("●", rgb(255, 100, 100)),
        CredentialStatus::Unknown => ("○", auth_dot_color(AuthState::NotConfigured)),
    };
    Line::from(vec![
        Span::styled(
            if selected { "> " } else { "  " },
            Style::default().fg(auth_dot_color(AuthState::Available)),
        ),
        Span::styled(dot, Style::default().fg(color)),
        Span::styled(format!(" {label}"), Style::default().fg(dim_color())),
    ])
}

#[cfg(test)]
#[path = "account_router_tests.rs"]
mod tests;
