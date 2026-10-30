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
    for account in crate::auth::claude::list_accounts().unwrap_or_default() {
        if account.expires <= now {
            expired.push(("claude", account.label));
        }
    }
    for account in crate::auth::codex::list_accounts().unwrap_or_default() {
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
            row.spans
                .push(Span::styled(" · ", Style::default().fg(dim_color())));
            if let Some(quota) = remaining_bar(usage, display_used) {
                row.spans.extend(quota.spans);
            } else {
                row.spans.push(Span::styled(
                    usage_status_label(Some(usage)),
                    Style::default().fg(dim_color()),
                ));
            }
            lines.push(row);
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
        let quota = usage.as_ref().and_then(|usage| {
            remaining_bar(usage, crate::config::config().display.usage_display_used())
        });
        let mut row = detail(
            oauth_credential_status(expired, usage.as_ref()),
            format!("{name:<9} {}", masked_identity(account.email.as_deref())),
            is_current,
        );
        row.spans
            .push(Span::styled(" · ", Style::default().fg(dim_color())));
        if let Some(quota) = quota {
            row.spans.extend(quota.spans);
        } else {
            row.spans.push(Span::styled(
                usage_status_label(usage.as_ref()),
                Style::default().fg(dim_color()),
            ));
        }
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
                    priority_row(line.clone(), inventory.pool.priority(key), &tiers)
                })
        })
        .collect();
    if lines.is_empty() {
        lines.push(Line::styled(
            "No auto-routing accounts · /login to add credential",
            Style::default().fg(dim_color()),
        ));
    }
    lines
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

/// Explain missing bars without exposing raw endpoint errors or implying zero usage.
fn usage_status_label(usage: Option<&crate::usage::AccountUsageSnapshot>) -> &'static str {
    match usage {
        None => "limits pending",
        Some(usage) => match usage.error.as_deref() {
            Some(error) if error.contains("429") => "usage fetch failed (429)",
            Some(error) if error.contains("401") => "usage fetch needs login",
            Some(_) => "usage fetch failed",
            None => "limits not reported",
        },
    }
}

