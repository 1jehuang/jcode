//! Aggregate "many agents" status reporter for the mobile Live Activity.
//!
//! Contract: `~/jcode-mobile/docs/PUSH.md` ("Many agents (v2)"). The server
//! POSTs a full snapshot of this host's non-idle sessions to
//! `{base}/agents/status` on change (debounced), as a heartbeat every
//! [`HEARTBEAT`] while non-empty, and one final empty snapshot. Swarm workers
//! are included. Uses the same opt-out and credentials as mobile pushes, and
//! never blocks a turn.

use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const DEBOUNCE: Duration = Duration::from_secs(2);
pub const HEARTBEAT: Duration = Duration::from_secs(5 * 60);
const MAX_SESSIONS: usize = 200;
const TITLE_MAX: usize = 80;
const DETAIL_MAX: usize = 60;
const ERROR_BACKOFF_MIN: Duration = Duration::from_secs(30);
const ERROR_BACKOFF_MAX: Duration = Duration::from_secs(15 * 60);
const SEND_TIMEOUT: Duration = Duration::from_secs(5);
/// Unanswered error states drop out of the snapshot after this.
pub const ERROR_TTL: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentState {
    Running,
    Waiting,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    title: String,
    state: AgentState,
    detail: String,
    since: u64,
    /// Monotonic time the current state began (for error expiry).
    entered: Instant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionStatus {
    pub session_id: String,
    pub title: String,
    pub state: AgentState,
    pub detail: String,
    pub since: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Snapshot {
    pub host: String,
    pub sessions: Vec<SessionStatus>,
}

/// Pure state machine: session states plus debounce/heartbeat scheduling.
#[derive(Debug, Default)]
pub struct Tracker {
    sessions: HashMap<String, Entry>,
    /// First unsent change (debounce anchor).
    dirty_since: Option<Instant>,
    last_sent: Option<Instant>,
    /// Whether the last delivered snapshot was non-empty.
    last_sent_nonempty: bool,
    /// Do not send before this (error back-off).
    backoff_until: Option<Instant>,
    backoff: Option<Duration>,
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn clip(text: &str, max: usize) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= max {
        return one_line;
    }
    let mut out: String = one_line.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

impl Tracker {
    fn mark(&mut self, now: Instant) {
        self.dirty_since.get_or_insert(now);
    }

    fn set(
        &mut self,
        id: &str,
        state: AgentState,
        title: Option<&str>,
        detail: &str,
        now: Instant,
    ) {
        let since = now_unix();
        let mut changed = !self.sessions.contains_key(id);
        let entry = self
            .sessions
            .entry(id.to_string())
            .or_insert_with(|| Entry {
                title: String::new(),
                state,
                detail: String::new(),
                since,
                entered: now,
            });
        if let Some(title) = title {
            let title = clip(title, TITLE_MAX);
            if entry.title != title {
                entry.title = title;
                changed = true;
            }
        }
        if entry.state != state {
            entry.state = state;
            entry.since = since;
            entry.entered = now;
            changed = true;
        }
        let detail = clip(detail, DETAIL_MAX);
        if entry.detail != detail {
            entry.detail = detail;
            changed = true;
        }
        if changed {
            self.mark(now);
        }
    }

    pub fn turn_started(&mut self, id: &str, title: &str, now: Instant) {
        self.sessions.remove(id);
        self.set(id, AgentState::Running, Some(title), "", now);
    }

    pub fn tool_started(&mut self, id: &str, tool: &str, now: Instant) {
        if self
            .sessions
            .get(id)
            .is_some_and(|e| e.state == AgentState::Running)
        {
            self.set(id, AgentState::Running, None, tool, now);
        }
    }

    pub fn tool_done(&mut self, id: &str, now: Instant) {
        if self.sessions.contains_key(id) {
            self.set(id, AgentState::Running, None, "", now);
        }
    }

    pub fn waiting(&mut self, id: &str, detail: &str, now: Instant) {
        self.set(id, AgentState::Waiting, None, detail, now);
    }

    pub fn turn_finished(&mut self, id: &str, error: Option<&str>, now: Instant) {
        match error {
            Some(detail) => self.set(id, AgentState::Error, None, detail, now),
            None => {
                if self.sessions.remove(id).is_some() {
                    self.mark(now);
                }
            }
        }
    }

    pub fn session_closed(&mut self, id: &str, now: Instant) {
        if self.sessions.remove(id).is_some() {
            self.mark(now);
        }
    }

    /// Drop error states older than [`ERROR_TTL`].
    pub fn expire(&mut self, now: Instant) {
        let before = self.sessions.len();
        self.sessions.retain(|_, e| {
            e.state != AgentState::Error || now.saturating_duration_since(e.entered) < ERROR_TTL
        });
        if self.sessions.len() != before {
            self.mark(now);
        }
    }

    fn next_expiry(&self) -> Option<Instant> {
        self.sessions
            .values()
            .filter(|e| e.state == AgentState::Error)
            .map(|e| e.entered + ERROR_TTL)
            .min()
    }

    pub fn snapshot(&self, host: &str) -> Snapshot {
        let mut sessions: Vec<SessionStatus> = self
            .sessions
            .iter()
            .map(|(id, e)| SessionStatus {
                session_id: id.clone(),
                title: e.title.clone(),
                state: e.state,
                detail: e.detail.clone(),
                since: e.since,
            })
            .collect();
        let rank = |s: AgentState| match s {
            AgentState::Waiting => 0,
            AgentState::Error => 1,
            AgentState::Running => 2,
        };
        sessions.sort_by(|a, b| {
            rank(a.state)
                .cmp(&rank(b.state))
                .then(b.since.cmp(&a.since))
                .then(a.session_id.cmp(&b.session_id))
        });
        sessions.truncate(MAX_SESSIONS);
        Snapshot {
            host: host.to_string(),
            sessions,
        }
    }

    /// When the next send is due, or `None` if nothing to send.
    pub fn next_due(&self) -> Option<Instant> {
        let due = match (self.dirty_since, self.last_sent) {
            (Some(dirty), _) => Some(dirty + DEBOUNCE),
            (None, Some(last)) if self.last_sent_nonempty || !self.sessions.is_empty() => {
                Some(last + HEARTBEAT)
            }
            (None, None) if !self.sessions.is_empty() => Some(Instant::now()),
            _ => None,
        };
        let due = due.map(|due| match self.backoff_until {
            Some(b) if b > due => b,
            _ => due,
        });
        match (due, self.next_expiry()) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Take a snapshot if a send is due at `now`.
    pub fn take_due(&mut self, host: &str, now: Instant) -> Option<Snapshot> {
        self.expire(now);
        if self.next_due()? > now {
            return None;
        }
        Some(self.snapshot(host))
    }

    pub fn record_result(&mut self, snapshot: &Snapshot, ok: bool, now: Instant) {
        if ok {
            self.dirty_since = None;
            self.last_sent = Some(now);
            self.last_sent_nonempty = !snapshot.sessions.is_empty();
            self.backoff = None;
            self.backoff_until = None;
        } else {
            let next = self
                .backoff
                .map(|b| (b * 2).min(ERROR_BACKOFF_MAX))
                .unwrap_or(ERROR_BACKOFF_MIN);
            self.backoff = Some(next);
            self.backoff_until = Some(now + next);
        }
    }

    /// Back off without counting an error (e.g. no registered devices).
    pub fn suppress_until(&mut self, until: Instant) {
        self.backoff_until = Some(until);
    }
}

static TRACKER: Mutex<Option<Tracker>> = Mutex::new(None);
static WAKE: OnceLock<tokio::sync::Notify> = OnceLock::new();
static WORKER_STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn with_tracker(f: impl FnOnce(&mut Tracker, Instant)) {
    if !crate::mobile_push::enabled() {
        return;
    }
    let mut guard = TRACKER.lock().unwrap_or_else(|p| p.into_inner());
    f(guard.get_or_insert_with(Tracker::default), Instant::now());
    drop(guard);
    ensure_worker();
    WAKE.get_or_init(tokio::sync::Notify::new).notify_one();
}

pub fn turn_started(session_id: &str, title: &str) {
    with_tracker(|t, now| t.turn_started(session_id, title, now));
}
pub fn tool_started(session_id: &str, tool: &str) {
    with_tracker(|t, now| t.tool_started(session_id, tool, now));
}
pub fn tool_done(session_id: &str) {
    with_tracker(|t, now| t.tool_done(session_id, now));
}
pub fn waiting(session_id: &str, detail: &str) {
    with_tracker(|t, now| t.waiting(session_id, detail, now));
}
pub fn turn_finished(session_id: &str, error: Option<&str>) {
    with_tracker(|t, now| t.turn_finished(session_id, error, now));
}
pub fn session_closed(session_id: &str) {
    with_tracker(|t, now| t.session_closed(session_id, now));
}

/// Stable host id: `[A-Za-z0-9_.:-]{1,64}`, from /etc/machine-id or hostname.
pub fn host_id() -> &'static str {
    static HOST: OnceLock<String> = OnceLock::new();
    HOST.get_or_init(|| {
        let raw = ["/etc/machine-id", "/var/lib/dbus/machine-id"]
            .iter()
            .find_map(|p| std::fs::read_to_string(p).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| std::env::var("HOSTNAME").ok())
            .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
            .or_else(|| std::env::var("COMPUTERNAME").ok())
            .unwrap_or_else(|| "jcode-host".to_string());
        sanitize_host(&raw)
    })
}

pub fn sanitize_host(raw: &str) -> String {
    let s: String = raw
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '-') {
                c
            } else {
                '-'
            }
        })
        .take(64)
        .collect();
    if s.is_empty() { "jcode-host".into() } else { s }
}

