//! Startup-only credential inventory. No credential probes or usage refreshes.
use super::*;
#[path = "account_router_inventory.rs"]
mod inventory;
pub(super) use inventory::fit_account_row;
use inventory::inventory_lines;
use inventory::load_inventory;
#[cfg(test)]
use inventory::remaining_bars;
use inventory::remote_inventory_lines;

use crate::auth::account_pool::{AccountPool, OAuthLogin, account_key};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

static GENERATION: AtomicU64 = AtomicU64::new(0);

pub(super) fn generation() -> u64 {
    let _ = cached_inventory();
    GENERATION.load(Ordering::Acquire)
}

#[derive(Default, PartialEq, Eq)]
struct Inventory {
    logins: Vec<OAuthLogin>,
    pool: AccountPool,
    expired: Vec<(&'static str, String)>,
    /// Antigravity quota pools from the persisted model catalog, read on the
    /// inventory worker so rendering never touches disk.
    antigravity: Option<AntigravityQuota>,
}

#[derive(Default, PartialEq, Eq, Clone, Debug)]
struct AntigravityQuota {
    email: Option<String>,
    /// (pool label, used per-mille, reset time) per quota pool.
    pools: Vec<(String, u16, Option<String>)>,
}

#[derive(Default)]
struct InventoryCache {
    inventory: Arc<Inventory>,
    updated: Option<Instant>,
    refreshing: bool,
}

impl InventoryCache {
    fn snapshot(&self) -> Arc<Inventory> {
        Arc::clone(&self.inventory)
    }

    fn replace_inventory(&mut self, inventory: Inventory) -> bool {
        if *self.inventory == inventory {
            return false;
        }
        self.inventory = Arc::new(inventory);
        true
    }
}

fn cached_inventory() -> Arc<Inventory> {
    static CACHE: OnceLock<Mutex<InventoryCache>> = OnceLock::new();
    static EMPTY: OnceLock<Arc<Inventory>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let Ok(mut state) = cache.try_lock() else {
        return Arc::clone(EMPTY.get_or_init(Default::default));
    };
    let inventory = state.snapshot();
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
                    if state.replace_inventory(inventory) {
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

const QUOTA_SEPARATOR: &str = "  ";

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
