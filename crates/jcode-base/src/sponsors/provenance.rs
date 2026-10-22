//! Sponsored-discovery provenance and coarse usage metering.
//!
//! When the agent connects an MCP server whose command matches a recent
//! `discover_tools` listing, that server is tagged with discovery
//! provenance. Calls routed to provenance-tagged servers are metered
//! coarsely: per-sponsor per-day counts of connects, calls, and errors.
//! Nothing else. Never arguments, never results, never session content,
//! never user identity. Aggregates are flushed at most once per hour to
//! `POST {sponsors.endpoint}/usage` and only while `sponsors.enabled` is
//! true. The policy is disclosed at
//! <https://jcode.sh/sponsor> and in the connect-time
//! UI line.
//!
//! Everything here is process-local and best-effort: metering failures
//! must never affect tool execution.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// A discovered MCP setup remembered from a `discover_tools` listing.
/// Matching is on (command, args) so an agent connecting `npx -y
/// agentcard-mcp` after discovering it gets tagged, while a user's
/// long-standing config entry for some unrelated server never is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredSetup {
    pub sponsor: String,
    pub command: String,
    pub args: Vec<String>,
}

#[derive(Default)]
struct ProvenanceState {
    /// Setups seen in discovery listings this process lifetime.
    discovered: Vec<DiscoveredSetup>,
    /// server name -> sponsor, for servers connected after discovery.
    tagged: HashMap<String, String>,
    /// (sponsor, day) -> counts, pending flush.
    pending: HashMap<(String, String), UsageCounts>,
    last_flush: Option<Instant>,
}

#[derive(Debug, Default, Clone, Copy)]
struct UsageCounts {
    connects: u64,
    calls: u64,
    errors: u64,
}

static STATE: Mutex<Option<ProvenanceState>> = Mutex::new(None);

const FLUSH_INTERVAL: Duration = Duration::from_secs(60 * 60);
const FLUSH_TIMEOUT: Duration = Duration::from_secs(5);

fn with_state<R>(f: impl FnOnce(&mut ProvenanceState) -> R) -> R {
    let mut guard = STATE.lock().unwrap_or_else(|poison| poison.into_inner());
    f(guard.get_or_insert_with(ProvenanceState::default))
}

fn today() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

/// Record MCP setups from a discovery listing so a later `mcp connect`
/// can be recognized as discovery-initiated.
pub fn record_discovered_setups(setups: Vec<DiscoveredSetup>) {
    if setups.is_empty() {
        return;
    }
    with_state(|state| {
        for setup in setups {
            if !state.discovered.contains(&setup) {
                state.discovered.push(setup);
            }
        }
        // Bounded: discovery listings are small and per-process.
        state.discovered.truncate(64);
    });
}

/// Called when an MCP server connects. If its command matches a recorded
/// discovered setup, tag it and count the connect. Returns the sponsor
/// name when tagged so the caller can render the disclosure line.
pub fn on_server_connected(server_name: &str, command: &str, args: &[String]) -> Option<String> {
    if !crate::config::config().sponsors.enabled {
        return None;
    }
    with_state(|state| {
        let sponsor = state
            .discovered
            .iter()
            .find(|s| s.command == command && s.args == args)?
            .sponsor
            .clone();
        state
            .tagged
            .insert(server_name.to_string(), sponsor.clone());
        state
            .pending
            .entry((sponsor.clone(), today()))
            .or_default()
            .connects += 1;
        Some(sponsor)
    })
}

/// Called after every MCP tool call. Counts calls/errors for
/// provenance-tagged servers only; everything else is a no-op.
pub fn on_tool_call(server_name: &str, is_error: bool) {
    if !crate::config::config().sponsors.enabled {
        return;
    }
    let flush_body = with_state(|state| {
        let sponsor = state.tagged.get(server_name)?.clone();
        let counts = state.pending.entry((sponsor, today())).or_default();
        counts.calls += 1;
        if is_error {
            counts.errors += 1;
        }
        // Start the flush clock on the first metered call; flush only after
        // a full interval has elapsed so aggregates are actually aggregated.
        let due = match state.last_flush {
            None => {
                state.last_flush = Some(Instant::now());
                false
            }
            Some(at) => at.elapsed() >= FLUSH_INTERVAL,
        };
        if !due {
            return None;
        }
        state.last_flush = Some(Instant::now());
        Some(drain_reports(state))
    });
    if let Some(body) = flush_body {
        spawn_flush(body);
    }
}

