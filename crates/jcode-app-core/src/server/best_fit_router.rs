//! Best-fit model routing driven by one declarative policy file
//! (`routing-policy.json`, see waterlux-claude-config). Provider-, model- and
//! family-agnostic: every decision reads live `ModelRoute` metadata only.

use jcode_provider_core::{ModelRoute, RouteBillingKind};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::net::ToSocketAddrs;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub(crate) const PARENT_TIER: &str = "deep_architecture";
pub(crate) const SWARM_TIER: &str = "parallel_shard";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Billing {
    Oauth,
    Subscription,
    Local,
    MeteredApproved,
    Metered,
    Browser,
    Unknown,
}

impl Billing {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Oauth => "oauth",
            Self::Subscription => "subscription",
            Self::Local => "local",
            Self::MeteredApproved => "metered_approved",
            Self::Metered => "metered",
            Self::Browser => "browser",
            Self::Unknown => "unknown",
        }
    }
}

pub(crate) struct Policy {
    raw: Value,
}

fn strs(v: &Value, ptr: &str) -> Vec<String> {
    match v.pointer(ptr).and_then(Value::as_array) {
        Some(a) => a
            .iter()
            .filter_map(|s| s.as_str().map(str::to_lowercase))
            .collect(),
        None => Vec::new(),
    }
}

fn policy_path() -> Option<std::path::PathBuf> {
    match std::env::var("JCODE_ROUTING_POLICY") {
        Ok(p) if p == "off" => None,
        Ok(p) if !p.is_empty() => Some(p.into()),
        _ => dirs::home_dir().map(|h| h.join("code/waterlux-claude-config/routing-policy.json")),
    }
}

