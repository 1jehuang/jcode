use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, RwLock};
use uuid::Uuid;
use chrono::{DateTime, Utc};
use serde::{Serialize, Deserialize};

use crate::utils::rope::Rope;

pub struct CollaborationServer {
    sessions: RwLock<HashMap<CollabSessionId, CollabSession>>,
    participants: RwLock<HashMap<ParticipantId, Participant>>,
    document_store: Arc<DocumentStore>,
    change_broadcast: broadcast::Sender<ServerPushMessage>,
    presence: PresenceManager,
    conflict_resolver: ConflictResolver,
    config: CollabConfig,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CollabSessionId(Uuid);

impl CollabSessionId {
    pub fn new() -> Self { CollabSessionId(Uuid::new_v4()) }
    pub fn as_uuid(&self) -> &Uuid { &self.0 }
}

#[derive(Clone)]
pub struct CollabSession {
    pub id: CollabSessionId,
    pub document: CollaborativeDocument,
    pub participants: HashSet<ParticipantId>,
    pub owner_id: ParticipantId,
    pub created_at: DateTime<Utc>,
    pub settings: CollabSettings,
    pub history: OperationLog,
}

#[derive(Clone)]
pub struct CollaborativeDocument {
    pub doc_id: Uuid,
    pub content: Rope,
    pub version: VectorClock,
    pub history: OperationLog,
}

pub struct CrdtDocument {
    pub text: Rope,
    pub cursors: HashMap<ParticipantId, CursorState>,
    pub selections: HashMap<ParticipantId, SelectionRange>,
    pub last_operation_id: u64,
}

impl CrdtDocument {
    pub fn new() -> Self {
        CrdtDocument {
            text: Rope::new(),
            cursors: HashMap::new(),
            selections: HashMap::new(),
            last_operation_id: 0,
        }
    }

    pub fn from_str(s: &str) -> Self {
        CrdtDocument {
            text: Rope::from_str(s),
            cursors: HashMap::new(),
            selections: HashMap::new(),
            last_operation_id: 0,
        }
    }

    pub fn next_op_id(&mut self) -> u64 {
        self.last_operation_id += 1;
        self.last_operation_id
    }
}

#[derive(Clone)]
struct CursorState {
    participant_id: ParticipantId,
    position: Position,
    anchor: Position,
    last_updated: DateTime<Utc>,
    selection_mode: SelectionMode,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelectionMode { Normal, Word, Line, Block }

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Position {
    pub line: usize,
    pub column: usize,
}

impl Position {
    pub fn new(line: usize, column: usize) -> Self { Position { line, column } }
    pub fn zero() -> Self { Position { line: 0, column: 0 } }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectionRange {
    pub start: Position,
    pub end: Position,
}

impl SelectionRange {
    pub fn new(start: Position, end: Position) -> Self { SelectionRange { start, end } }
}

#[derive(Clone)]
pub struct Participant {
    pub id: ParticipantId,
    pub user_id: UserId,
    pub display_name: String,
    pub avatar: Option<AvatarUrl>,
    pub role: ParticipantRole,
    pub permissions: PermissionSet,
    pub connection: ConnectionHandle,
    pub joined_at: DateTime<Utc>,
    pub last_activity: DateTime<Utc>,
}

#[derive(Clone, Debug, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct ParticipantId(pub Uuid);

impl ParticipantId {
    pub fn new() -> Self { ParticipantId(Uuid::new_v4()) }
}

#[derive(Clone, Debug, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub enum UserId { Anonymous, Registered { id: Uuid, email: String } }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ParticipantRole { Owner, Editor, Viewer, Commenter }

pub type AvatarUrl = String;

#[derive(Clone)]
pub struct PermissionSet {
    can_edit: bool,
    can_delete: bool,
    can_invite: bool,
    can_change_settings: bool,
    can_export: bool,
}

impl PermissionSet {
    pub fn owner() -> Self {
        PermissionSet { can_edit: true, can_delete: true, can_invite: true, can_change_settings: true, can_export: true }
    }
    pub fn editor() -> Self {
        PermissionSet { can_edit: true, can_delete: false, can_invite: true, can_change_settings: false, can_export: true }
    }
    pub fn viewer() -> Self {
        PermissionSet { can_edit: false, can_delete: false, can_invite: false, can_change_settings: false, can_export: true }
    }
    pub fn commenter() -> Self {
        PermissionSet { can_edit: false, can_delete: false, can_invite: false, can_change_settings: false, can_export: false }
    }

