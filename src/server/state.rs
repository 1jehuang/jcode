use crate::bus::FileOp;
use crate::plan::PlanItem;
use crate::protocol::ServerEvent;
use jcode_agent_runtime::{SoftInterruptMessage, SoftInterruptQueue, SoftInterruptSource};
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
        access.session_id != current_session_id && swarm_session_ids.contains(&access.session_id)
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

/// A versioned plan for a swarm.
#[derive(Clone, Debug)]
pub struct VersionedPlan {
    pub items: Vec<PlanItem>,
    pub version: u64,
    /// Session ids that should receive this plan's updates.
    pub participants: HashSet<String>,
}

impl VersionedPlan {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            version: 0,
            participants: HashSet::new(),
        }
    }
}

impl Default for VersionedPlan {
    fn default() -> Self {
        Self::new()
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
        member.event_tx = event_tx.clone();
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
        if let Some((_, tx)) = member.event_txs.iter().next() {
            member.event_tx = tx.clone();
        }
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
            if let Some((_, tx)) = member.event_txs.iter().next() {
                member.event_tx = tx.clone();
            }
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