fn ensure_worker() {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    if WORKER_STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    handle.spawn(worker());
}

async fn worker() {
    let wake = WAKE.get_or_init(tokio::sync::Notify::new);
    loop {
        let due = TRACKER
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .and_then(Tracker::next_due);
        match due {
            Some(due) => {
                let sleep = tokio::time::sleep_until(tokio::time::Instant::from_std(due));
                tokio::select! { _ = sleep => {}, _ = wake.notified() => continue }
            }
            None => {
                wake.notified().await;
                continue;
            }
        }
        let snapshot = TRACKER
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_mut()
            .and_then(|t| t.take_due(host_id(), Instant::now()));
        let Some(snapshot) = snapshot else { continue };
        let outcome = post(&snapshot).await;
        let mut guard = TRACKER.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(t) = guard.as_mut() {
            let now = Instant::now();
            match outcome {
                PostOutcome::Ok => t.record_result(&snapshot, true, now),
                PostOutcome::NoDevices => {
                    t.record_result(&snapshot, true, now);
                    t.suppress_until(now + crate::mobile_push::NO_DEVICES_SUPPRESS);
                }
                PostOutcome::Failed => t.record_result(&snapshot, false, now),
            }
        }
    }
}

enum PostOutcome {
    Ok,
    NoDevices,
    Failed,
}