    pub fn can_edit(&self) -> bool { self.can_edit }
    pub fn can_delete(&self) -> bool { self.can_delete }
    pub fn can_invite(&self) -> bool { self.can_invite }
    pub fn can_change_settings(&self) -> bool { self.can_change_settings }
    pub fn can_export(&self) -> bool { self.can_export }
}

pub type ServerPushMessageReceiver = broadcast::Receiver<ServerPushMessage>;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ServerPushMessage {
    TextInserted { participant: ParticipantId, position: Position, text: String, op_id: u64 },
    TextDeleted { participant: ParticipantId, range: SelectionRange, op_id: u64 },
    CursorMoved { participant: ParticipantId, position: Position },
    SelectionChanged { participant: ParticipantId, selection: SelectionRange },
    ParticipantJoined { info: ParticipantInfo },
    ParticipantLeft { participant_id: ParticipantId },
    ConflictDetected { conflict: ConflictInfo },
    DocumentSaved { version: VectorClock },
}

pub struct ConflictResolver {
    strategy: MergeStrategy,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MergeStrategy { LastWriteWins, FirstWriteWins, ManualMerge, OperationalTransform }

impl ConflictResolver {
    pub fn new(strategy: MergeStrategy) -> Self { ConflictResolver { strategy } }

    pub fn resolve_concurrent_edits(
        &self,
        local_op: &TextOperation,
        remote_ops: &[RemoteOperation],
        base_version: &VectorClock,
    ) -> ResolvedOperations {
        let mut operations = vec![local_op.clone()];
        let mut had_conflicts = false;
        let mut conflict_descriptions = Vec::new();

        for remote in remote_ops {
            if base_version.is_concurrent(&remote.operation.version) {
                had_conflicts = true;
                match self.strategy {
                    MergeStrategy::LastWriteWins => {
                        if remote.operation.timestamp > local_op.timestamp {
                            operations[0] = remote.operation.clone();
                            conflict_descriptions.push(format!(
                                "Remote op at {:?} overrode local op",
                                remote.operation.timestamp
                            ));
                        }
                    }
                    MergeStrategy::FirstWriteWins => {
                        conflict_descriptions.push(format!(
                            "Local op at {:?} kept over remote op",
                            local_op.timestamp
                        ));
                    }
                    MergeStrategy::OperationalTransform => {
                        let transformed = Self::transform_against(local_op, &remote.operation);
                        operations[0] = transformed;
                        conflict_descriptions.push("OT transformation applied".to_string());
                    }
                    MergeStrategy::ManualMerge => {
                        conflict_descriptions.push("Manual merge required".to_string());
                    }
                }
            } else {
                operations.push(remote.operation.clone());
            }
        }

        ResolvedOperations { operations, had_conflicts, conflict_descriptions }
    }

