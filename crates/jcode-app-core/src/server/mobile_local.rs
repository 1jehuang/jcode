//! Lets the iOS app message sessions that are running in this daemon.
//!
//! `jcode_base::mobile_runner` owns the account API side (presence report,
//! claim, completion). This module is the daemon side: which sessions are
//! live, and how a phone message enters one. A message is handled like a
//! prompt typed in the session's own window: it starts a turn when the
//! session is idle and is queued behind the current turn when it is busy.

use super::live_turn::{LiveTurnSwarmContext, run_live_turn_if_idle};
use super::state::{fanout_live_client_event, queue_soft_interrupt_for_session};
use super::{SessionAgents, SessionInterruptQueues};
use crate::mobile_runner::{LiveSession, LocalSessionHost};
use crate::protocol::ServerEvent;
use anyhow::Result;
use jcode_agent_runtime::SoftInterruptSource;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

/// How long an idle-looking session may hold its agent lock (history load,
/// attach, save) before the message is queued instead of started.
const IDLE_START_WAIT: Duration = Duration::from_secs(4);
const IDLE_START_RETRY: Duration = Duration::from_millis(100);

pub(super) struct DaemonSessions {
    pub sessions: SessionAgents,
    pub soft_interrupt_queues: SessionInterruptQueues,
    pub swarm: LiveTurnSwarmContext,
}

impl DaemonSessions {
    /// `Some(busy)` when the session can be driven from this daemon.
    async fn member_state(&self, session_id: &str) -> Option<bool> {
        let members = self.swarm.members.read().await;
        let member = members.get(session_id)?;
        let drivable =
            member.is_headless || !member.event_txs.is_empty() || !member.event_tx.is_closed();
        drivable.then(|| member.status == "running")
    }

    async fn live(&self) -> Vec<LiveSession> {
        let agents: Vec<_> = self
            .sessions
            .read()
            .await
            .iter()
            .map(|(id, agent)| (id.clone(), Arc::clone(agent)))
            .collect();
        let mut live = Vec::with_capacity(agents.len());
        for (session_id, agent) in agents {
            // An idle migrated copy cannot accept turns on this computer.
            // A locked agent may be mid-turn, so never wait on that lock
            // just to publish presence.
            if agent
                .try_lock()
                .is_ok_and(|agent| agent.session_lease_blocked())
            {
                continue;
            }
            if let Some(busy) = self.member_state(&session_id).await {
                live.push(LiveSession { session_id, busy });
            }
        }
        live
    }

    async fn deliver_message(&self, session_id: &str, text: &str) -> Result<()> {
        if !self.sessions.read().await.contains_key(session_id) {
            anyhow::bail!("that session is no longer running on this computer");
        }
        let deadline = tokio::time::Instant::now() + IDLE_START_WAIT;
        loop {
            if run_live_turn_if_idle(session_id, text, None, &self.sessions, self.swarm.clone())
                .await
            {
                // Windows attached to the session only receive the turn's
                // stream, so show them the prompt that started it.
                fanout_live_client_event(
                    &self.swarm.members,
                    session_id,
                    ServerEvent::PeerUserMessage {
                        content: text.to_string(),
                    },
                )
                .await;
                return Ok(());
            }
            let Some(busy) = self.member_state(session_id).await else {
                anyhow::bail!("that session is no longer open on this computer");
            };
            // A running turn drains its queue. An idle session never would,
            // so wait out the brief lock instead of stranding the message.
            if busy || tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(IDLE_START_RETRY).await;
        }
        if self.member_state(session_id).await != Some(true) {
            anyhow::bail!("that session cannot take a message right now; try again in a moment");
        }
        if queue_soft_interrupt_for_session(
            session_id,
            text.to_string(),
            false,
            SoftInterruptSource::User,
            &self.soft_interrupt_queues,
            &self.sessions,
        )
        .await
        {
            Ok(())
        } else {
            anyhow::bail!("that session is busy and could not queue the message")
        }
    }
}

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

impl LocalSessionHost for DaemonSessions {
    fn live_sessions(&self) -> BoxFuture<'_, Vec<LiveSession>> {
        Box::pin(self.live())
    }

    fn deliver<'a>(&'a self, session_id: &'a str, text: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(self.deliver_message(session_id, text))
    }
}

pub(super) fn spawn(host: DaemonSessions) {
    crate::mobile_runner::spawn_local_runner(Arc::new(host));
}