/// True when `server_name` carries discovery provenance.
pub fn is_tagged(server_name: &str) -> bool {
    with_state(|state| state.tagged.contains_key(server_name))
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct UsageReport {
    pub sponsor: String,
    pub day: String,
    pub connects: u64,
    pub calls: u64,
    pub errors: u64,
}

fn drain_reports(state: &mut ProvenanceState) -> Vec<UsageReport> {
    state
        .pending
        .drain()
        .map(|((sponsor, day), counts)| UsageReport {
            sponsor,
            day,
            connects: counts.connects,
            calls: counts.calls,
            errors: counts.errors,
        })
        .collect()
}

fn spawn_flush(reports: Vec<UsageReport>) {
    if reports.is_empty() {
        return;
    }
    let endpoint = format!(
        "{}/usage",
        crate::config::config()
            .sponsors
            .endpoint
            .trim_end_matches('/')
    );
    let body = serde_json::json!({
        "client_version": env!("CARGO_PKG_VERSION"),
        "reports": reports,
    });
    // Best effort: fire and forget. A failed flush drops this hour's
    // aggregates rather than queueing unbounded state.
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(async move {
            let client = crate::provider::shared_http_client();
            let _ = client
                .post(&endpoint)
                .json(&body)
                .timeout(FLUSH_TIMEOUT)
                .send()
                .await;
        });
    }
}

/// Flush pending aggregates immediately (best effort). Called on shutdown
/// so short sessions still report.
pub fn flush_now() {
    if !crate::config::config().sponsors.enabled {
        return;
    }
    let reports = with_state(|state| {
        state.last_flush = Some(Instant::now());
        drain_reports(state)
    });
    spawn_flush(reports);
}

#[cfg(test)]
pub(crate) fn reset_for_tests() {
    let mut guard = STATE.lock().unwrap_or_else(|poison| poison.into_inner());
    *guard = Some(ProvenanceState::default());
}

#[cfg(all(test, unix))]
pub(crate) fn drain_pending_for_tests() -> Vec<UsageReport> {
    with_state(drain_reports)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enable_sponsors() -> (crate::storage::TestEnvGuard, tempfile::TempDir) {
        let guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().unwrap();
        crate::env::set_var("JCODE_HOME", temp.path());
        std::fs::write(
            temp.path().join("config.toml"),
            "[sponsors]\nenabled = true\n",
        )
        .unwrap();
        crate::config::Config::invalidate_cache();
        (guard, temp)
    }

    #[test]
    fn connect_matching_discovered_setup_tags_and_counts() {
        let _env = enable_sponsors();
        reset_for_tests();
        record_discovered_setups(vec![DiscoveredSetup {
            sponsor: "agentcard".into(),
            command: "npx".into(),
            args: vec!["-y".into(), "agentcard-mcp".into()],
        }]);

        let sponsor = on_server_connected(
            "agentcard",
            "npx",
            &["-y".to_string(), "agentcard-mcp".to_string()],
        );
        assert_eq!(sponsor.as_deref(), Some("agentcard"));
        assert!(is_tagged("agentcard"));

        on_tool_call("agentcard", false);
        on_tool_call("agentcard", true);

        let reports = with_state(drain_reports);
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].sponsor, "agentcard");
        assert_eq!(reports[0].connects, 1);
        assert_eq!(reports[0].calls, 2);
        assert_eq!(reports[0].errors, 1);
    }

    #[test]
    fn non_discovered_servers_are_never_tagged_or_metered() {
        let _env = enable_sponsors();
        reset_for_tests();
        record_discovered_setups(vec![DiscoveredSetup {
            sponsor: "agentcard".into(),
            command: "npx".into(),
            args: vec!["-y".into(), "agentcard-mcp".into()],
        }]);

        // Same server name, different command: user's own config, not ours.
        let sponsor = on_server_connected("agentcard", "node", &["my-local-server.js".to_string()]);
        assert_eq!(sponsor, None);
        assert!(!is_tagged("agentcard"));

        on_tool_call("agentcard", false);
        on_tool_call("some-private-server", false);
        let reports = with_state(drain_reports);
        assert!(reports.is_empty(), "untagged calls must never be metered");
    }

    #[test]
    fn disabled_sponsors_config_disables_everything() {
        let guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().unwrap();
        crate::env::set_var("JCODE_HOME", temp.path());
        std::fs::write(
            temp.path().join("config.toml"),
            "[sponsors]\nenabled = false\n",
        )
        .unwrap();
        crate::config::Config::invalidate_cache();
        drop(guard);
        let _guard = crate::storage::lock_test_env();
        reset_for_tests();
        record_discovered_setups(vec![DiscoveredSetup {
            sponsor: "agentcard".into(),
            command: "npx".into(),
            args: vec!["-y".into(), "agentcard-mcp".into()],
        }]);
        let sponsor = on_server_connected(
            "agentcard",
            "npx",
            &["-y".to_string(), "agentcard-mcp".to_string()],
        );
        assert_eq!(sponsor, None, "opt-out must disable provenance tagging");
        on_tool_call("agentcard", false);
        let reports = with_state(drain_reports);
        assert!(reports.is_empty());
    }
}
