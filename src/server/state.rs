use crate::bus::FileOp;
use crate::plan::VersionedPlan;
use crate::protocol::ServerEvent;
use jcode_agent_runtime::{
    InterruptSignal, SoftInterruptMessage, SoftInterruptQueue, SoftInterruptSource,
};
use jcode_swarm_core::{SwarmLifecycleStatus, SwarmMemberRecord, SwarmRole};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{RwLock, mpsc};

/// Record of a file access by an agent
#[derive(Clone, Debug)]
pub struct FileAccess {
    pub session_id: String,
    pub op: FileOp,
    pub timestamp: Instant,
    pub absolute_time: std::time::SystemTime,
    pub intent: Option<String>,
    pub summary: Option<String>,
    pub detail: Option<String>,
}

pub(super) fn latest_peer_touches(
    accesses: &[FileAccess],
    current_session_id: &str,
    swarm_session_ids: &HashSet<String>,
) -> Vec<FileAccess> {
    let mut latest_by_session: HashMap<&str, &FileAccess> = HashMap::new();

    for access in accesses.iter().filter(|access| {
        access.session_id != current_session_id
            && swarm_session_ids.contains(&access.session_id)
            && access.op.is_modification()
    }) {
        latest_by_session
            .entry(&access.session_id)
            .and_modify(|existing| {
                if access.timestamp > existing.timestamp {
                    *existing = access;
                }
            })
            .or_insert(access);
    }

    let mut latest: Vec<FileAccess> = latest_by_session.into_values().cloned().collect();
    latest.sort_by(|left, right| left.session_id.cmp(&right.session_id));
    latest
}

/// Shared ownership of the core persisted swarm coordination state.
#[derive(Clone)]
pub struct SwarmState {
    pub members: Arc<RwLock<HashMap<String, SwarmMember>>>,
    pub swarms_by_id: Arc<RwLock<HashMap<String, HashSet<String>>>>,
    pub plans: Arc<RwLock<HashMap<String, VersionedPlan>>>,
    pub coordinators: Arc<RwLock<HashMap<String, String>>>,
}

/// First-class snapshot of a single swarm's logical runtime state.
#[derive(Clone, Debug)]
pub struct SwarmRuntime {
    pub swarm_id: String,
    pub coordinator_session_id: Option<String>,
    pub member_session_ids: HashSet<String>,
    pub members: Vec<SwarmMember>,
    pub plan: Option<VersionedPlan>,
}

impl SwarmRuntime {
    pub fn has_any_state(&self) -> bool {
        self.plan.is_some() || self.coordinator_session_id.is_some() || !self.members.is_empty()
    }
}

/// Live transport attachment for a connected session.
#[derive(Clone, Debug)]
pub struct LiveSessionAttachment {
    pub connection_id: String,
    pub event_tx: mpsc::UnboundedSender<ServerEvent>,
}

impl SwarmState {
    pub fn new(
        members: HashMap<String, SwarmMember>,
        swarms_by_id: HashMap<String, HashSet<String>>,
        plans: HashMap<String, VersionedPlan>,
        coordinators: HashMap<String, String>,
    ) -> Self {
        Self {
            members: Arc::new(RwLock::new(members)),
            swarms_by_id: Arc::new(RwLock::new(swarms_by_id)),
            plans: Arc::new(RwLock::new(plans)),
            coordinators: Arc::new(RwLock::new(coordinators)),
        }
    }

    pub async fn load_runtime(&self, swarm_id: &str) -> SwarmRuntime {
        let plan = {
            let plans = self.plans.read().await;
            plans.get(swarm_id).cloned()
        };
        let coordinator_session_id = {
            let coordinators = self.coordinators.read().await;
            coordinators.get(swarm_id).cloned()
        };
        let member_session_ids = {
            let swarms = self.swarms_by_id.read().await;
            swarms.get(swarm_id).cloned().unwrap_or_default()
        };
        let mut members = {
            let members = self.members.read().await;
            members
                .values()
                .filter(|member| member.swarm_id.as_deref() == Some(swarm_id))
                .cloned()
                .collect::<Vec<_>>()
        };
        members.sort_by(|left, right| left.session_id.cmp(&right.session_id));

        SwarmRuntime {
            swarm_id: swarm_id.to_string(),
            coordinator_session_id,
            member_session_ids,
            members,
            plan,
        }
    }
}

