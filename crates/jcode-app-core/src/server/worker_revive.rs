//! Lazily restore a headless swarm worker's agent from disk.
//!
//! Startup recovery only restores headless workers that were mid-turn when the
//! server went down. A worker that had already failed (for example on a
//! provider rate limit) or was idle stays in the swarm as a durable member
//! record with no live agent. Every later wake, resume, retry or DM then found
//! "no live agent" and the only way forward was spawning a replacement, which
//! throws away the worker's transcript. This module rebuilds the agent on
//! demand from the persisted session so the coordinator can simply resume it.

use super::{
    SessionAgents, SessionInterruptQueues, SwarmMember, register_background_tool_signal,
    register_session_interrupt_queue,
};
use crate::agent::Agent;
use crate::provider::Provider;
use jcode_agent_runtime::InterruptSignal;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use tokio::sync::{Mutex, OnceCell, RwLock};

/// Server-owned handles needed to rebuild an agent. Registered once when the
/// server starts. Tests and embedders that never register it simply get no
/// revival, which preserves the previous behavior.
pub(super) struct WorkerReviveContext {
    pub provider: Arc<dyn Provider>,
    pub sessions: SessionAgents,
    pub mcp_pool: Arc<OnceCell<Arc<crate::mcp::SharedMcpPool>>>,
    pub shutdown_signals: Arc<RwLock<HashMap<String, InterruptSignal>>>,
    pub soft_interrupt_queues: SessionInterruptQueues,
}

static CONTEXT: OnceLock<WorkerReviveContext> = OnceLock::new();

pub(super) fn register_context(context: WorkerReviveContext) {
    let _ = CONTEXT.set(context);
}

/// Serializes revivals so two concurrent wakes cannot build two agents for the
/// same session.
fn revive_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Return the live agent for `session_id`, restoring it from disk when it is a
/// headless swarm member whose agent is not loaded. Returns `None` when the
/// session is not a headless member, the session file cannot be loaded, or no
/// revive context is registered.
pub(super) async fn live_or_revived_agent(
    session_id: &str,
    sessions: &SessionAgents,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
) -> Option<Arc<Mutex<Agent>>> {
    if let Some(agent) = sessions.read().await.get(session_id).cloned() {
        return Some(agent);
    }
    let is_headless_member = swarm_members
        .read()
        .await
        .get(session_id)
        .is_some_and(|member| member.is_headless);
    if !is_headless_member {
        return None;
    }
    let context = CONTEXT.get()?;
    let _guard = revive_lock().lock().await;
    if let Some(agent) = sessions.read().await.get(session_id).cloned() {
        return Some(agent);
    }

    let session = match crate::session::Session::load(session_id) {
        Ok(session) => session,
        Err(error) => {
            crate::logging::warn(&format!(
                "Cannot revive headless worker {session_id}: failed to load session: {error}"
            ));
            return None;
        }
    };
    // A migrated source or partially received destination must not be revived
    // by a delayed wake. The transfer importer clears the inbound lease only
    // after every selected transcript and swarm snapshot has landed.
    if session.migration_lease_block().is_some() {
        return None;
    }
    let provider = context.provider.fork();
    let registry = crate::tool::Registry::new(provider.clone()).await;
    if session.is_canary {
        registry.register_selfdev_tools().await;
    }
    let mcp_pool = super::get_shared_mcp_pool(&context.mcp_pool).await;
    registry
        .register_mcp_tools_for_dir(
            None,
            Some(mcp_pool),
            Some("headless".to_string()),
            session.working_dir.as_ref().map(std::path::PathBuf::from),
        )
        .await;
    let mut agent = Agent::new_with_session(provider, registry, session, None);
    agent.set_debug(true);
    if matches!(
        crate::config::config().agents.swarm_spawn_mode,
        crate::config::SwarmSpawnMode::Inline
    ) {
        agent.set_inline_output_tap(true);
    }
    let agent = Arc::new(Mutex::new(agent));

    {
        let guard = agent.lock().await;
        register_session_interrupt_queue(
            &context.soft_interrupt_queues,
            session_id,
            guard.soft_interrupt_queue(),
        )
        .await;
        context
            .shutdown_signals
            .write()
            .await
            .insert(session_id.to_string(), guard.graceful_shutdown_signal());
        register_background_tool_signal(session_id, guard.background_tool_signal());
    }
    // `sessions` and the registered context normally share the same map, but
    // insert into the caller's map so the lookup that follows always succeeds.
    sessions
        .write()
        .await
        .insert(session_id.to_string(), Arc::clone(&agent));
    if !Arc::ptr_eq(sessions, &context.sessions) {
        context
            .sessions
            .write()
            .await
            .insert(session_id.to_string(), Arc::clone(&agent));
    }
    // A headless member restored from durable state carries a closed
    // placeholder channel. Give it a draining channel so fanout and
    // reachability checks treat it as live again.
    {
        let (event_tx, mut event_rx) =
            tokio::sync::mpsc::unbounded_channel::<crate::protocol::ServerEvent>();
        tokio::spawn(async move { while event_rx.recv().await.is_some() {} });
        if let Some(member) = swarm_members.write().await.get_mut(session_id)
            && member.event_tx.is_closed()
        {
            member.event_tx = event_tx;
        }
    }
    crate::logging::info(&format!(
        "Revived headless swarm worker {session_id} from disk"
    ));
    Some(agent)
}