fn remaining_bar(
    usage: &crate::usage::AccountUsageSnapshot,
    display_used: bool,
) -> Option<Line<'static>> {
    let (used, window) = match usage.five_hour_ratio {
        Some(used) => (used, usage.primary_label.as_deref().unwrap_or("5h")),
        None => (
            usage.seven_day_ratio?,
            usage.secondary_label.as_deref().unwrap_or("7d"),
        ),
    };
    if !used.is_finite() || !(0.0..=1.0).contains(&used) {
        return None;
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
    if let Some(label) = bar.spans.first_mut() {
        label.content = format!(
            "{}{} ",
            label.content.trim_end(),
            if usage.error.is_some() { " stale" } else { "" },
        )
        .into();
    }
    if usage.error.is_some() {
        bar.spans.push(Span::styled(
            format!(" · {}", usage_status_label(Some(usage))),
            Style::default().fg(dim_color()),
        ));
    }
    Some(bar)
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
mod tests {
    use super::*;
    use crate::auth::{ActiveCredential, ProviderAuth};

    fn text(lines: Vec<Line<'static>>) -> String {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn remote_rows_mask_email_and_show_quota() {
        let account = crate::usage::AccountUsageSnapshot {
            label: "work".into(),
            email: Some("jeremy@example.com".into()),
            exhausted: false,
            primary_label: Some("5-hour".into()),
            five_hour_ratio: Some(0.15),
            secondary_label: None,
            seven_day_ratio: None,
            resets_at: None,
            error: None,
        };
        let out = text(remote_inventory_lines(
            &[],
            &[account],
            "openai",
            Some("work"),
        ));
        assert!(out.contains("openai"), "{out}");
        assert!(out.contains("j***y@example.com"), "{out}");
        assert!(!out.contains("jeremy@"), "{out}");
        assert!(out.contains("85%"), "{out}");
    }

    fn auth() -> AuthStatus {
        AuthStatus {
            anthropic: ProviderAuth {
                state: AuthState::Available,
                has_oauth: true,
                oauth_state: AuthState::Available,
                has_api_key: true,
            },
            openai: AuthState::Available,
            openai_has_oauth: true,
            openai_oauth_state: AuthState::Available,
            openai_has_api_key: true,
            openrouter: AuthState::Available,
            ..Default::default()
        }
    }

    fn login(provider: &'static str, label: &str, email: &str) -> OAuthLogin {
        OAuthLogin {
            provider,
            label: label.into(),
            email: Some(email.into()),
            active: true,
        }
    }

    #[test]
    fn router_inventory_follows_pool_order_within_priority_without_promoting_active() {
        let inventory = Inventory {
            logins: vec![
                login("claude", "a", "alice@example.com"),
                login("openai", "a", "anna@example.com"),
                login("openai", "b", "bob@example.com"),
            ],
            pool: AccountPool {
                members: vec![
                    "openai:b".into(),
                    "claude:a".into(),
                    "openai-api".into(),
                    "openai:a".into(),
                ],
                excluded: vec![],
                priorities: [("openai-api".into(), 1)].into_iter().collect(),
                ..Default::default()
            },
            ..Default::default()
        };
        let lines = inventory_lines(
            &auth(),
            ActiveCredentialOverrides {
                openai: Some(ActiveCredential::OAuth),
                anthropic: Some(ActiveCredential::OAuth),
            },
            &inventory,
            "openai",
            Some("a"),
        );
        let rendered = text(lines.clone());
        assert_eq!(lines.len(), 4, "{rendered}");
        assert!(text(vec![lines[0].clone()]).contains("b***b@example.com"));
        assert!(text(vec![lines[1].clone()]).contains("a***e@example.com"));
        assert!(text(vec![lines[2].clone()]).contains("API key"));
        assert!(text(vec![lines[3].clone()]).contains("a***a@example.com"));
        assert_eq!(lines[3].spans[1].content, "○");
        assert_eq!(lines[0].spans[1].content, "○");
        assert_eq!(rendered.matches('●').count(), 1);
        assert_eq!(lines[3].spans[0].content, "> ");
        assert_eq!(rendered.matches('>').count(), 1);
        for (index, line) in lines.iter().enumerate() {
            let state = if index == 2 {
                AuthState::Available
            } else {
                AuthState::NotConfigured
            };
            assert_eq!(line.spans[1].style.fg, Some(auth_dot_color(state)));
        }
        for word in [
            "active",
            "standby",
            "fallback",
            "manual",
            "alice@example.com",
            "bob@example.com",
            "anthropic-api",
        ] {
            assert!(!rendered.contains(word), "{rendered}");
        }
    }

    #[test]
    fn router_inventory_hides_excluded_and_manual_credentials_even_if_active() {
        let inventory = Inventory {
            logins: vec![
                login("openai", "excluded", "secret@example.com"),
                login("claude", "a", "alice@example.com"),
            ],
            pool: AccountPool {
                members: vec!["openrouter".into()],
                excluded: vec!["openai:excluded".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        let rendered = text(inventory_lines(
            &auth(),
            ActiveCredentialOverrides {
                openai: Some(ActiveCredential::ApiKey),
                anthropic: None,
            },
            &inventory,
            "openai",
            Some("excluded"),
        ));
        assert!(rendered.contains("openrouter"));
        assert!(rendered.contains("API key"));
        assert!(rendered.contains("a***e@example.com"));
        assert!(!rendered.contains("openai"));
        assert!(!rendered.contains("s***t@example.com"));
        assert_eq!(rendered.matches("API key").count(), 1);
    }

    #[test]
    fn router_inventory_default_subscriptions_and_expiry_are_account_scoped() {
        let mut status = auth();
        status.openai_oauth_state = AuthState::Expired;
        let inventory = Inventory {
            logins: vec![
                login("openai", "old", "old@example.com"),
                login("openai", "bound", "new@example.com"),
            ],
            expired: vec![("openai", "old".into())],
            ..Default::default()
        };
        let lines = inventory_lines(
            &status,
            ActiveCredentialOverrides {
                openai: Some(ActiveCredential::OAuth),
                anthropic: None,
            },
            &inventory,
            "openai",
            Some("bound"),
        );
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].spans[1].content, "◐");
        assert_eq!(
            lines[0].spans[1].style.fg,
            Some(auth_dot_color(AuthState::Expired))
        );
        assert_eq!(lines[0].spans[0].content, "  ");
        assert_eq!(lines[1].spans[0].content, "> ");
        assert_eq!(lines[1].spans[1].content, "○");
        assert_eq!(
            lines[1].spans[1].style.fg,
            Some(auth_dot_color(AuthState::NotConfigured))
        );
        assert!(!text(lines).contains("API key"));
    }

    #[test]
    fn router_inventory_selection_is_independent_of_status() {
        for state in [
            AuthState::Available,
            AuthState::Expired,
            AuthState::NotConfigured,
        ] {
            for selected in [false, true] {
                let row = detail(state, "credential".into(), selected);
                assert_eq!(row.spans[0].content, if selected { "> " } else { "  " });
                assert_eq!(row.spans[1].content, auth_dot_char(state));
                assert_eq!(row.spans[1].style.fg, Some(auth_dot_color(state)));
                assert_eq!(row.width(), 14);
            }
        }
    }

    #[test]
    fn router_priorities_only_dim_original_identity_colors_and_fit_narrow_headers() {
        let usage = crate::usage::AccountUsageSnapshot {
            label: "one".into(),
            email: None,
            exhausted: false,
            primary_label: Some("5h".into()),
            five_hour_ratio: Some(0.48),
            secondary_label: None,
            seven_day_ratio: None,
            resets_at: None,
            error: None,
        };
        for selected in [false, true] {
            let mut row = detail(
                AuthState::Available,
                "openai 界界@example.com".into(),
                selected,
            );
            row.spans
                .extend(remaining_bar(&usage, false).unwrap().spans);
            let high = priority_row(row.clone(), 20, &[20, 30, 90]);
            let low = priority_row(row.clone(), 90, &[20, 30, 90]);
            assert_eq!(low.spans[2].content, row.spans[2].content);
            assert_eq!(high.spans[2].style.fg, row.spans[2].style.fg);
            assert_eq!(low.spans[..2], row.spans[..2]);
            assert_eq!(low.spans[3..], row.spans[3..]);
            assert_ne!(high.spans[2].style.fg, low.spans[2].style.fg);
            assert_eq!(
                high.spans[2].style.fg,
                priority_row(row.clone(), 0, &[0, 1, 2]).spans[2].style.fg
            );
            assert_eq!(
                high.spans[2].style.fg,
                priority_row(row.clone(), 90, &[90]).spans[2].style.fg
            );
            for width in [0, 1, 4, 12, 24, 40, 80] {
                let fitted = fit_header_lines(vec![low.clone()], width);
                assert!(fitted[0].width() <= usize::from(width.saturating_sub(1)));
            }
        }
    }

    #[test]
    fn router_inventory_usability_requires_account_scoped_evidence() {
        let mut usage = crate::usage::AccountUsageSnapshot {
            label: "one".into(),
            email: None,
            exhausted: false,
            primary_label: Some("5h".into()),
            five_hour_ratio: Some(0.48),
            secondary_label: None,
            seven_day_ratio: None,
            resets_at: None,
            error: None,
        };
        assert_eq!(
            oauth_credential_status(false, None),
            CredentialStatus::Unknown
        );
        assert_eq!(
            oauth_credential_status(true, None),
            CredentialStatus::Attention
        );
        assert_eq!(
            oauth_credential_status(false, Some(&usage)),
            CredentialStatus::Available
        );
        usage.exhausted = true;
        assert_eq!(
            oauth_credential_status(false, Some(&usage)),
            CredentialStatus::Blocked
        );
        usage.exhausted = false;
        usage.five_hour_ratio = Some(1.0);
        assert_eq!(
            oauth_credential_status(false, Some(&usage)),
            CredentialStatus::Blocked
        );
        usage.five_hour_ratio = Some(0.48);
        usage.seven_day_ratio = Some(1.0);
        assert_eq!(
            oauth_credential_status(false, Some(&usage)),
            CredentialStatus::Blocked
        );
        usage.seven_day_ratio = None;
        for error in [
            "Usage API error (401 Unauthorized): invalid token",
            "API error (401 Unauthorized)",
            "HTTP 401",
        ] {
            usage.error = Some(error.into());
            assert_eq!(
                oauth_credential_status(false, Some(&usage)),
                CredentialStatus::Blocked,
                "{error}"
            );
        }
        // Probe failures cannot make old quota values look authoritative.
        usage.exhausted = true;
        for error in [
            "timeout",
            "API error (403 Forbidden)",
            "API error (429 Too Many Requests)",
            "API error (500): body mentions 401 Unauthorized",
            "HTTP 4010",
        ] {
            usage.error = Some(error.into());
            assert_eq!(
                oauth_credential_status(false, Some(&usage)),
                CredentialStatus::Unknown,
                "{error}"
            );
        }
        usage.error = None;
        usage.exhausted = false;
        for ratio in [None, Some(f32::NAN), Some(-0.1), Some(1.1)] {
            usage.five_hour_ratio = ratio;
            assert_eq!(
                oauth_credential_status(false, Some(&usage)),
                CredentialStatus::Unknown
            );
        }
        for (status, dot, color) in [
            (
                CredentialStatus::Available,
                "●",
                auth_dot_color(AuthState::Available),
            ),
            (
                CredentialStatus::Attention,
                "◐",
                auth_dot_color(AuthState::Expired),
            ),
            (CredentialStatus::Blocked, "●", rgb(255, 100, 100)),
            (
                CredentialStatus::Unknown,
                "○",
                auth_dot_color(AuthState::NotConfigured),
            ),
        ] {
            for selected in [false, true] {
                let row = detail(status, "credential".into(), selected);
                assert_eq!(row.spans[0].content, if selected { "> " } else { "  " });
                assert_eq!(row.spans[1].content, dot);
                assert_eq!(row.spans[1].style.fg, Some(color));
                assert_eq!(row.width(), 14);
            }
        }
    }

    #[test]
    fn router_inventory_empty_and_unknown_identities_remain_private() {
        assert_eq!(masked_identity(None), "saved account");
        assert_eq!(
            masked_identity(Some("secret-not-an-email")),
            "saved account"
        );
        assert_eq!(masked_identity(Some("a@bad\ndomain")), "saved account");
        assert_eq!(masked_identity(Some("éx@example.com")), "é*@example.com");
        let rendered = text(inventory_lines(
            &AuthStatus::default(),
            ActiveCredentialOverrides::default(),
            &Inventory::default(),
            "",
            None,
        ));
        assert!(rendered.contains("No auto-routing accounts"));
        assert!(rendered.contains("/login"));
    }

    #[test]
    fn router_inventory_missing_usage_has_explicit_safe_status() {
        assert_eq!(usage_status_label(None), "limits pending");
        let mut usage = crate::usage::AccountUsageSnapshot {
            label: "test".into(),
            email: None,
            exhausted: false,
            primary_label: None,
            five_hour_ratio: None,
            secondary_label: None,
            seven_day_ratio: None,
            resets_at: None,
            error: None,
        };
        assert_eq!(usage_status_label(Some(&usage)), "limits not reported");
        for (error, expected) in [
            (
                "Usage temporarily rate limited (429)",
                "usage fetch failed (429)",
            ),
            (
                "Couldn't fetch usage (429: lookup throttled)",
                "usage fetch failed (429)",
            ),
            (
                "Usage authentication failed (401)",
                "usage fetch needs login",
            ),
            ("secret transport diagnostic", "usage fetch failed"),
        ] {
            usage.error = Some(error.into());
            assert_eq!(usage_status_label(Some(&usage)), expected);
            assert!(remaining_bar(&usage, false).is_none());
        }
        usage.error = Some("Usage authentication failed (401)".into());
        assert_eq!(
            oauth_credential_status(false, Some(&usage)),
            CredentialStatus::Blocked
        );
    }

    #[test]
    fn router_inventory_weekly_only_usage_shows_reported_window() {
        let mut usage = crate::usage::AccountUsageSnapshot {
            label: "weekly-only".into(),
            email: None,
            exhausted: false,
            primary_label: None,
            five_hour_ratio: None,
            secondary_label: Some("7-day window".into()),
            seven_day_ratio: Some(0.0),
            resets_at: None,
            error: None,
        };
        let bar = remaining_bar(&usage, false).unwrap();
        assert_eq!(bar.spans[0].content, "7d ");
        assert!(text(vec![bar]).contains("100% left"));
        usage.seven_day_ratio = Some(1.0);
        assert!(text(vec![remaining_bar(&usage, false).unwrap()]).contains("0% left"));
        for ratio in [None, Some(f32::NAN), Some(-0.1), Some(1.1)] {
            usage.seven_day_ratio = ratio;
            assert!(remaining_bar(&usage, false).is_none());
        }
        usage.seven_day_ratio = Some(0.0);
        usage.error = Some("Usage unavailable".into());
        let stale = text(vec![remaining_bar(&usage, false).unwrap()]);
        assert!(stale.contains("100% left"));
        assert!(stale.contains("7d stale"));
        assert!(stale.contains("usage fetch failed"));
    }

    #[test]
    fn router_inventory_remaining_bar_rejects_unknown_and_invalid_usage() {
        let mut usage = crate::usage::AccountUsageSnapshot {
            label: "one".into(),
            email: None,
            exhausted: false,
            primary_label: Some("5h".into()),
            five_hour_ratio: Some(0.48),
            secondary_label: None,
            seven_day_ratio: None,
            resets_at: None,
            error: None,
        };
        let bar = remaining_bar(&usage, false).unwrap();
        assert!(text(vec![bar.clone()]).contains("52% left"));
        assert!(text(vec![bar.clone()]).contains('▰'));
        assert!(text(vec![bar.clone()]).contains('▱'));
        assert!(!text(vec![bar.clone()]).contains('['));
        assert_eq!(bar.spans[0].content, "5h ");
        let shared = crate::tui::info_widget::render_usage_limit_bar("5h", 48, 52, None, 22, false);
        assert_eq!(bar.spans[1..], shared.spans[1..]);
        assert!(text(vec![remaining_bar(&usage, true).unwrap()]).contains("48% used"));
        for ratio in [None, Some(f32::NAN), Some(-0.1), Some(1.1)] {
            usage.five_hour_ratio = ratio;
            assert!(remaining_bar(&usage, false).is_none());
        }
        usage.five_hour_ratio = Some(0.0);
        usage.error = Some("HTTP 429".into());
        let stale = text(vec![remaining_bar(&usage, false).unwrap()]);
        assert!(stale.contains("100% left"));
        assert!(stale.contains("5h stale"));
        assert!(stale.contains("usage fetch failed (429)"));
        assert_eq!(
            oauth_credential_status(false, Some(&usage)),
            CredentialStatus::Unknown
        );
    }
}