impl Policy {
    pub(crate) fn from_json(text: &str) -> Option<Self> {
        let raw: Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(e) => {
                crate::logging::warn(&format!("routing-policy.json invalid: {e}"));
                return None;
            }
        };
        raw.get("tiers")?.as_object()?;
        Some(Self { raw })
    }

    /// Re-read on each call: the file is small and edits must apply without a restart.
    pub(crate) fn load() -> Option<Self> {
        match std::fs::read_to_string(policy_path()?) {
            Ok(text) => Self::from_json(&text),
            Err(_) => None, // no policy file: routing stays on the existing defaults
        }
    }

    fn class_has(&self, class: &str, key: &str, hay: &str) -> bool {
        strs(&self.raw, &format!("/billing/classify/{class}/{key}"))
            .iter()
            .any(|needle| hay.contains(needle.as_str()))
    }

    pub(crate) fn classify(&self, route: &ModelRoute, approved: &HashSet<String>) -> Billing {
        let api = route.api_method.to_lowercase();
        let detail = route.detail.to_lowercase();
        let kind = route.cheapness.as_ref().map(|c| match c.billing_kind {
            RouteBillingKind::Metered => "metered",
            RouteBillingKind::Subscription => "subscription",
            RouteBillingKind::IncludedQuota => "included_quota",
            RouteBillingKind::Unknown => "unknown",
        });
        let kind_is = |class: &str| {
            kind.is_some_and(|k| {
                strs(
                    &self.raw,
                    &format!("/billing/classify/{class}/billing_kind"),
                )
                .iter()
                .any(|x| x == k)
            })
        };
        if self.class_has("browser", "api_method_contains", &api) {
            return Billing::Browser;
        }
        if self.class_has("oauth", "api_method_contains", &api) {
            return Billing::Oauth;
        }
        if self.class_has("local", "detail_contains", &detail) {
            return Billing::Local;
        }
        if kind_is("subscription") {
            return Billing::Subscription;
        }
        // An operator approval in provider config outranks the metered/unknown default.
        if approved
            .iter()
            .any(|p| api.ends_with(&format!(":{p}")) || api == *p)
        {
            return Billing::MeteredApproved;
        }
        if kind_is("metered") {
            return Billing::Metered;
        }
        Billing::Unknown
    }

    pub(crate) fn allowed(&self, b: Billing) -> bool {
        let allow = strs(&self.raw, "/billing/allow");
        allow.iter().any(|a| a == b.as_str())
            || (b == Billing::MeteredApproved
                && self
                    .raw
                    .pointer("/billing/allow_metered_when")
                    .and_then(Value::as_str)
                    == Some("metered_approved"))
    }

    fn tier(&self, tier: &str) -> Option<&Value> {
        self.raw.pointer(&format!("/tiers/{tier}"))
    }

    pub(crate) fn context_window(&self, route: &ModelRoute) -> Option<u64> {
        let pat = self
            .raw
            .pointer("/capability_parse/context_window_regex")?
            .as_str()?;
        let re = match regex::Regex::new(pat) {
            Ok(re) => re,
            Err(_) => return None,
        };
        let digits = re.captures(&route.detail)?.get(1)?.as_str();
        // Regex group is all digits, so only overflow can fail; treat it as unknown.
        match digits.parse::<u64>() {
            Ok(n) => Some(n),
            Err(e) => {
                crate::logging::warn(&format!("route context window unparsable: {e}"));
                None
            }
        }
    }

    fn supports_tools(&self, route: &ModelRoute) -> bool {
        let d = route.detail.to_lowercase();
        !strs(&self.raw, "/capability_parse/no_tools_contains")
            .iter()
            .any(|n| d.contains(n.as_str()))
    }

    /// Rank eligible routes for `tier`, best first. `probe` decides local warmth.
    pub(crate) fn rank<'a>(
        &self,
        tier: &str,
        routes: &'a [ModelRoute],
        approved: &HashSet<String>,
        payload_tokens: u64,
        excluded: &HashSet<String>,
        probe: &dyn Fn(&ModelRoute) -> bool,
    ) -> Vec<(&'a ModelRoute, Billing)> {
        let Some(t) = self.tier(tier) else {
            return Vec::new();
        };
        let prefer = strs(t, "/prefer");
        let need_tools = t
            .get("supports_tools")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let ctx_floor = t
            .get("min_context_over_payload")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let deep = t.get("rank").and_then(Value::as_str) == Some("highest_reasoning_then_context");
        let mut out: Vec<_> = routes
            .iter()
            .filter(|r| r.available && !excluded.contains(&route_id(r)))
            .map(|r| (r, self.classify(r, approved)))
            .filter(|(_, b)| self.allowed(*b) && prefer.iter().any(|p| p == b.as_str()))
            .filter(|(r, _)| !need_tools || self.supports_tools(r))
            .filter(|(r, _)| {
                !ctx_floor || self.context_window(r).is_none_or(|c| c > payload_tokens)
            })
            // A local route is only eligible while its endpoint answers within 2s.
            .filter(|(r, b)| *b != Billing::Local || probe(r))
            .collect();
        let pos = |b: Billing| {
            prefer
                .iter()
                .position(|p| p == b.as_str())
                .unwrap_or(usize::MAX)
        };
        let cost = |r: &ModelRoute| {
            r.cheapness
                .as_ref()
                .and_then(|c| c.estimated_reference_cost_micros)
                .unwrap_or(0)
        };
        out.sort_by(|(ra, ba), (rb, bb)| {
            pos(*ba).cmp(&pos(*bb)).then_with(|| {
                if deep {
                    // Reasoning proxy: higher reference cost, then larger context.
                    cost(rb)
                        .cmp(&cost(ra))
                        .then(self.context_window(rb).cmp(&self.context_window(ra)))
                } else {
                    cost(ra).cmp(&cost(rb))
                }
            })
        });
        out
    }

    pub(crate) fn step_down_on(&self) -> Vec<String> {
        strs(&self.raw, "/stickiness/step_down_on")
    }
}

pub(crate) fn route_id(r: &ModelRoute) -> String {
    format!("{}@{}", r.model, r.api_method)
}

/// Provider profiles the operator marked `metered_approved = true`.
/// Read raw from config.toml (`[providers.<name>] metered_approved = true`) so the
/// typed provider config stays untouched; unknown keys are ignored there.
pub(crate) fn approved_profiles() -> HashSet<String> {
    let Some(path) = crate::config::Config::path() else {
        return HashSet::new();
    };
    match std::fs::read_to_string(path) {
        Ok(text) => approved_from_toml(&text),
        Err(_) => HashSet::new(), // no config file: nothing is approved
    }
}

pub(crate) fn approved_from_toml(text: &str) -> HashSet<String> {
    let Ok(doc) = text.parse::<toml::Table>() else {
        return HashSet::new();
    };
    doc.get("providers")
        .and_then(|p| p.as_table())
        .into_iter()
        .flatten()
        .filter(|(_, v)| {
            ["metered_approved", "metered-approved"]
                .iter()
                .any(|k| v.get(*k).and_then(toml::Value::as_bool) == Some(true))
        })
        .map(|(k, _)| k.to_lowercase())
        .collect()
}