/// Information about a session in a swarm
#[derive(Clone, Debug)]
pub struct SwarmMember {
    pub session_id: String,
    /// Primary channel to send events to this session.
    ///
    /// This remains for backward-compatible single-sender call sites and for
    /// headless sessions that do not maintain a live attachment map.
    pub event_tx: mpsc::UnboundedSender<ServerEvent>,
    /// Live client attachments for this session keyed by connection id.
    pub event_txs: HashMap<String, mpsc::UnboundedSender<ServerEvent>>,
    /// Working directory (used to derive swarm id)
    pub working_dir: Option<PathBuf>,
    /// Swarm identifier (shared across worktrees)
    pub swarm_id: Option<String>,
    /// Whether swarm coordination is enabled for this member
    pub swarm_enabled: bool,
    /// Lifecycle status (ready, running, completed, failed, stopped, etc.)
    pub status: String,
    /// Optional detail (current task, error, etc.)
    pub detail: Option<String>,
    /// Friendly name like "fox"
    pub friendly_name: Option<String>,
    /// Session that should receive direct completion report-back for this member, if any.
    pub report_back_to_session_id: Option<String>,
    /// Latest explicit completion report submitted by this member.
    pub latest_completion_report: Option<String>,
    /// Role: "agent", "coordinator", "worktree_manager"
    pub role: String,
    /// When this member joined the swarm
    pub joined_at: Instant,
    /// When status was last changed
    pub last_status_change: Instant,
    /// Whether this is a headless (spawned) session vs a TUI-connected session.
    /// Headless sessions should not be automatically elected as coordinator.
    pub is_headless: bool,
}

impl SwarmMember {
    pub fn durable_record(&self) -> SwarmMemberRecord {
        SwarmMemberRecord {
            session_id: self.session_id.clone(),
            working_dir: self.working_dir.clone(),
            swarm_id: self.swarm_id.clone(),
            swarm_enabled: self.swarm_enabled,
            status: SwarmLifecycleStatus::from(self.status.clone()),
            detail: self.detail.clone(),
            friendly_name: self.friendly_name.clone(),
            report_back_to_session_id: self.report_back_to_session_id.clone(),
            latest_completion_report: self.latest_completion_report.clone(),
            role: SwarmRole::from(self.role.clone()),
            is_headless: self.is_headless,
        }
    }

    pub fn live_attachments(&self) -> Vec<LiveSessionAttachment> {
        self.event_txs
            .iter()
            .map(|(connection_id, event_tx)| LiveSessionAttachment {
                connection_id: connection_id.clone(),
                event_tx: event_tx.clone(),
            })
            .collect()
    }

    pub fn from_record(
        record: SwarmMemberRecord,
        event_tx: mpsc::UnboundedSender<ServerEvent>,
    ) -> Self {
        Self {
            session_id: record.session_id,
            event_tx,
            event_txs: HashMap::new(),
            working_dir: record.working_dir,
            swarm_id: record.swarm_id,
            swarm_enabled: record.swarm_enabled,
            status: record.status.as_str().into_owned(),
            detail: record.detail,
            friendly_name: record.friendly_name,
            report_back_to_session_id: record.report_back_to_session_id,
            latest_completion_report: record.latest_completion_report,
            role: record.role.as_str().into_owned(),
            joined_at: Instant::now(),
            last_status_change: Instant::now(),
            is_headless: record.is_headless,
        }
    }
}

/// A shared context entry stored by the server
#[derive(Clone, Debug)]
pub struct SharedContext {
    pub key: String,
    pub value: String,
    pub from_session: String,
    pub from_name: Option<String>,
    /// When this context was created
    pub created_at: Instant,
    /// When this context was last updated
    pub updated_at: Instant,
}