#[derive(serde::Deserialize)]
struct Reply {
    #[serde(default)]
    devices: Option<u64>,
}

async fn post(snapshot: &Snapshot) -> PostOutcome {
    if !crate::mobile_push::enabled() {
        return PostOutcome::Ok;
    }
    if crate::mobile_push::devices_suppressed() {
        return PostOutcome::NoDevices;
    }
    let Some((key, base)) = crate::mobile_push::credentials() else {
        // Not signed in: treat as delivered so we do not spin.
        return PostOutcome::Ok;
    };
    let result = crate::provider::shared_http_client()
        .post(format!("{base}/agents/status"))
        .bearer_auth(key)
        .json(snapshot)
        .timeout(SEND_TIMEOUT)
        .send()
        .await;
    match result {
        Ok(resp) if resp.status().is_success() => match resp.json::<Reply>().await {
            Ok(Reply { devices: Some(0) }) => PostOutcome::NoDevices,
            _ => PostOutcome::Ok,
        },
        Ok(resp) => {
            crate::logging::warn(&format!("agents status rejected: HTTP {}", resp.status()));
            PostOutcome::Failed
        }
        Err(error) => {
            crate::logging::warn(&format!("agents status failed: {error}"));
            PostOutcome::Failed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_orders_and_truncates() {
        let mut t = Tracker::default();
        let now = Instant::now();
        t.turn_started("a", "Fix login", now);
        t.tool_started("a", "bash", now);
        t.turn_started("b", &"x".repeat(200), now);
        t.waiting("b", "needs input", now);
        t.turn_started("c", "C", now);
        t.turn_finished("c", Some("boom"), now);
        t.turn_started("d", "D", now);
        t.turn_finished("d", None, now);
        let snap = t.snapshot("host1");
        let ids: Vec<_> = snap
            .sessions
            .iter()
            .map(|s| s.session_id.as_str())
            .collect();
        assert_eq!(ids, ["b", "c", "a"]);
        assert_eq!(snap.sessions[2].detail, "bash");
        assert_eq!(snap.sessions[0].title.chars().count(), TITLE_MAX);
        let json = serde_json::to_value(&snap).unwrap();
        assert_eq!(json["sessions"][1]["state"], "error");
        assert_eq!(json["host"], "host1");
    }

    #[test]
    fn debounce_heartbeat_and_final_empty() {
        let mut t = Tracker::default();
        let t0 = Instant::now();
        assert!(t.next_due().is_none());
        t.turn_started("a", "A", t0);
        t.tool_started("a", "bash", t0 + Duration::from_millis(500));
        assert!(t.take_due("h", t0 + Duration::from_secs(1)).is_none());
        let at = t0 + DEBOUNCE;
        let snap = t.take_due("h", at).expect("due after debounce");
        assert_eq!(snap.sessions.len(), 1);
        t.record_result(&snap, true, at);
        assert_eq!(t.next_due(), Some(at + HEARTBEAT));
        t.turn_finished("a", None, at + Duration::from_secs(10));
        let end = at + Duration::from_secs(10) + DEBOUNCE;
        let snap = t.take_due("h", end).expect("final empty");
        assert!(snap.sessions.is_empty());
        t.record_result(&snap, true, end);
        assert!(t.next_due().is_none(), "no heartbeat after empty");
    }

    #[test]
    fn errors_back_off() {
        let mut t = Tracker::default();
        let t0 = Instant::now();
        t.turn_started("a", "A", t0);
        let snap = t.take_due("h", t0 + DEBOUNCE).unwrap();
        t.record_result(&snap, false, t0 + DEBOUNCE);
        assert_eq!(t.next_due(), Some(t0 + DEBOUNCE + ERROR_BACKOFF_MIN));
        t.record_result(&snap, false, t0 + DEBOUNCE);
        assert_eq!(t.next_due(), Some(t0 + DEBOUNCE + ERROR_BACKOFF_MIN * 2));
    }

    #[test]
    fn error_state_expires_after_ttl() {
        let mut t = Tracker::default();
        let t0 = Instant::now();
        t.turn_started("a", "A", t0);
        t.turn_finished("a", Some("boom"), t0);
        t.turn_started("b", "B", t0);
        let snap = t.take_due("h", t0 + DEBOUNCE).unwrap();
        t.record_result(&snap, true, t0 + DEBOUNCE);
        assert_eq!(t.next_due(), Some(t0 + DEBOUNCE + HEARTBEAT));
        t.last_sent = Some(t0 + ERROR_TTL);
        assert_eq!(
            t.next_due(),
            Some(t0 + ERROR_TTL),
            "expiry wakes the worker"
        );
        t.expire(t0 + ERROR_TTL - Duration::from_secs(1));
        assert_eq!(t.snapshot("h").sessions.len(), 2);
        t.expire(t0 + ERROR_TTL);
        let snap = t.take_due("h", t0 + ERROR_TTL + DEBOUNCE).unwrap();
        let ids: Vec<_> = snap
            .sessions
            .iter()
            .map(|s| s.session_id.as_str())
            .collect();
        assert_eq!(ids, ["b"]);
    }

    #[test]
    fn host_is_sanitized() {
        assert_eq!(sanitize_host(" my host!\n"), "my-host-");
        assert_eq!(sanitize_host(""), "jcode-host");
        assert_eq!(sanitize_host(&"a".repeat(100)).len(), 64);
    }
}