/// `<2s` TCP connect to the first `http(s)://host:port` in the route detail.
pub(crate) fn local_probe(route: &ModelRoute) -> bool {
    static CACHE: OnceLock<Mutex<HashMap<String, (Instant, bool)>>> = OnceLock::new();
    let Some(addr) = route.detail.split_whitespace().find_map(|w| {
        let rest = w
            .trim_matches(|c| c == '(' || c == ')' || c == ';' || c == ',')
            .split_once("://")?
            .1;
        let hostport = rest.split('/').next()?;
        Some(if hostport.contains(':') {
            hostport.to_string()
        } else {
            format!("{hostport}:80")
        })
    }) else {
        return false;
    };
    let cache = CACHE.get_or_init(Default::default);
    let cached = match cache.lock() {
        Ok(c) => c.get(&addr).copied(),
        Err(_) => None,
    };
    if let Some((at, ok)) = cached
        && at.elapsed() < Duration::from_secs(30)
    {
        return ok;
    }
    let ok = match addr.to_socket_addrs() {
        Ok(mut a) => a.next(),
        Err(_) => None, // unresolvable host counts as cold
    }
    .is_some_and(|sa| std::net::TcpStream::connect_timeout(&sa, Duration::from_secs(2)).is_ok());
    if let Ok(mut c) = cache.lock() {
        c.insert(addr, (Instant::now(), ok));
    }
    ok
}

type PinKey = (String, String);
/// (pinned route id, routes excluded after step-down)
type PinState = (String, HashSet<String>);
fn pins() -> &'static Mutex<HashMap<PinKey, PinState>> {
    static PINS: OnceLock<Mutex<HashMap<PinKey, PinState>>> = OnceLock::new();
    PINS.get_or_init(Default::default)
}

/// Sticky best-fit pick: reuse the session's pinned route for this tier while it
/// stays eligible, so mid-session dispatches never invalidate the KV cache.
pub(crate) fn pick(
    policy: &Policy,
    session_id: &str,
    tier: &str,
    routes: &[ModelRoute],
    probe: &dyn Fn(&ModelRoute) -> bool,
) -> Option<(ModelRoute, Billing, bool)> {
    let approved = approved_profiles();
    let key = (session_id.to_string(), tier.to_string());
    let Ok(mut map) = pins().lock() else {
        return None;
    };
    let entry = map
        .entry(key)
        .or_insert_with(|| (String::new(), HashSet::new()));
    let ranked = policy.rank(tier, routes, &approved, 0, &entry.1, probe);
    if let Some((r, b)) = ranked.iter().find(|(r, _)| route_id(r) == entry.0) {
        return Some(((*r).clone(), *b, true));
    }
    let (r, b) = ranked.first()?;
    entry.0 = route_id(r);
    Some(((*r).clone(), *b, false))
}

/// Step down: drop the pin and exclude the failed route, on a policy-listed reason.
pub(crate) fn step_down(policy: &Policy, session_id: &str, tier: &str, reason: &str) -> bool {
    if !policy
        .step_down_on()
        .iter()
        .any(|r| r == &reason.to_lowercase())
    {
        return false;
    }
    let Ok(mut map) = pins().lock() else {
        return false;
    };
    let Some(entry) = map.get_mut(&(session_id.to_string(), tier.to_string())) else {
        return false;
    };
    let max = policy
        .raw
        .pointer("/stickiness/max_step_downs")
        .and_then(Value::as_u64)
        .unwrap_or(3);
    if entry.1.len() as u64 >= max || entry.0.is_empty() {
        return false;
    }
    let failed = std::mem::take(&mut entry.0);
    entry.1.insert(failed);
    true
}

/// Map a provider error string onto a policy step-down reason.
pub(crate) fn step_down_reason(err: &str) -> Option<&'static str> {
    let e = err.to_lowercase();
    if e.contains("429") || e.contains("rate limit") {
        Some("429")
    } else if e.contains("quota")
        || e.contains("usage limit")
        || e.contains("cap") && e.contains("exhaust")
    {
        Some("cap_exhausted")
    } else if e.contains("timed out") || e.contains("timeout") {
        Some("timeout")
    } else {
        None
    }
}

pub(crate) fn telemetry(event: Value) {
    let Some(path) = dirs::home_dir().map(|h| h.join(".jcode/logs/hook-telemetry.jsonl")) else {
        return;
    };
    let mut row = serde_json::json!({
        "ts": chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        "hook": "best_fit_router",
    });
    if let (Some(o), Value::Object(e)) = (row.as_object_mut(), event) {
        o.extend(e);
    }
    let written = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut f| writeln!(f, "{row}"));
    if let Err(e) = written {
        crate::logging::warn(&format!("best-fit telemetry write failed: {e}"));
    }
}

#[cfg(test)]
#[path = "best_fit_router_tests.rs"]
mod tests;
