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
    Inventory {
        logins: crate::auth::account_pool::oauth_logins(),
        pool: AccountPool::load(),
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
        let usage = crate::usage::cached_account_usage_snapshot(usage_provider, &account.label);
        let quota = usage.as_ref().and_then(remaining_bar).unwrap_or_default();
        entries.push((
            account_key(account.provider, Some(&account.label)),
            true,
            detail(
                if expired {
                    "×"
                } else if is_current {
                    "●"
                } else {
                    "○"
                },
                format!(
                    "{name:<12} {}{quota}",
                    masked_identity(account.email.as_deref())
                ),
                is_current && !expired,
            ),
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
                    if is_current { "●" } else { "○" },
                    format!("{name:<12} API key"),
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
            let dot = if state == AuthState::Expired {
                "×"
            } else if is_current {
                "●"
            } else {
                "○"
            };
            let label = if api_key {
                format!("{name:<12} API key")
            } else {
                name.into()
            };
            entries.push((
                key.into(),
                crate::auth::account_pool::default_member(key, api_key),
                detail(dot, label, is_current && state == AuthState::Available),
            ));
        }
    }
    let keys: Vec<_> = entries
        .iter()
        .map(|(key, default, _)| (key.clone(), *default))
        .collect();
    let (ordered, _) = inventory.pool.partition(&keys);
    let mut lines: Vec<_> = ordered
        .into_iter()
        .filter_map(|key| {
            entries
                .iter()
                .find(|(candidate, _, _)| candidate == key)
                .map(|(_, _, line)| line.clone())
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

fn remaining_bar(usage: &crate::usage::AccountUsageSnapshot) -> Option<String> {
    if usage.error.is_some() {
        return None;
    }
    let used = usage.five_hour_ratio?;
    if !used.is_finite() || !(0.0..=1.0).contains(&used) {
        return None;
    }
    let remaining = 1.0 - used;
    let filled = (remaining * 4.0).round() as usize;
    let label = match usage.primary_label.as_deref().unwrap_or("5h") {
        "5-hour" | "5-hour window" => "5h",
        "7-day" | "7-day window" => "7d",
        label => label,
    };
    Some(format!(
        " · {label} [{}{}] {:.0}% left",
        "█".repeat(filled),
        "░".repeat(4 - filled),
        remaining * 100.0
    ))
}

fn detail(dot: &str, label: String, ready: bool) -> Line<'static> {
    Line::from(vec![
        Span::raw("  "),
        Span::styled(
            dot.to_owned(),
            Style::default().fg(if ready {
                auth_dot_color(AuthState::Available)
            } else {
                dim_color()
            }),
        ),
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
    fn router_inventory_follows_global_pool_order_without_promoting_active() {
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
        assert_eq!(lines[3].spans[1].content, "●");
        assert_eq!(lines[0].spans[1].content, "○");
        assert_eq!(rendered.matches('●').count(), 1);
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
        assert_eq!(lines[0].spans[1].content, "×");
        assert_eq!(lines[0].spans[1].style.fg, Some(dim_color()));
        assert_eq!(lines[1].spans[1].content, "●");
        assert_eq!(
            lines[1].spans[1].style.fg,
            Some(auth_dot_color(AuthState::Available))
        );
        assert!(!text(lines).contains("API key"));
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
        assert_eq!(
            remaining_bar(&usage).as_deref(),
            Some(" · 5h [██░░] 52% left")
        );
        for ratio in [None, Some(f32::NAN), Some(-0.1), Some(1.1)] {
            usage.five_hour_ratio = ratio;
            assert!(remaining_bar(&usage).is_none());
        }
        usage.five_hour_ratio = Some(0.0);
        usage.error = Some("unavailable".into());
        assert!(remaining_bar(&usage).is_none());
    }
}
