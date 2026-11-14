//! Delivers messages the phone sends to sessions that are running on this
//! computer, without moving them to Jcode Cloud.
//!
//! The daemon reports the sessions it currently holds to
//! `/mobile/commands/claim-local` every few seconds. That report is what makes
//! a synced session "live" in the iOS app, and its response carries at most
//! one queued `message` command for one of those sessions. The message is
//! handed to the running session exactly like a prompt typed in its window,
//! and the resulting transcript reaches the phone through mobile sync.
//!
//! Nothing here runs unless the user enabled mobile sync on this computer,
//! and `jcode mobile messages off` turns delivery off again.

use super::{ClaimResponse, MobileCommand, POLL_INTERVAL, events};
use crate::mobile_sync;
use anyhow::Result;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

/// The API accepts at most this many sessions per report.
pub const MAX_REPORTED_SESSIONS: usize = 90;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveSession {
    pub session_id: String,
    /// A turn is in flight, so a message is queued behind it.
    pub busy: bool,
}

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The running daemon's sessions, as seen by the phone.
pub trait LocalSessionHost: Send + Sync + 'static {
    /// Sessions that can take a message right now.
    fn live_sessions(&self) -> BoxFuture<'_, Vec<LiveSession>>;
    /// Hand `text` to a live session as a user message. Returns once the
    /// message was accepted (turn started or queued), not when the turn ends.
    fn deliver<'a>(&'a self, session_id: &'a str, text: &'a str) -> BoxFuture<'a, Result<()>>;
}

/// Whether this daemon should accept messages from the phone.
pub fn enabled() -> bool {
    if super::is_cloud_machine() || mobile_sync::session_source() != "local" {
        return false;
    }
    let state = mobile_sync::load_state();
    state.enabled && !state.phone_messages_off
}

/// Busy sessions first, then by id, so the cap never hides an active session
/// and the report is stable between heartbeats.
fn report(sessions: Vec<LiveSession>) -> Vec<LiveSession> {
    let mut unique = std::collections::BTreeMap::<String, bool>::new();
    for session in sessions {
        if mobile_sync::valid_session_id(&session.session_id) {
            unique
                .entry(session.session_id)
                .and_modify(|busy| *busy |= session.busy)
                .or_insert(session.busy);
        }
    }
    let mut sessions: Vec<_> = unique
        .into_iter()
        .map(|(session_id, busy)| LiveSession { session_id, busy })
        .collect();
    sessions.sort_by(|a, b| {
        b.busy
            .cmp(&a.busy)
            .then_with(|| a.session_id.cmp(&b.session_id))
    });
    sessions.truncate(MAX_REPORTED_SESSIONS);
    sessions
}

async fn claim_local(
    client: &reqwest::Client,
    base: &str,
    key: &str,
    machine: &str,
    sessions: &[LiveSession],
) -> Result<Option<MobileCommand>> {
    let sessions: Vec<_> = sessions
        .iter()
        .map(
            |session| serde_json::json!({ "session_id": session.session_id, "busy": session.busy }),
        )
        .collect();
    let response = client
        .post(format!("{base}/mobile/commands/claim-local"))
        .bearer_auth(key)
        .json(&serde_json::json!({ "machine": machine, "sessions": sessions }))
        .timeout(Duration::from_secs(30))
        .send()
        .await?;
    if !response.status().is_success() {
        anyhow::bail!("claim-local returned {}", response.status());
    }
    Ok(response.json::<ClaimResponse>().await?.command)
}

/// Report live sessions and deliver at most one message. Returns whether a
/// message was claimed.
pub async fn run_local_once(
    client: &reqwest::Client,
    base: &str,
    key: &str,
    machine: &str,
    host: &dyn LocalSessionHost,
) -> Result<bool> {
    let sessions = report(host.live_sessions().await);
    if sessions.is_empty() {
        // Nothing can be live, so there is nothing to report or claim.
        return Ok(false);
    }
    let Some(command) = claim_local(client, base, key, machine, &sessions).await? else {
        return Ok(false);
    };
    let outcome: Result<String> = async {
        if command.kind != "message" {
            anyhow::bail!("this computer only accepts messages for running sessions");
        }
        let session_id = command
            .session_id
            .as_deref()
            .filter(|id| sessions.iter().any(|session| session.session_id == *id))
            .ok_or_else(|| anyhow::anyhow!("that session is not running on this computer"))?;
        if command.text.trim().is_empty() {
            anyhow::bail!("empty message");
        }
        host.deliver(session_id, &command.text).await?;
        Ok(session_id.to_string())
    }
    .await;
    if let Err(error) = &outcome {
        // The error text is ours, never the message content.
        crate::logging::warn(&format!("phone message was not delivered: {error:#}"));
    }
    super::complete(client, base, key, &command.command_id, &outcome).await?;
    Ok(true)
}

fn machine_name() -> String {
    mobile_sync::host_name().unwrap_or_else(|| "computer".to_string())
}

/// Background loop for the daemon on the user's own computer.
pub fn spawn_local_runner(host: Arc<dyn LocalSessionHost>) {
    if super::is_cloud_machine() {
        return;
    }
    tokio::spawn(async move {
        let client = reqwest::Client::new();
        let machine = machine_name();
        // Created on first use so a daemon without mobile sync opens no socket.
        let mut wakeups: Option<events::Wakeups> = None;
        loop {
            if !enabled() {
                wakeups = None;
                tokio::time::sleep(Duration::from_secs(15)).await;
                continue;
            }
            let Some(key) = crate::subscription_catalog::configured_api_key() else {
                wakeups = None;
                tokio::time::sleep(Duration::from_secs(15)).await;
                continue;
            };
            let base = crate::subscription_api::configured_api_base();
            let delay = match run_local_once(&client, &base, &key, &machine, host.as_ref()).await {
                Ok(true) => continue,
                Ok(false) => POLL_INTERVAL,
                Err(error) => {
                    crate::logging::debug(&format!("phone message poll failed: {error:#}"));
                    Duration::from_secs(20)
                }
            };
            wakeups.get_or_insert_with(events::spawn).wait(delay).await;
        }
    });
}

#[cfg(test)]
#[path = "mobile_runner_local_tests.rs"]
mod tests;