    fn transform_against(local: &TextOperation, remote: &TextOperation) -> TextOperation {
        match (&local.op_type, &remote.op_type) {
            (OpType::Insert, OpType::Insert) => {
                if remote.position <= local.position || remote.position == local.position {
                    let new_pos = Position {
                        line: local.position.line + (remote.text.len() - remote.text.chars().filter(|&c| c == '\n').count()).min(100),
                        column: local.column_offset_after_insert(remote),
                    };
                    TextOperation { position: new_pos, ..local.clone() }
                } else {
                    local.clone()
                }
            }
            _ => local.clone()
        }
    }
}

impl TextOperation {
    fn column_offset_after_insert(&self, other: &TextOperation) -> usize {
        if other.position.line == self.position.line {
            self.position.column.saturating_add(other.text.len())
        } else {
            self.position.column
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextOperation {
    pub op_type: OpType,
    pub position: Position,
    pub text: String,
    pub participant_id: ParticipantId,
    pub timestamp: DateTime<Utc>,
    pub version: VectorClock,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpType { Insert, Delete, Replace }

pub struct RemoteOperation {
    pub operation: TextOperation,
    pub received_at: DateTime<Utc>,
}

pub struct ResolvedOperations {
    pub operations: Vec<TextOperation>,
    pub had_conflicts: bool,
    pub conflict_descriptions: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConflictInfo {
    pub local_op: TextOperation,
    pub conflicting_ops: Vec<TextOperation>,
    pub suggestion: ConflictSuggestion,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConflictSuggestion { UseLocal, UseRemote, ManualResolution { merged: String } }

pub struct PresenceManager {
    online: RwLock<HashMap<ParticipantId, PresenceState>>,
    typing_indicators: RwLock<HashMap<ParticipantId, TypingState>>,
}

impl PresenceManager {
    pub fn new() -> Self {
        PresenceManager {
            online: RwLock::new(HashMap::new()),
            typing_indicators: RwLock::new(HashMap::new()),
        }
    }

    pub async fn mark_online(&self, participant_id: &ParticipantId, doc_id: Option<Uuid>) {
        self.online.write().await.insert(participant_id.clone(), PresenceState {
            is_online: true,
            last_seen: Utc::now(),
            current_document: doc_id,
            idle_duration: Duration::ZERO,
        });
    }

    pub async fn mark_offline(&self, participant_id: &ParticipantId) {
        self.online.write().await.remove(participant_id);
        self.typing_indicators.write().await.remove(participant_id);
    }

    pub async fn is_online(&self, participant_id: &ParticipantId) -> bool {
        self.online.read().await.get(participant_id).map_or(false, |s| s.is_online)
    }

    pub async fn set_typing(&self, participant_id: &ParticipantId, document_id: Uuid) {
        self.typing_indicators.write().await.insert(participant_id.clone(), TypingState {
            is_typing: true,
            last_type_event: Utc::now(),
            document_id: Some(document_id),
        });
    }

    pub async fn clear_typing(&self, participant_id: &ParticipantId) {
        if let Some(state) = self.typing_indicators.write().await.get_mut(participant_id) {
            state.is_typing = false;
        }
    }

    pub async fn online_count(&self) -> usize { self.online.read().await.len() }
}

pub struct PresenceState {
    pub is_online: bool,
    pub last_seen: DateTime<Utc>,
    pub current_document: Option<Uuid>,
    pub idle_duration: Duration,
}

pub struct TypingState {
    pub is_typing: bool,
    pub last_type_event: DateTime<Utc>,
    pub document_id: Option<Uuid>,
}

#[derive(Clone)]
pub struct OperationLog {
    operations: Vec<LoggedOperation>,
    max_size: usize,
}

impl OperationLog {
    pub fn new(max_size: usize) -> Self {
        OperationLog { operations: Vec::new(), max_size }
    }

    pub fn push(&mut self, op: LoggedOperation) {
        if self.operations.len() >= self.max_size {
            self.operations.remove(0);
        }
        self.operations.push(op);
    }

    pub fn len(&self) -> usize { self.operations.len() }
    pub fn is_empty(&self) -> bool { self.operations.is_empty() }
    pub fn iter(&self) -> impl Iterator<Item = &LoggedOperation> { self.operations.iter() }

    pub fn since_version(&self, version: &VectorClock) -> Vec<&LoggedOperation> {
        self.operations.iter().skip_while(|op| !version.happens_before(&op.vector_clock)).collect()
    }
}

#[derive(Clone)]
struct LoggedOperation {
    id: u64,
    operation: TextOperation,
    vector_clock: VectorClock,
    timestamp: DateTime<Utc>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct VectorClock(HashMap<ParticipantId, u64>);

impl VectorClock {
    pub fn increment(&mut self, participant: &ParticipantId) {
        let entry = self.0.entry(participant.clone()).or_insert(0);
        *entry += 1;
    }

    pub fn get(&self, participant: &ParticipantId) -> u64 {
        self.0.get(participant).copied().unwrap_or(0)
    }

    pub fn merge(&self, other: &VectorClock) -> VectorClock {
        let mut merged = HashMap::new();
        for (k, v) in &self.0 { merged.insert(k.clone(), *v); }
        for (k, v) in &other.0 {
            merged.entry(k.clone()).and_modify(|e| *e = (*v).max(*e)).or_insert(*v);
        }
        VectorClock(merged)
    }

    pub fn happens_before(&self, other: &VectorClock) -> bool {
        if self == other { return false; }
        for (k, v) in &self.0 {
            if other.0.get(k).map_or(false, |ov| *v > *ov) { return false; }
        }
        for k in other.0.keys() {
            if !self.0.contains_key(k) && other.0[k] > 0 { return false; }
        }
        self.0.keys().any(|k| other.0.get(k) != self.0.get(k)) || self.0.len() != other.0.len()
    }

    pub fn is_concurrent(&self, other: &VectorClock) -> bool {
        !self.happens_before(other) && !other.happens_before(self)
    }

    pub fn entry_count(&self) -> usize { self.0.len() }
}

pub struct CollabConfig {
    pub max_participants_per_session: usize,
    pub max_document_size_bytes: usize,
    pub session_timeout: Duration,
    pub typing_indicator_timeout: Duration,
    pub auto_save_interval: Duration,
    pub max_history_operations: usize,
}

impl Default for CollabConfig {
    fn default() -> Self {
        CollabConfig {
            max_participants_per_session: 50,
            max_document_size_bytes: 10 * 1024 * 1024,
            session_timeout: Duration::from_secs(3600),
            typing_indicator_timeout: Duration::from_secs(3),
            auto_save_interval: Duration::from_secs(30),
            max_history_operations: 1000,
        }
    }
}

#[derive(Clone)]
pub struct CollabSettings {
    pub language: Option<String>,
    pub tab_size: usize,
    pub insert_spaces: bool,
    pub word_wrap: bool,
    pub show_line_numbers: bool,
    pub theme: String,
}

impl Default for CollabSettings {
    fn default() -> Self {
        CollabSettings {
            language: None,
            tab_size: 4,
            insert_spaces: true,
            word_wrap: false,
            show_line_numbers: true,
            theme: "dark".to_string(),
        }
    }
}

type ConnectionHandle = ();

impl CollaborationServer {
    pub fn new(config: CollabConfig) -> Self {
        let (tx, _) = broadcast::channel(4096);
        CollaborationServer {
            sessions: RwLock::new(HashMap::new()),
            participants: RwLock::new(HashMap::new()),
            document_store: Arc::new(DocumentStore::new()),
            change_broadcast: tx,
            presence: PresenceManager::new(),
            conflict_resolver: ConflictResolver::new(MergeStrategy::OperationalTransform),
            config,
        }
    }

    pub async fn create_session(&self, owner: &Participant, initial_content: &str) -> Result<CollabSession, String> {
        let session_id = CollabSessionId::new();
        let doc_id = Uuid::new_v4();
        let now = Utc::now();

        let crdt_doc = CrdtDocument::from_str(initial_content);
        let mut version = VectorClock::default();
        version.increment(&owner.id);

        let session = CollabSession {
            id: session_id.clone(),
            document: CollaborativeDocument {
                doc_id,
                content: crdt_doc.text.clone(),
                version: version.clone(),
                history: OperationLog::new(self.config.max_history_operations),
            },
            participants: HashSet::from([owner.id.clone()]),
            owner_id: owner.id.clone(),
            created_at: now,
            settings: CollabSettings::default(),
            history: OperationLog::new(self.config.max_history_operations),
        };

        self.participants.write().await.insert(owner.id.clone(), owner.clone());
        self.sessions.write().await.insert(session_id.clone(), session.clone());
        self.document_store.store(doc_id, &crdt_doc.text, &owner.id, &version).await;

        Ok(session)
    }

    pub async fn join_session(&self, session_id: &CollabSessionId, participant: &Participant) -> Result<JoinResult, String> {
        let mut sessions = self.sessions.write().await;
        let session = sessions.get_mut(session_id).ok_or("Session not found")?;
        if session.participants.len() >= self.config.max_participants_per_session {
            return Err("Session full".to_string());
        }
        session.participants.insert(participant.id.clone());
        self.participants.write().await.insert(participant.id.clone(), participant.clone());

        // 异步读取参与者映射（不能在非 async 闭包中 await）
        let session_clone = session.clone();
        drop(sessions);
        let participants_read = self.participants.read().await;
        let existing_info: Vec<ParticipantInfo> = session_clone.participants.iter()
            .filter_map(|pid| participants_read.get(pid).map(ParticipantInfo::from))
            .collect();
        drop(participants_read);

        let document_content = session_clone.document.content.to_string();
        Ok(JoinResult {
            session: session_clone,
            document_content,
            existing_participants: existing_info,
            missed_operations: Vec::new(),
        })
    }

    pub async fn apply_edit(&self, session_id: &CollabSessionId, participant_id: &ParticipantId, edit: &TextEdit) -> Result<EditResult, String> {
        let mut sessions = self.sessions.write().await;
        let session = sessions.get_mut(session_id).ok_or("Session not found")?;
        if !session.participants.contains(participant_id) {
            return Err("Not a participant".to_string());
        }

        let _old_text = session.document.content.to_string();
        let new_rope = if let Some(ref range) = edit.old_range {
            let start_pos = session.document.content.line_to_pos(range.start.line) + range.start.column;
            let end_pos = session.document.content.line_to_pos(range.end.line) + range.end.column;
            session.document.content.replace(start_pos..end_pos, &edit.new_text)
        } else {
            let pos = session.document.content.line_to_pos(edit.position.line) + edit.position.column;
            session.document.content.insert(pos, &edit.new_text)
        };

        let mut new_version = session.document.version.clone();
        new_version.increment(participant_id);

        let _op = TextOperation {
            op_type: if edit.old_range.is_some() { OpType::Replace } else { OpType::Insert },
            position: edit.position.clone(),
            text: edit.new_text.clone(),
            participant_id: participant_id.clone(),
            timestamp: Utc::now(),
            version: new_version.clone(),
        };

        session.document.content = new_rope;
        session.document.version = new_version.clone();

        let broadcast_to: Vec<ParticipantId> = session.participants.iter()
            .filter(|p| **p != *participant_id)
            .cloned()
            .collect();

        let _ = self.change_broadcast.send(ServerPushMessage::TextInserted {
            participant: participant_id.clone(),
            position: edit.position.clone(),
            text: edit.new_text.clone(),
            op_id: session.document.history.len() as u64,
        });

        Ok(EditResult {
            success: true,
            new_version,
            conflicts: Vec::new(),
            broadcast_to,
        })
    }

    pub fn broadcast_cursor_move(&self, _session_id: &CollabSessionId, participant: &ParticipantId, position: Position) {
        let _ = self.change_broadcast.send(ServerPushMessage::CursorMoved {
            participant: participant.clone(),
            position,
        });
    }

    pub async fn get_online_participants(&self, session_id: &CollabSessionId) -> Vec<ParticipantInfo> {
        let sessions = self.sessions.read().await;
        let session = match sessions.get(session_id) { Some(s) => s, None => return Vec::new() };
        let parts = self.participants.read().await;
        session.participants.iter()
            .filter_map(|pid| parts.get(pid).map(|p| ParticipantInfo::from(p)))
            .collect()
    }

    pub async fn handle_reconnect(&self, participant_id: &ParticipantId, missed_ops: &[ServerPushMessage]) -> Result<SyncState, String> {
        let sessions = self.sessions.read().await;
        let session = sessions.values().next().ok_or("No sessions")?;

        let mut cursor_states = HashMap::new();
        for msg in missed_ops {
            if let ServerPushMessage::CursorMoved { participant, position } = msg {
                cursor_states.insert(participant.clone(), CursorState {
                    participant_id: participant.clone(),
                    position: position.clone(),
                    anchor: position.clone(),
                    last_updated: Utc::now(),
                    selection_mode: SelectionMode::Normal,
                });
            }
        }

        Ok(SyncState {
            current_content: session.document.content.to_string(),
            operations_to_apply: missed_ops.to_vec(),
            cursor_states,
        })
    }

    pub fn subscribe(&self) -> ServerPushMessageReceiver { self.change_broadcast.subscribe() }

    pub async fn save_document(&self, session_id: &CollabSessionId) -> Result<SaveResult, String> {
        let sessions = self.sessions.read().await;
        let session = sessions.get(session_id).ok_or("Session not found")?;
        let size_bytes = session.document.content.byte_len();
        let saved_at = Utc::now();

        self.document_store.store(
            session.document.doc_id,
            &session.document.content,
            &session.owner_id,
            &session.document.version,
        ).await;

        let _ = self.change_broadcast.send(ServerPushMessage::DocumentSaved {
            version: session.document.version.clone(),
        });

        Ok(SaveResult {
            success: true,
            version: session.document.version.clone(),
            size_bytes,
            saved_at,
        })
    }
}

pub struct TextEdit {
    pub position: Position,
    pub new_text: String,
    pub old_range: Option<SelectionRange>,
}

pub struct JoinResult {
    pub session: CollabSession,
    pub document_content: String,
    pub existing_participants: Vec<ParticipantInfo>,
    pub missed_operations: Vec<ServerPushMessage>,
}

pub struct EditResult {
    pub success: bool,
    pub new_version: VectorClock,
    pub conflicts: Vec<ConflictInfo>,
    pub broadcast_to: Vec<ParticipantId>,
}

pub struct SyncState {
    pub current_content: String,
    pub operations_to_apply: Vec<ServerPushMessage>,
    pub cursor_states: HashMap<ParticipantId, CursorState>,
}

pub struct SaveResult {
    pub success: bool,
    pub version: VectorClock,
    pub size_bytes: usize,
    pub saved_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ParticipantInfo {
    pub id: ParticipantId,
    pub display_name: String,
    pub role: ParticipantRole,
    pub color: RgbaColor,
    pub is_online: bool,
    pub is_typing: bool,
    pub cursor: Option<Position>,
}

impl From<&Participant> for ParticipantInfo {
    fn from(p: &Participant) -> Self {
        ParticipantInfo {
            id: p.id.clone(),
            display_name: p.display_name.clone(),
            role: p.role.clone(),
            color: RgbaColor::from_hash(&p.id.0),
            is_online: true,
            is_typing: false,
            cursor: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RgbaColor { pub r: u8, pub g: u8, pub b: u8, pub a: u8 }

impl RgbaColor {
    pub fn random() -> Self {
        use std::hash::{Hash, Hasher};
        use std::collections::hash_map::DefaultHasher;
        let mut h = DefaultHasher::new();
        Uuid::new_v4().hash(&mut h);
        let hash = h.finish();
        RgbaColor {
            r: ((hash >> 24) & 0xFF) as u8,
            g: ((hash >> 16) & 0xFF) as u8,
            b: ((hash >> 8) & 0xFF) as u8,
            a: 255,
        }
    }

    pub fn from_hash(uuid: &Uuid) -> Self {
        use std::hash::{Hash, Hasher};
        use std::collections::hash_map::DefaultHasher;
        let mut h = DefaultHasher::new();
        uuid.hash(&mut h);
        let hash = h.finish();
        RgbaColor {
            r: ((hash >> 24) & 0xFF) as u8,
            g: ((hash >> 16) & 0xFF) as u8,
            b: ((hash >> 8) & 0xFF) as u8,
            a: 255,
        }
    }

    pub fn from_str(hex: &str) -> Option<Self> {
        let hex = hex.trim_start_matches('#');
        if hex.len() == 6 {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            Some(RgbaColor { r, g, b, a: 255 })
        } else if hex.len() == 8 {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            let a = u8::from_str_radix(&hex[6..8], 16).ok()?;
            Some(RgbaColor { r, g, b, a })
        } else {
            None
        }
    }

    pub fn to_hex_string(&self) -> String {
        format!("#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a)
    }
}

pub struct DocumentStore {
    documents: RwLock<HashMap<Uuid, StoredDocument>>,
}

struct StoredDocument {
    content: Rope,
    saved_at: DateTime<Utc>,
    versions: Vec<DocumentVersion>,
}

struct DocumentVersion {
    version: VectorClock,
    saved_at: DateTime<Utc>,
    content_hash: u64,
    author: ParticipantId,
}

impl DocumentStore {
    pub fn new() -> Self {
        DocumentStore { documents: RwLock::new(HashMap::new()) }
    }

    pub async fn store(&self, doc_id: Uuid, content: &Rope, author: &ParticipantId, version: &VectorClock) {
        use std::hash::{Hash, Hasher};
        use std::collections::hash_map::DefaultHasher;
        let mut hasher = DefaultHasher::new();
        content.to_string().hash(&mut hasher);
        let content_hash = hasher.finish();

        let stored = StoredDocument {
            content: content.clone(),
            saved_at: Utc::now(),
            versions: vec![DocumentVersion {
                version: version.clone(),
                saved_at: Utc::now(),
                content_hash,
                author: author.clone(),
            }],
        };
        self.documents.write().await.insert(doc_id, stored);
    }

    pub async fn load(&self, doc_id: Uuid) -> Option<Rope> {
        self.documents.read().await.get(&doc_id).map(|d| d.content.clone())
    }

    pub async fn version_count(&self, doc_id: Uuid) -> usize {
        self.documents.read().await.get(&doc_id).map_or(0, |d| d.versions.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_position_new_and_zero() {
        let p = Position::new(5, 10);
        assert_eq!(p.line, 5);
        assert_eq!(p.column, 10);
        let z = Position::zero();
        assert_eq!(z.line, 0);
        assert_eq!(z.column, 0);
    }

    #[test]
    fn test_selection_range_new() {
        let start = Position::new(1, 1);
        let end = Position::new(3, 5);
        let range = SelectionRange::new(start.clone(), end.clone());
        assert_eq!(range.start, start);
        assert_eq!(range.end, end);
    }

    #[test]
    fn test_participant_id_generation() {
        let id1 = ParticipantId::new();
        let id2 = ParticipantId::new();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_session_id_generation() {
        let id1 = CollabSessionId::new();
        let id2 = CollabSessionId::new();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_permission_set_roles() {
        let owner = PermissionSet::owner();
        assert!(owner.can_edit());
        assert!(owner.can_delete());
        assert!(owner.can_invite());
        assert!(owner.can_change_settings());
        assert!(owner.can_export());

        let editor = PermissionSet::editor();
        assert!(editor.can_edit());
        assert!(!editor.can_delete());
        assert!(editor.can_invite());
        assert!(!editor.can_change_settings());

        let viewer = PermissionSet::viewer();
        assert!(!viewer.can_edit());
        assert!(!viewer.can_delete());
        assert!(!viewer.can_invite());
        assert!(viewer.can_export());

        let commenter = PermissionSet::commenter();
        assert!(!commenter.can_edit());
        assert!(!commenter.can_export());
    }

    #[test]
    fn test_vector_clock_increment() {
        let p = ParticipantId::new();
        let mut vc = VectorClock::default();
        vc.increment(&p);
        assert_eq!(vc.get(&p), 1);
        vc.increment(&p);
        assert_eq!(vc.get(&p), 2);
    }

    #[test]
    fn test_vector_clock_happens_before() {
        let p1 = ParticipantId::new();
        let p2 = ParticipantId::new();
        let mut va = VectorClock::default();
        let mut vb = VectorClock::default();
        va.increment(&p1);
        vb.increment(&p1);
        vb.increment(&p2);
        assert!(va.happens_before(&vb));
        assert!(!vb.happens_before(&va));
    }

    #[test]
    fn test_vector_clock_is_concurrent() {
        let p1 = ParticipantId::new();
        let p2 = ParticipantId::new();
        let mut va = VectorClock::default();
        let mut vb = VectorClock::default();
        va.increment(&p1);
        vb.increment(&p2);
        assert!(va.is_concurrent(&vb));
        assert!(vb.is_concurrent(&va));
    }

    #[test]
    fn test_vector_clock_merge() {
        let p1 = ParticipantId::new();
        let p2 = ParticipantId::new();
        let mut va = VectorClock::default();
        let mut vb = VectorClock::default();
        va.increment(&p1);
        va.increment(&p1);
        vb.increment(&p2);
        let merged = va.merge(&vb);
        assert_eq!(merged.get(&p1), 2);
        assert_eq!(merged.get(&p2), 1);
    }

    #[test]
    fn test_vector_clock_same_not_happens_before() {
        let p = ParticipantId::new();
        let mut vc = VectorClock::default();
        vc.increment(&p);
        assert!(!vc.happens_before(&vc));
    }

    #[test]
    fn test_crdt_document_new_and_from_str() {
        let doc = CrdtDocument::new();
        assert!(doc.text.is_empty());
        assert_eq!(doc.last_operation_id, 0);

        let doc2 = CrdtDocument::from_str("hello world");
        assert_eq!(doc2.text.to_string(), "hello world");
        assert!(doc2.cursors.is_empty());
    }

    #[test]
    fn test_crdt_document_next_op_id() {
        let mut doc = CrdtDocument::new();
        assert_eq!(doc.next_op_id(), 1);
        assert_eq!(doc.next_op_id(), 2);
        assert_eq!(doc.next_op_id(), 3);
    }

    #[test]
    fn test_operation_log_push_and_truncation() {
        let mut log = OperationLog::new(3);
        let p = ParticipantId::new();
        log.push(LoggedOperation {
            id: 1,
            operation: make_dummy_op(&p),
            vector_clock: VectorClock::default(),
            timestamp: Utc::now(),
        });
        log.push(LoggedOperation {
            id: 2,
            operation: make_dummy_op(&p),
            vector_clock: VectorClock::default(),
            timestamp: Utc::now(),
        });
        assert_eq!(log.len(), 2);

        log.push(LoggedOperation {
            id: 3,
            operation: make_dummy_op(&p),
            vector_clock: VectorClock::default(),
            timestamp: Utc::now(),
        });
        assert_eq!(log.len(), 3);

        log.push(LoggedOperation {
            id: 4,
            operation: make_dummy_op(&p),
            vector_clock: VectorClock::default(),
            timestamp: Utc::now(),
        });
        assert_eq!(log.len(), 3);
        assert_eq!(log.iter().next().unwrap().id, 2);
    }

    #[test]
    fn test_collab_config_default() {
        let cfg = CollabConfig::default();
        assert_eq!(cfg.max_participants_per_session, 50);
        assert_eq!(cfg.max_document_size_bytes, 10 * 1024 * 1024);
        assert_eq!(cfg.typing_indicator_timeout, Duration::from_secs(3));
        assert_eq!(cfg.max_history_operations, 1000);
    }

    #[test]
    fn test_collab_settings_default() {
        let settings = CollabSettings::default();
        assert!(settings.language.is_none());
        assert_eq!(settings.tab_size, 4);
        assert!(settings.insert_spaces);
        assert!(!settings.word_wrap);
        assert!(settings.show_line_numbers);
        assert_eq!(settings.theme, "dark");
    }

    #[test]
    fn test_rgba_color_from_str() {
        let c = RgbaColor::from_str("#FF6B6B").unwrap();
        assert_eq!(c.r, 0xFF);
        assert_eq!(c.g, 0x6B);
        assert_eq!(c.b, 0x6B);
        assert_eq!(c.a, 255);

        let c8 = RgbaColor::from_str("FF6B6B80").unwrap();
        assert_eq!(c8.a, 0x80);

        assert!(RgbaColor::from_str("invalid").is_none());
        assert!(RgbaColor::from_str("#abc").is_none());
    }

    #[test]
    fn test_rgba_color_roundtrip() {
        let original = RgbaColor { r: 255, g: 128, b: 0, a: 200 };
        let hex = original.to_hex_string();
        let restored = RgbaColor::from_str(&hex).unwrap();
        assert_eq!(original, restored);
    }

    #[test]
    fn test_presence_manager_online_tracking() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let pm = PresenceManager::new();
            let pid = ParticipantId::new();
            assert_eq!(pm.online_count().await, 0);
            pm.mark_online(&pid, None).await;
            assert!(pm.is_online(&pid).await);
            assert_eq!(pm.online_count().await, 1);
            pm.mark_offline(&pid).await;
            assert!(!pm.is_online(&pid).await);
            assert_eq!(pm.online_count().await, 0);
        });
    }

    #[test]
    fn test_presence_manager_typing_indicators() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let pm = PresenceManager::new();
            let pid = ParticipantId::new();
            let doc_id = Uuid::new_v4();
            pm.set_typing(&pid, doc_id).await;
            pm.clear_typing(&pid).await;
            pm.mark_offline(&pid).await;
        });
    }

    #[test]
    fn test_conflict_resolver_lww_strategy() {
        let resolver = ConflictResolver::new(MergeStrategy::LastWriteWins);
        let p1 = ParticipantId::new();
        let earlier = TextOperation {
            op_type: OpType::Insert,
            position: Position::zero(),
            text: "old".to_string(),
            participant_id: p1.clone(),
            timestamp: chrono::Utc::now() - chrono::Duration::seconds(10),
            version: VectorClock::default(),
        };
        let later = TextOperation {
            op_type: OpType::Insert,
            position: Position::zero(),
            text: "new".to_string(),
            participant_id: ParticipantId::new(),
            timestamp: chrono::Utc::now(),
            version: VectorClock::default(),
        };
        let remote = RemoteOperation { operation: later, received_at: Utc::now() };
        let result = resolver.resolve_concurrent_edits(&earlier, &[remote], &VectorClock::default());
        assert!(result.had_conflicts);
        assert_eq!(result.operations[0].text, "new");
    }

    #[test]
    fn test_conflict_resolver_fww_strategy() {
        let resolver = ConflictResolver::new(MergeStrategy::FirstWriteWins);
        let p1 = ParticipantId::new();
        let local = TextOperation {
            op_type: OpType::Insert,
            position: Position::zero(),
            text: "first".to_string(),
            participant_id: p1.clone(),
            timestamp: Utc::now(),
            version: VectorClock::default(),
        };
        let remote_op = TextOperation {
            op_type: OpType::Insert,
            position: Position::zero(),
            text: "second".to_string(),
            participant_id: ParticipantId::new(),
            timestamp: Utc::now() + chrono::Duration::seconds(10),
            version: VectorClock::default(),
        };
        let remote = RemoteOperation { operation: remote_op, received_at: Utc::now() };
        let result = resolver.resolve_concurrent_edits(&local, &[remote], &VectorClock::default());
        assert!(result.had_conflicts);
        assert_eq!(result.operations[0].text, "first");
    }

    #[test]
    fn test_document_store_save_and_load() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let store = DocumentStore::new();
            let doc_id = Uuid::new_v4();
            let owner = ParticipantId::new();
            let rope = Rope::from_str("hello");
            let version = VectorClock::default();
            store.store(doc_id, &rope, &owner, &version).await;
            let loaded = store.load(doc_id).await;
            assert!(loaded.is_some());
            assert_eq!(loaded.unwrap().to_string(), "hello");
            assert_eq!(store.version_count(doc_id).await, 1);
            assert_eq!(store.version_count(Uuid::new_v4()).await, 0);
        });
    }

    #[test]
    fn test_server_create_session() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let server = CollaborationServer::new(CollabConfig::default());
            let owner = Participant {
                id: ParticipantId::new(),
                user_id: UserId::Anonymous,
                display_name: "Owner".to_string(),
                avatar: None,
                role: ParticipantRole::Owner,
                permissions: PermissionSet::owner(),
                connection: (),
                joined_at: Utc::now(),
                last_activity: Utc::now(),
            };
            let session = server.create_session(&owner, "initial content").await.unwrap();
            assert_eq!(session.document.content.to_string(), "initial content");
            assert!(session.participants.contains(&owner.id));
        });
    }

    #[test]
    fn test_server_join_session() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let server = CollaborationServer::new(CollabConfig::default());
            let owner = make_test_participant(ParticipantRole::Owner, "Alice");
            let session = server.create_session(&owner, "").await.unwrap();
            let guest = make_test_participant(ParticipantRole::Editor, "Bob");
            let result = server.join_session(&session.id, &guest).await.unwrap();
            assert_eq!(result.existing_participants.len(), 1);
            assert_eq!(result.document_content, "");
        });
    }

    #[test]
    fn test_server_apply_edit() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let server = CollaborationServer::new(CollabConfig::default());
            let owner = make_test_participant(ParticipantRole::Owner, "Alice");
            let session = server.create_session(&owner, "hello").await.unwrap();
            let edit = TextEdit {
                position: Position::new(0, 5),
                new_text: " world".to_string(),
                old_range: None,
            };
            let result = server.apply_edit(&session.id, &owner.id, &edit).await.unwrap();
            assert!(result.success);
            let sessions = server.sessions.read().await;
            let updated = &sessions[&session.id];
            assert_eq!(updated.document.content.to_string(), "hello world");
        });
    }