/// Event types for real-time event subscription
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SwarmEventType {
    /// A file was touched (read/write/edit)
    FileTouch {
        path: String,
        op: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        intent: Option<String>,
        summary: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    /// A notification was broadcast
    Notification {
        notification_type: String,
        message: String,
    },
    /// A swarm plan was updated
    PlanUpdate { swarm_id: String, item_count: usize },
    /// A plan proposal was submitted
    PlanProposal {
        swarm_id: String,
        proposer_session: String,
        item_count: usize,
    },
    /// Shared context was updated
    ContextUpdate { swarm_id: String, key: String },
    /// Session status changed
    StatusChange {
        old_status: String,
        new_status: String,
    },
    /// Session joined/left swarm
    MemberChange {
        action: String, // "joined" or "left"
    },
}

/// A swarm event with metadata
#[derive(Clone, Debug)]
pub struct SwarmEvent {
    pub id: u64,
    pub session_id: String,
    pub session_name: Option<String>,
    pub swarm_id: Option<String>,
    pub event: SwarmEventType,
    pub timestamp: Instant,
    pub absolute_time: std::time::SystemTime,
}

/// Ring buffer for recent swarm events
pub(super) const MAX_EVENT_HISTORY: usize = 5000;

pub(super) type SessionInterruptQueues = Arc<RwLock<HashMap<String, SoftInterruptQueue>>>;

pub(super) async fn register_session_event_sender(
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    session_id: &str,
    connection_id: &str,
    event_tx: mpsc::UnboundedSender<ServerEvent>,
) {
    let mut members = swarm_members.write().await;
    if let Some(member) = members.get_mut(session_id) {
        // Only adopt this sender as the singular fallback when the existing
        // one is closed (e.g. the headless intake or a prior connection has
        // gone away). Overwriting an already-live fallback would shift where
        // subsequent direct `member.event_tx` users send their payloads,
        // which is one of the root causes of cross-connection wire
        // corruption observed in production logs.
        if member.event_tx.is_closed() {
            member.event_tx = event_tx.clone();
        }
        member.event_txs.insert(connection_id.to_string(), event_tx);
    }
}

pub(super) async fn unregister_session_event_sender(
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    session_id: &str,
    connection_id: &str,
) {
    let mut members = swarm_members.write().await;
    if let Some(member) = members.get_mut(session_id) {
        member.event_txs.remove(connection_id);
        // Intentionally do NOT silently re-point `member.event_tx` here. The
        // singular `event_tx` is a fallback set at member creation (headless
        // spawn intake). Silently swapping it to a surviving connection's
        // sender means later code that reads `member.event_tx` can deliver
        // events to a different connection's writer than intended, which has
        // shown up as cross-connection protocol corruption in the wild.
    }
}

pub(super) async fn fanout_session_event(
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    session_id: &str,
    event: ServerEvent,
) -> usize {
    let targets = {
        let mut members = swarm_members.write().await;
        let Some(member) = members.get_mut(session_id) else {
            return 0;
        };

        member.event_txs.retain(|_, tx| !tx.is_closed());

        if member.event_txs.is_empty() {
            vec![member.event_tx.clone()]
        } else {
            // Snapshot all live attachments. Do not mutate `member.event_tx`
            // here: this function is called from many fanout sites and the
            // HashMap iteration order is non-deterministic, so re-pointing
            // the singular fallback would silently re-route subsequent
            // direct uses of `member.event_tx` to an arbitrary connection.
            member.event_txs.values().cloned().collect::<Vec<_>>()
        }
    };

    let mut delivered = 0;
    for tx in targets {
        if tx.send(event.clone()).is_ok() {
            delivered += 1;
        }
    }
    delivered
}

pub(super) async fn fanout_live_client_event(
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    session_id: &str,
    event: ServerEvent,
) -> usize {
    let targets = {
        let mut members = swarm_members.write().await;
        let Some(member) = members.get_mut(session_id) else {
            return 0;
        };

        member.event_txs.retain(|_, tx| !tx.is_closed());
        member.event_txs.values().cloned().collect::<Vec<_>>()
    };

    let mut delivered = 0;
    for tx in targets {
        if tx.send(event.clone()).is_ok() {
            delivered += 1;
        }
    }
    delivered
}

pub(super) fn session_event_fanout_sender(
    session_id: String,
    swarm_members: Arc<RwLock<HashMap<String, SwarmMember>>>,
) -> mpsc::UnboundedSender<ServerEvent> {
    let (tx, mut rx) = mpsc::unbounded_channel::<ServerEvent>();
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            let _ = fanout_session_event(&swarm_members, &session_id, event).await;
        }
    });
    tx
}

pub(super) fn enqueue_soft_interrupt(
    queue: &SoftInterruptQueue,
    content: String,
    urgent: bool,
    source: SoftInterruptSource,
) -> bool {
    if let Ok(mut pending) = queue.lock() {
        pending.push(SoftInterruptMessage {
            content,
            urgent,
            source,
        });
        true
    } else {
        false
    }
}

/// Lock-free control-plane handles for a live session.
///
/// This intentionally exposes only out-of-band controls that are safe to use
/// while a turn owns the Agent mutex. Stateful operations such as history
/// mutation, model changes, or direct tool execution should continue to
/// coordinate through the Agent lock after the turn is idle/stopped.
#[derive(Clone)]
pub struct SessionControlHandle {
    pub session_id: String,
    soft_interrupt_queue: SoftInterruptQueue,
    background_tool_signal: Option<InterruptSignal>,
    stop_current_turn_signal: InterruptSignal,
}

