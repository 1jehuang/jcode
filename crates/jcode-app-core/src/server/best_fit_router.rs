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
pub(crate) const CODE_TIER: &str = "code_execution";
pub(crate) const LOCAL_TIER: &str = "lightweight_local";
pub(crate) const ALL_TIERS: [&str; 4] = [PARENT_TIER, CODE_TIER, SWARM_TIER, LOCAL_TIER];

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
        if self.is_true_local(route) || self.class_has("local", "detail_contains", &detail) {
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

    /// Loopback host in the route detail AND no remote/cloud/proxy marker anywhere.
    pub(crate) fn is_true_local(&self, route: &ModelRoute) -> bool {
        let detail = route.detail.to_lowercase();
        let all = format!("{} {} {}", route.model, route.api_method, route.detail).to_lowercase();
        strs(&self.raw, "/local_detection/loopback_hosts")
            .iter()
            .any(|h| detail.contains(&format!("//{h}")))
            && !strs(&self.raw, "/local_detection/remote_markers")
                .iter()
                .any(|m| all.contains(m.as_str()))
    }

    fn num(&self, ptr: &str) -> Option<u64> {
        self.raw.pointer(ptr).and_then(Value::as_u64)
    }

    /// Spawn prompt -> tier: a mutating marker wins, else read-only by file count.
    pub(crate) fn classify_spawn(&self, prompt: &str) -> &'static str {
        let p = prompt.to_lowercase();
        let tier = |k: &str, d: &'static str| -> &'static str {
            let want = self
                .raw
                .pointer(&format!("/swarm_classification/{k}"))
                .and_then(Value::as_str);
            ALL_TIERS
                .into_iter()
                .find(|t| Some(*t) == want)
                .unwrap_or(d)
        };
        if strs(&self.raw, "/swarm_classification/mutating_markers")
            .iter()
            .any(|m| p.contains(m.as_str()))
        {
            return tier("mutating_tier", CODE_TIER);
        }
        let files: HashSet<&str> = self
            .raw
            .pointer("/swarm_classification/file_token_regex")
            .and_then(Value::as_str)
            .and_then(|r| regex::Regex::new(r).ok())
            .map(|re| re.find_iter(&p).map(|m| m.as_str()).collect())
            .unwrap_or_default();
        let small = self
            .num("/swarm_classification/small_max_files")
            .unwrap_or(3) as usize;
        if !files.is_empty() && files.len() <= small {
            tier("small_read_only_tier", LOCAL_TIER)
        } else {
            tier("read_only_tier", SWARM_TIER)
        }
    }

    /// Rough token estimate for payload-aware context escalation.
    pub(crate) fn estimate_tokens(&self, text: &str) -> u64 {
        let cpt = self
            .num("/invariants/context_escalation/chars_per_token")
            .unwrap_or(4)
            .max(1);
        text.len() as u64 / cpt
    }

    fn escalation(&self) -> f64 {
        self.raw
            .pointer("/invariants/context_escalation/multiplier")
            .and_then(Value::as_f64)
            .unwrap_or(1.5)
    }

    pub(crate) fn spill_to(&self, tier: &str) -> Option<&'static str> {
        let want = self.tier(tier)?.get("spill_to")?.as_str()?;
        ALL_TIERS.into_iter().find(|t| *t == want)
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
        let min_ctx = t
            .get("min_context_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        // Payload-aware escalation: window must be >= estimated tokens * multiplier.
        let need_ctx = min_ctx.max((payload_tokens as f64 * self.escalation()).ceil() as u64);
        let deep = t.get("rank").and_then(Value::as_str) == Some("highest_reasoning_then_context");
        let mut out: Vec<_> = routes
            .iter()
            .filter(|r| r.available && !excluded.contains(&route_id(r)))
            .map(|r| (r, self.classify(r, approved)))
            .filter(|(_, b)| self.allowed(*b) && prefer.iter().any(|p| p == b.as_str()))
            .filter(|(r, _)| !need_tools || self.supports_tools(r))
            // Unknown window passes only when no floor applies.
            // shortcut: an unadvertised window passes the static floor but never a payload
            // escalation above it; tighten once every route advertises its window.
            .filter(|(r, _)| match self.context_window(r) {
                Some(c) => c >= need_ctx,
                None => need_ctx <= min_ctx,
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
/// Single-tier form of [`dispatch`], kept for the pin/step-down tests.
#[cfg(test)]
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

/// Cross-process router state shared by every jcode window:
/// `~/.jcode/state/router-state.json`, guarded by an flock on a sibling lock file.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize, PartialEq)]
pub(crate) struct SharedState {
    /// local route id -> (holder pid, lease expiry unix s) for max_concurrent_local.
    /// shortcut: a lease expires instead of being released on worker exit; wire an
    /// explicit release when swarm member completion reaches this module.
    #[serde(default)]
    pub local_slots: HashMap<String, (u32, u64)>,
    /// route id -> unix seconds until which the breaker is open
    #[serde(default)]
    pub breakers: HashMap<String, u64>,
    /// session id -> metered tokens dispatched
    #[serde(default)]
    pub metered_tokens: HashMap<String, u64>,
}

fn now_s() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn pid_alive(pid: u32) -> bool {
    // kill(pid, 0): 0 = alive, EPERM = alive but not ours.
    let alive = unsafe { ::libc::kill(pid as ::libc::pid_t, 0) == 0 };
    alive || std::io::Error::last_os_error().raw_os_error() == Some(::libc::EPERM)
}

pub(crate) fn state_path() -> Option<std::path::PathBuf> {
    match std::env::var("JCODE_ROUTER_STATE") {
        Ok(p) if !p.is_empty() => Some(p.into()),
        _ => dirs::home_dir().map(|h| h.join(".jcode/state/router-state.json")),
    }
}

/// Run `f` on the shared state under an exclusive flock; drops dead-pid slots
/// and expired breakers first. `None` when the state file is unreachable.
pub(crate) fn with_state<T>(f: impl FnOnce(&mut SharedState) -> T) -> Option<T> {
    use std::os::unix::io::AsRawFd;
    let path = state_path()?;
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path.with_extension("lock"))
        .ok()?;
    if unsafe { ::libc::flock(lock.as_raw_fd(), ::libc::LOCK_EX) } != 0 {
        return None;
    }
    // A corrupt or missing file starts fresh; the lock still serializes writers.
    let mut st: SharedState = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let now = now_s();
    st.local_slots
        .retain(|_, (pid, until)| *until > now && pid_alive(*pid));
    st.breakers.retain(|_, until| *until > now);
    let out = f(&mut st);
    let tmp = path.with_extension("tmp");
    let written = serde_json::to_vec_pretty(&st)
        .map_err(std::io::Error::other)
        .and_then(|b| std::fs::write(&tmp, b))
        .and_then(|_| std::fs::rename(&tmp, &path));
    if let Err(e) = written {
        crate::logging::warn(&format!("router-state write failed: {e}"));
    }
    unsafe { ::libc::flock(lock.as_raw_fd(), ::libc::LOCK_UN) };
    Some(out)
}

impl Policy {
    fn breaker_reasons(&self) -> Vec<String> {
        strs(&self.raw, "/invariants/circuit_breaker/on")
    }

    /// Open the cross-process breaker for `route` on a listed reason (429/timeout).
    pub(crate) fn trip(&self, route: &str, reason: &str) -> bool {
        if !self
            .breaker_reasons()
            .iter()
            .any(|r| r == &reason.to_lowercase())
        {
            return false;
        }
        let until = now_s()
            + self
                .num("/invariants/circuit_breaker/cooldown_s")
                .unwrap_or(120);
        with_state(|st| st.breakers.insert(route.to_string(), until)).is_some()
    }

    fn max_local(&self) -> usize {
        self.num(&format!("/tiers/{LOCAL_TIER}/max_concurrent_local"))
            .unwrap_or(1) as usize
    }

    fn budget(&self) -> Option<u64> {
        self.num("/invariants/budget/max_session_metered_tokens")
    }

    fn over_budget_allowed(&self, b: Billing) -> bool {
        strs(&self.raw, "/invariants/budget/over_budget_billing")
            .iter()
            .any(|x| x == b.as_str())
    }

    /// Env override for a tier: `JCODE_ROUTE_TIER_MODEL_<TIER>` (upper-case tier).
    pub(crate) fn env_override(&self, tier: &str) -> Option<String> {
        let prefix = self
            .raw
            .pointer("/invariants/env_override")
            .and_then(Value::as_str)
            .unwrap_or("JCODE_ROUTE_TIER_MODEL_")
            .trim_end_matches("<TIER>");
        std::env::var(format!("{prefix}{}", tier.to_uppercase()))
            .ok()
            .filter(|v| !v.trim().is_empty())
    }
}

/// Live catalog with a policy TTL (`invariants.catalog.list_models_ttl_s`, 60s):
/// reuse `cached` routes for `key` until stale, else store `fresh`.
pub(crate) fn catalog_ttl(
    policy: &Policy,
    key: &str,
    fresh: impl FnOnce() -> Vec<ModelRoute>,
) -> Vec<ModelRoute> {
    type Catalog = Mutex<HashMap<String, (Instant, Vec<ModelRoute>)>>;
    static CAT: OnceLock<Catalog> = OnceLock::new();
    let ttl = Duration::from_secs(
        policy
            .num("/invariants/catalog/list_models_ttl_s")
            .unwrap_or(60),
    );
    let cat = CAT.get_or_init(Default::default);
    if let Ok(c) = cat.lock()
        && let Some((at, r)) = c.get(key)
        && at.elapsed() < ttl
    {
        return r.clone();
    }
    let routes = fresh();
    if let Ok(mut c) = cat.lock() {
        c.insert(key.to_string(), (Instant::now(), routes.clone()));
    }
    routes
}

/// Outcome of a tiered dispatch.
#[derive(Debug, Clone)]
pub(crate) struct Dispatch {
    pub route: ModelRoute,
    pub billing: Billing,
    pub tier: &'static str,
    pub requested_tier: &'static str,
    pub sticky: bool,
    pub over_budget: bool,
    pub payload_tokens: u64,
}

/// Full 4-tier dispatch: classify the spawn prompt, apply breakers, budget,
/// local slot limits and payload escalation, spilling a full/cold local tier
/// to its `spill_to` tier. Records slot holder pid and metered spend.
pub(crate) fn dispatch(
    policy: &Policy,
    session_id: &str,
    prompt: &str,
    routes: &[ModelRoute],
    probe: &dyn Fn(&ModelRoute) -> bool,
    pid: u32,
) -> Option<Dispatch> {
    let requested_tier = policy.classify_spawn(prompt);
    let payload_tokens = policy.estimate_tokens(prompt);
    let approved = approved_profiles();
    let max_local = policy.max_local();
    with_state(|st| {
        let spent = st.metered_tokens.get(session_id).copied().unwrap_or(0);
        let over_budget = policy
            .budget()
            .is_some_and(|cap| spent + payload_tokens > cap);
        let open: HashSet<String> = st.breakers.keys().cloned().collect();
        let held = st.local_slots.len();
        let mut tier = requested_tier;
        loop {
            let mut excluded = open.clone();
            if let Ok(map) = pins().lock()
                && let Some((_, ex)) = map.get(&(session_id.to_string(), tier.to_string()))
            {
                excluded.extend(ex.iter().cloned());
            }
            let ranked = policy.rank(tier, routes, &approved, payload_tokens, &excluded, probe);
            let pinned = pins().lock().ok().and_then(|m| {
                m.get(&(session_id.to_string(), tier.to_string()))
                    .map(|p| p.0.clone())
            });
            let ok = |(r, b): &&(&ModelRoute, Billing)| {
                (!over_budget || policy.over_budget_allowed(*b))
                    && (*b != Billing::Local
                        || held < max_local
                        || st.local_slots.get(&route_id(r)).is_some_and(|s| s.0 == pid))
            };
            let pick = ranked
                .iter()
                .filter(ok)
                .find(|(r, _)| Some(route_id(r)) == pinned)
                .map(|x| (*x, true))
                .or_else(|| ranked.iter().find(ok).map(|x| (*x, false)));
            if let Some(((r, b), sticky)) = pick {
                if let Ok(mut m) = pins().lock() {
                    m.entry((session_id.to_string(), tier.to_string()))
                        .or_insert_with(|| (String::new(), HashSet::new()))
                        .0 = route_id(r);
                }
                if b == Billing::Local {
                    let lease = policy
                        .num("/invariants/shared_state/local_lease_s")
                        .unwrap_or(600);
                    st.local_slots.insert(route_id(r), (pid, now_s() + lease));
                }
                if matches!(b, Billing::MeteredApproved | Billing::Metered) {
                    *st.metered_tokens.entry(session_id.to_string()).or_default() += payload_tokens;
                }
                return Some(Dispatch {
                    route: r.clone(),
                    billing: b,
                    tier,
                    requested_tier,
                    sticky,
                    over_budget,
                    payload_tokens,
                });
            }
            tier = policy.spill_to(tier)?;
        }
    })
    .flatten()
}

#[cfg(test)]
#[path = "best_fit_router_tests.rs"]
mod tests;