    #[test]
    fn test_server_subscribe_receives_messages() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let server = CollaborationServer::new(CollabConfig::default());
            let owner = make_test_participant(ParticipantRole::Owner, "Alice");
            let session = server.create_session(&owner, "base").await.unwrap();
            let mut rx = server.subscribe();
            let edit = TextEdit {
                position: Position::zero(),
                new_text: "!".to_string(),
                old_range: None,
            };
            server.apply_edit(&session.id, &owner.id, &edit).await.ok();
            let msg = rx.recv().await;
            assert!(msg.is_ok());
            match msg.unwrap() {
                ServerPushMessage::TextInserted { text, .. } => assert_eq!(text, "!"),
                other => panic!("Unexpected message: {:?}", other),
            }
        });
    }

    fn make_dummy_op(participant: &ParticipantId) -> TextOperation {
        TextOperation {
            op_type: OpType::Insert,
            position: Position::zero(),
            text: String::new(),
            participant_id: participant.clone(),
            timestamp: Utc::now(),
            version: VectorClock::default(),
        }
    }

    fn make_test_participant(role: ParticipantRole, name: &str) -> Participant {
        Participant {
            id: ParticipantId::new(),
            user_id: UserId::Anonymous,
            display_name: name.to_string(),
            avatar: None,
            role,
            permissions: match role {
                ParticipantRole::Owner => PermissionSet::owner(),
                ParticipantRole::Editor => PermissionSet::editor(),
                ParticipantRole::Viewer => PermissionSet::viewer(),
                ParticipantRole::Commenter => PermissionSet::commenter(),
            },
            connection: (),
            joined_at: Utc::now(),
            last_activity: Utc::now(),
        }
    }
}