impl SessionControlHandle {
    pub fn new(
        session_id: impl Into<String>,
        soft_interrupt_queue: SoftInterruptQueue,
        background_tool_signal: InterruptSignal,
        stop_current_turn_signal: InterruptSignal,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            soft_interrupt_queue,
            background_tool_signal: Some(background_tool_signal),
            stop_current_turn_signal,
        }
    }

    pub fn cancel_only(
        session_id: impl Into<String>,
        soft_interrupt_queue: SoftInterruptQueue,
        stop_current_turn_signal: InterruptSignal,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            soft_interrupt_queue,
            background_tool_signal: None,
            stop_current_turn_signal,
        }
    }

    pub fn queue_soft_interrupt(
        &self,
        content: String,
        urgent: bool,
        source: SoftInterruptSource,
    ) -> bool {
        enqueue_soft_interrupt(&self.soft_interrupt_queue, content, urgent, source)
    }

    pub fn clear_soft_interrupts(&self) {
        if let Ok(mut queue) = self.soft_interrupt_queue.lock() {
            queue.clear();
        }
    }

    pub fn request_cancel(&self) {
        self.stop_current_turn_signal.fire();
    }

    pub fn reset_cancel(&self) {
        self.stop_current_turn_signal.reset();
    }

    pub fn request_background_current_tool(&self) -> bool {
        if let Some(signal) = &self.background_tool_signal {
            signal.fire();
            true
        } else {
            false
        }
    }

    pub fn stop_current_turn_signal(&self) -> InterruptSignal {
        self.stop_current_turn_signal.clone()
    }
}

pub(super) async fn register_session_interrupt_queue(
    queues: &SessionInterruptQueues,
    session_id: &str,
    queue: SoftInterruptQueue,
) {
    let mut guard = queues.write().await;
    guard.insert(session_id.to_string(), queue);
}

pub(super) async fn rename_session_interrupt_queue(
    queues: &SessionInterruptQueues,
    old_session_id: &str,
    new_session_id: &str,
) {
    let mut guard = queues.write().await;
    if let Some(queue) = guard.remove(old_session_id) {
        guard.insert(new_session_id.to_string(), queue);
    }
}

pub(super) async fn remove_session_interrupt_queue(
    queues: &SessionInterruptQueues,
    session_id: &str,
) {
    let mut guard = queues.write().await;
    guard.remove(session_id);
}

pub(super) async fn queue_soft_interrupt_for_session(
    session_id: &str,
    content: String,
    urgent: bool,
    source: SoftInterruptSource,
    queues: &SessionInterruptQueues,
    sessions: &super::SessionAgents,
) -> bool {
    if let Some(queue) = queues.read().await.get(session_id).cloned() {
        return enqueue_soft_interrupt(&queue, content, urgent, source);
    }

    let queue = {
        let guard = sessions.read().await;
        guard.get(session_id).and_then(|agent| {
            agent
                .try_lock()
                .ok()
                .map(|agent_guard| agent_guard.soft_interrupt_queue())
        })
    };

    if let Some(queue) = queue {
        register_session_interrupt_queue(queues, session_id, queue.clone()).await;
        enqueue_soft_interrupt(&queue, content, urgent, source)
    } else {
        let session_exists = {
            let guard = sessions.read().await;
            guard.contains_key(session_id)
        } || crate::session::session_exists(session_id);

        if !session_exists {
            return false;
        }

        crate::soft_interrupt_store::append(
            session_id,
            SoftInterruptMessage {
                content,
                urgent,
                source,
            },
        )
        .map(|_| true)
        .unwrap_or_else(|err| {
            crate::logging::warn(&format!(
                "Failed to persist deferred soft interrupt for session {}: {}",
                session_id, err
            ));
            false
        })
    }
}

#[cfg(test)]
mod multi_connection_protocol_tests {
    //! Regression tests for the multi-session protocol corruption that showed
    //! up as truncated/interleaved JSON lines on the wire (e.g.
    //! `nt","is_headless":true,...`) and `Remote protocol error is not
    //! retryable` reconnect-loop terminations in
    //! `~/.jcode/logs/jcode-2026-05-21.log`.
    //!
    //! Root cause: several server code paths used the singular fallback
    //! `member.event_tx` directly instead of fanning out to all
    //! `member.event_txs`, AND `register/unregister/fanout_session_event`
    //! silently overwrote `member.event_tx` to point at whichever
    //! connection's writer happened to be touched last. That meant a `send`
    //! intended for one client's writer could land on another client's
    //! writer mid-line, splicing event tails into unrelated frames.

