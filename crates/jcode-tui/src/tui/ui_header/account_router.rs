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
    let mut lines = Vec::new();
    let runtime = std::env::var("JCODE_RUNTIME_PROVIDER").ok();
    let current_provider = jcode_provider_core::parse_provider_hint(provider_name);
    for (name, id, provider, state) in [
        (
            "anthropic",
            "claude",
            ActiveProvider::Claude,
            auth.anthropic.state,
        ),
        ("openai", "openai", ActiveProvider::OpenAI, auth.openai),
    ] {
        let accounts: Vec<_> = inventory
            .logins
            .iter()
            .filter(|a| a.provider == id)
            .collect();
        let resolved = resolve_dual_credential_auth(provider, auth, runtime.as_deref());
        if accounts.is_empty() && state == AuthState::NotConfigured {
            continue;
        }
        lines.push(Line::styled(name, Style::default().fg(dim_color())));
        let credential = active.get(provider).or(resolved.map(|r| r.active));
        let bound = (current_provider == Some(provider))
            .then_some(selected_label)
            .flatten();
        let oauth_state = if provider == ActiveProvider::Claude {
            auth.anthropic.oauth_state
        } else {
            auth.openai_oauth_state
        };
        let mut accounts = accounts;
        accounts.sort_by_key(|a| !bound.map_or(a.active, |label| a.label == label));
        for account in &accounts {
            let selected = bound.map_or(account.active, |label| account.label == label);
            let pooled = inventory
                .pool
                .is_member(&account_key(id, Some(&account.label)), true);
            let expired = inventory
                .expired
                .iter()
                .any(|(p, label)| *p == id && label == &account.label)
                || (selected && bound.is_none() && oauth_state == AuthState::Expired);
            let status = if selected && credential == Some(ActiveCredential::OAuth) {
                if expired {
                    "active · expired"
                } else {
                    "active"
                }
            } else if expired {
                "saved · expired"
            } else if pooled {
                "standby"
            } else {
                "saved · manual"
            };
            let usage_provider = if provider == ActiveProvider::Claude {
                crate::usage::MultiAccountProviderKind::Anthropic
            } else {
                crate::usage::MultiAccountProviderKind::OpenAI
            };
            let usage = crate::usage::cached_account_usage_snapshot(usage_provider, &account.label);
            let quota = usage.as_ref().and_then(remaining_bar).unwrap_or_default();
            lines.push(detail(
                if selected && credential == Some(ActiveCredential::OAuth) {
                    "●"
                } else {
                    "○"
                },
                format!(
                    "{} · {status}{quota}",
                    masked_identity(account.email.as_deref())
                ),
                selected
                    && !expired
                    && credential == Some(ActiveCredential::OAuth)
                    && (bound.is_some() || oauth_state == AuthState::Available),
            ));
        }
        if let Some(resolved) = resolved {
            if resolved.has_oauth && accounts.is_empty() {
                lines.push(detail("◐", "OAuth · identity unavailable".into(), false));
            }
            if resolved.has_api_key {
                let key = if provider == ActiveProvider::Claude {
                    "anthropic-api"
                } else {
                    "openai-api"
                };
                let status = if credential == Some(ActiveCredential::ApiKey) {
                    "active"
                } else if inventory.pool.is_member(key, false) {
                    "fallback · auto-switch enabled"
                } else {
                    "available · manual fallback"
                };
                lines.push(detail("●", format!("API key · {status}"), true));
            }
        }
    }
    for (name, state) in [
        ("jcode", auth.jcode),
        ("azure", auth.azure),
        ("bedrock", auth.bedrock),
        ("grok build", auth.grok_build),
        ("OpenAI-compatible profiles", auth.openai_compatible_any),
        ("openrouter · API key", auth.openrouter),
        ("cursor", auth.cursor),
        ("copilot", auth.copilot),
        ("gemini", auth.gemini),
        ("antigravity", auth.antigravity),
    ] {
        if state != AuthState::NotConfigured {
            lines.push(detail(
                auth_dot_char(state),
                format!(
                    "{name} · {}",
                    if state == AuthState::Available {
                        "ready"
                    } else {
                        "expired"
                    }
                ),
                state == AuthState::Available,
            ));
        }
    }
    if lines.is_empty() {
        lines.push(Line::styled(
            "No cached accounts · /login to add credential",
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
    let label = usage.primary_label.as_deref().unwrap_or("5h");
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

    #[test]
    fn router_inventory_masks_and_honors_bound_account_and_manual_keys() {
        let auth = AuthStatus {
            anthropic: ProviderAuth {
                state: AuthState::Available,
                has_oauth: true,
                oauth_state: AuthState::Available,
                has_api_key: true,
            },
            openrouter: AuthState::Available,
            ..Default::default()
        };
        let inventory = Inventory {
            logins: vec![
                OAuthLogin {
                    provider: "claude",
                    label: "one".into(),
                    email: Some("alice@example.com".into()),
                    active: true,
                },
                OAuthLogin {
                    provider: "claude",
                    label: "two".into(),
                    email: Some("bob@example.com".into()),
                    active: false,
                },
            ],
            ..Default::default()
        };
        let active = ActiveCredentialOverrides {
            anthropic: Some(ActiveCredential::OAuth),
            openai: None,
        };
        let rendered = text(inventory_lines(
            &auth,
            active,
            &inventory,
            "anthropic",
            Some("two"),
        ));
        assert!(
            rendered.contains("b***b@example.com · active"),
            "{rendered}"
        );
        assert!(rendered.contains("a***e@example.com · standby"));
        assert!(rendered.contains("API key · available · manual fallback"));
        assert!(rendered.contains("openrouter · API key · ready"));
        assert!(!rendered.contains("alice"));
        assert!(!rendered.contains('%'));
    }

    #[test]
    fn router_inventory_respects_exclusions_and_key_pool_membership() {
        let auth = AuthStatus {
            openai: AuthState::Available,
            openai_has_oauth: true,
            openai_oauth_state: AuthState::Available,
            openai_has_api_key: true,
            ..Default::default()
        };
        let inventory = Inventory {
            logins: vec![OAuthLogin {
                provider: "openai",
                label: "one".into(),
                email: None,
                active: false,
            }],
            pool: AccountPool {
                members: vec!["openai-api".into()],
                excluded: vec!["openai:one".into()],
            },
            ..Default::default()
        };
        let active = ActiveCredentialOverrides {
            anthropic: None,
            openai: Some(ActiveCredential::OAuth),
        };
        let rendered = text(inventory_lines(&auth, active, &inventory, "claude", None));
        assert!(rendered.contains("saved account · saved · manual"));
        assert!(rendered.contains("API key · fallback · auto-switch enabled"));
        assert!(!rendered.contains("standby"));
    }

    #[test]
    fn router_inventory_expiry_is_account_scoped_and_keys_can_be_active() {
        let auth = AuthStatus {
            openai: AuthState::Available,
            openai_has_oauth: true,
            openai_oauth_state: AuthState::Expired,
            openai_has_api_key: true,
            ..Default::default()
        };
        let inventory = Inventory {
            logins: vec![
                OAuthLogin {
                    provider: "openai",
                    label: "old".into(),
                    email: Some("old@example.com".into()),
                    active: true,
                },
                OAuthLogin {
                    provider: "openai",
                    label: "bound".into(),
                    email: Some("new@example.com".into()),
                    active: false,
                },
            ],
            expired: vec![("openai", "old".into())],
            ..Default::default()
        };
        let active = ActiveCredentialOverrides {
            anthropic: None,
            openai: Some(ActiveCredential::OAuth),
        };
        let rendered = text(inventory_lines(
            &auth,
            active,
            &inventory,
            "openai",
            Some("bound"),
        ));
        assert!(rendered.contains("n***w@example.com · active"));
        assert!(!rendered.contains("n***w@example.com · active · expired"));
        assert!(rendered.contains("o***d@example.com · saved · expired"));
        let active = ActiveCredentialOverrides {
            anthropic: None,
            openai: Some(ActiveCredential::ApiKey),
        };
        let rendered = text(inventory_lines(
            &auth,
            active,
            &inventory,
            "openai",
            Some("bound"),
        ));
        assert!(rendered.contains("API key · active"));
        assert!(!rendered.contains("n***w@example.com · active"));
    }

    #[test]
    fn router_inventory_preserves_other_configured_providers() {
        let auth = AuthStatus {
            jcode: AuthState::Available,
            azure: AuthState::Available,
            bedrock: AuthState::Available,
            grok_build: AuthState::Available,
            openai_compatible_any: AuthState::Available,
            ..Default::default()
        };
        let rendered = text(inventory_lines(
            &auth,
            ActiveCredentialOverrides::default(),
            &Inventory::default(),
            "",
            None,
        ));
        for provider in [
            "jcode",
            "azure",
            "bedrock",
            "grok build",
            "OpenAI-compatible profiles",
        ] {
            assert!(rendered.contains(provider), "{rendered}");
        }
        assert!(!rendered.contains("No cached accounts"));
    }

    #[test]
    fn router_inventory_remaining_bar_rejects_unknown_and_invalid_usage() {
        let mut usage = crate::usage::AccountUsageSnapshot {
            label: "one".into(),
            email: None,
            exhausted: false,
            primary_label: Some("5h".into()),
            five_hour_ratio: Some(0.25),
            secondary_label: None,
            seven_day_ratio: None,
            resets_at: None,
            error: None,
        };
        assert_eq!(
            remaining_bar(&usage).as_deref(),
            Some(" · 5h [███░] 75% left")
        );
        for ratio in [None, Some(f32::NAN), Some(1.1), Some(-0.1)] {
            usage.five_hour_ratio = ratio;
            assert!(remaining_bar(&usage).is_none());
        }
        usage.five_hour_ratio = Some(0.0);
        usage.error = Some("unavailable".into());
        assert!(remaining_bar(&usage).is_none());
    }

    #[test]
    fn router_inventory_handles_unknown_identity_and_empty_auth() {
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
        assert!(rendered.contains("/login"));
        assert!(!rendered.contains("ready"));
    }

    #[test]
    fn router_inventory_expired_oauth_never_borrows_key_readiness() {
        let auth = AuthStatus {
            openai: AuthState::Available,
            openai_has_oauth: true,
            openai_oauth_state: AuthState::Expired,
            openai_has_api_key: true,
            ..Default::default()
        };
        let inventory = Inventory {
            logins: vec![OAuthLogin {
                provider: "openai",
                label: "one".into(),
                email: Some("alice@example.com".into()),
                active: true,
            }],
            expired: vec![("openai", "one".into())],
            ..Default::default()
        };
        let active = ActiveCredentialOverrides {
            anthropic: None,
            openai: Some(ActiveCredential::ApiKey),
        };
        let lines = inventory_lines(&auth, active, &inventory, "openai", Some("one"));
        let rendered = text(lines.clone());
        assert!(rendered.contains("saved · expired"), "{rendered}");
        assert!(rendered.contains("API key · active"));
        assert!(!rendered.contains("standby"));
        assert_eq!(lines[1].spans[1].style.fg, Some(dim_color()));
    }

    #[test]
    fn router_inventory_keeps_other_configured_providers() {
        let auth = AuthStatus {
            jcode: AuthState::Available,
            azure: AuthState::Available,
            bedrock: AuthState::Available,
            grok_build: AuthState::Available,
            openai_compatible_any: AuthState::Available,
            ..Default::default()
        };
        let rendered = text(inventory_lines(
            &auth,
            ActiveCredentialOverrides::default(),
            &Inventory::default(),
            "",
            None,
        ));
        for name in [
            "jcode",
            "azure",
            "bedrock",
            "grok build",
            "OpenAI-compatible profiles",
        ] {
            assert!(rendered.contains(name), "{rendered}");
        }
        assert!(!rendered.contains("No cached accounts"));
    }
}