    use super::*;
    use crate::protocol::ServerEvent;

    fn fresh_member(
        session_id: &str,
        fallback_tx: mpsc::UnboundedSender<ServerEvent>,
    ) -> SwarmMember {
        SwarmMember {
            session_id: session_id.to_string(),
            event_tx: fallback_tx,
            event_txs: HashMap::new(),
            working_dir: None,
            swarm_id: None,
            swarm_enabled: false,
            status: "ready".to_string(),
            detail: None,
            friendly_name: None,
            report_back_to_session_id: None,
            latest_completion_report: None,
            role: "agent".to_string(),
            joined_at: Instant::now(),
            last_status_change: Instant::now(),
            is_headless: true,
        }
    }

    fn drain<T>(rx: &mut mpsc::UnboundedReceiver<T>) -> Vec<T> {
        let mut out = Vec::new();
        while let Ok(v) = rx.try_recv() {
            out.push(v);
        }
        out
    }

    /// `register_session_event_sender` must NOT silently overwrite the
    /// singular `event_tx` fallback while it is still live. Doing so caused
    /// later direct `member.event_tx.send(...)` callers to deliver to a
    /// different connection's writer than intended.
    #[tokio::test]
    async fn register_does_not_overwrite_live_fallback_sender() {
        let (fallback_tx, mut fallback_rx) = mpsc::unbounded_channel::<ServerEvent>();
        let (conn_a_tx, _conn_a_rx) = mpsc::unbounded_channel::<ServerEvent>();

        let mut map = HashMap::new();
        map.insert("sess-1".to_string(), fresh_member("sess-1", fallback_tx));
        let members = Arc::new(RwLock::new(map));

        register_session_event_sender(&members, "sess-1", "conn-a", conn_a_tx).await;

        // Fallback must still point at the original headless intake.
        let guard = members.read().await;
        let member = guard.get("sess-1").expect("member exists");
        member
            .event_tx
            .send(ServerEvent::Ack { id: 7 })
            .expect("fallback sender still alive");
        drop(guard);

        let delivered = drain(&mut fallback_rx);
        assert_eq!(delivered.len(), 1, "fallback receiver got the event");
    }

    /// When the live fallback is closed, `register_session_event_sender` is
    /// allowed to adopt the new sender so the member is not stranded.
    #[tokio::test]
    async fn register_adopts_new_sender_when_fallback_is_closed() {
        let (fallback_tx, fallback_rx) = mpsc::unbounded_channel::<ServerEvent>();
        drop(fallback_rx); // closes the channel

        let (conn_a_tx, mut conn_a_rx) = mpsc::unbounded_channel::<ServerEvent>();

        let mut map = HashMap::new();
        map.insert("sess-1".to_string(), fresh_member("sess-1", fallback_tx));
        let members = Arc::new(RwLock::new(map));

        register_session_event_sender(&members, "sess-1", "conn-a", conn_a_tx).await;

        let guard = members.read().await;
        let member = guard.get("sess-1").expect("member exists");
        member
            .event_tx
            .send(ServerEvent::Ack { id: 42 })
            .expect("new fallback should be live");
        drop(guard);

        let delivered = drain(&mut conn_a_rx);
        assert_eq!(delivered.len(), 1, "new live conn picked up the fallback");
    }

    /// `unregister_session_event_sender` must not re-point the singular
    /// `event_tx` to a surviving connection. The fallback is owned by
    /// member-creation (headless intake) and silently swapping it caused
    /// cross-wired writes between connections.
    #[tokio::test]
    async fn unregister_does_not_repoint_fallback_to_survivor() {
        let (fallback_tx, mut fallback_rx) = mpsc::unbounded_channel::<ServerEvent>();
        let (conn_a_tx, _conn_a_rx) = mpsc::unbounded_channel::<ServerEvent>();
        let (conn_b_tx, mut conn_b_rx) = mpsc::unbounded_channel::<ServerEvent>();

        let mut map = HashMap::new();
        map.insert("sess-1".to_string(), fresh_member("sess-1", fallback_tx));
        let members = Arc::new(RwLock::new(map));

        register_session_event_sender(&members, "sess-1", "conn-a", conn_a_tx).await;
        register_session_event_sender(&members, "sess-1", "conn-b", conn_b_tx).await;

        // conn-a goes away.
        unregister_session_event_sender(&members, "sess-1", "conn-a").await;

        // Sending to the singular `event_tx` must land on the original
        // fallback receiver, NOT on conn-b's writer.
        {
            let guard = members.read().await;
            let member = guard.get("sess-1").expect("member exists");
            member
                .event_tx
                .send(ServerEvent::Ack { id: 1 })
                .expect("fallback still alive");
        }

        let fallback_msgs = drain(&mut fallback_rx);
        let conn_b_msgs = drain(&mut conn_b_rx);
        assert_eq!(
            fallback_msgs.len(),
            1,
            "fallback receiver should get the event"
        );
        assert!(
            conn_b_msgs.is_empty(),
            "surviving connection must NOT silently absorb fallback traffic"
        );
    }

    /// `fanout_session_event` must deliver to every live attachment exactly
    /// once and must not mutate the singular `event_tx` to point at one of
    /// them. Without this guarantee, subsequent direct `event_tx` users
    /// would deliver to an arbitrary connection (HashMap iteration order),
    /// which is exactly the wire-corruption pattern observed in production.
    #[tokio::test]
    async fn fanout_delivers_to_all_connections_and_does_not_mutate_fallback() {
        let (fallback_tx, mut fallback_rx) = mpsc::unbounded_channel::<ServerEvent>();
        let (conn_a_tx, mut conn_a_rx) = mpsc::unbounded_channel::<ServerEvent>();
        let (conn_b_tx, mut conn_b_rx) = mpsc::unbounded_channel::<ServerEvent>();

        let mut map = HashMap::new();
        map.insert("sess-1".to_string(), fresh_member("sess-1", fallback_tx));
        let members = Arc::new(RwLock::new(map));

        register_session_event_sender(&members, "sess-1", "conn-a", conn_a_tx).await;
        register_session_event_sender(&members, "sess-1", "conn-b", conn_b_tx).await;

        let delivered =
            fanout_session_event(&members, "sess-1", ServerEvent::Ack { id: 99 }).await;
        assert_eq!(delivered, 2, "fanout reaches both live attachments");

        let a = drain(&mut conn_a_rx);
        let b = drain(&mut conn_b_rx);
        assert_eq!(a.len(), 1, "conn-a got exactly one copy");
        assert_eq!(b.len(), 1, "conn-b got exactly one copy");

        // Fallback receiver must NOT have received anything: when live
        // attachments exist, fanout snapshots `event_txs` and does NOT
        // duplicate to `event_tx`.
        let fb = drain(&mut fallback_rx);
        assert!(
            fb.is_empty(),
            "fanout must not duplicate to the singular fallback when live conns exist"
        );

        // And the singular `event_tx` must STILL be the original fallback
        // (i.e. fanout did not silently re-point it). The cheapest check is
        // to send via `event_tx` and confirm the fallback receiver gets it.
        {
            let guard = members.read().await;
            let member = guard.get("sess-1").expect("member exists");
            member
                .event_tx
                .send(ServerEvent::Ack { id: 100 })
                .expect("fallback sender still alive");
        }
        let fb_after = drain(&mut fallback_rx);
        assert_eq!(
            fb_after.len(),
            1,
            "singular event_tx must remain pointed at the original fallback"
        );
        assert!(
            drain(&mut conn_a_rx).is_empty()
                && drain(&mut conn_b_rx).is_empty(),
            "direct fallback send must NOT bleed into live connections"
        );
    }

    /// When no live attachments are registered, fanout falls back to the
    /// singular `event_tx` so headless-only sessions still receive events.
    #[tokio::test]
    async fn fanout_falls_back_to_singular_sender_when_no_live_conns() {
        let (fallback_tx, mut fallback_rx) = mpsc::unbounded_channel::<ServerEvent>();

        let mut map = HashMap::new();
        map.insert("sess-1".to_string(), fresh_member("sess-1", fallback_tx));
        let members = Arc::new(RwLock::new(map));

        let delivered =
            fanout_session_event(&members, "sess-1", ServerEvent::Ack { id: 1 }).await;
        assert_eq!(delivered, 1, "fanout used the singular fallback");

        let msgs = drain(&mut fallback_rx);
        assert_eq!(msgs.len(), 1, "fallback receiver got the event");
    }
}
