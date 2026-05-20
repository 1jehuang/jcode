use crate::message::ToolCall;
use crate::side_panel::SidePanelSnapshot;
use crate::todo::TodoItem;
pub use jcode_background_types::{
    BackgroundTaskProgress, BackgroundTaskProgressEvent, BackgroundTaskProgressKind,
    BackgroundTaskProgressSource, BackgroundTaskStatus,
};
pub use jcode_batch_types::{BatchProgress, BatchSubcallProgress, BatchSubcallState};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ToolStatus {
    Running,
    Completed,
    Error,
}

impl ToolStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            ToolStatus::Running => "running",
            ToolStatus::Completed => "completed",
            ToolStatus::Error => "error",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolEvent {
    pub session_id: String,
    pub message_id: String,
    pub tool_call_id: String,
    pub tool_name: String,
    pub status: ToolStatus,
    pub title: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TodoEvent {
    pub session_id: String,
    pub todos: Vec<TodoItem>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolSummaryState {
    pub status: String,
    pub title: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolSummary {
    pub id: String,
    pub tool: String,
    pub state: ToolSummaryState,
}

/// Status update from a subagent (used by Task tool)
#[derive(Clone, Debug)]
pub struct SubagentStatus {
    pub session_id: String,
    pub status: String, // e.g., "calling API", "running grep", "streaming"
    pub model: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ManualToolCompleted {
    pub session_id: String,
    pub tool_call: ToolCall,
    pub output: String,
    pub is_error: bool,
    pub title: Option<String>,
    pub duration_ms: u64,
}

/// Type of file operation for swarm awareness
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum FileOp {
    Read,
    Write,
    Edit,
}

impl FileOp {
    pub fn as_str(&self) -> &'static str {
        match self {
            FileOp::Read => "read",
            FileOp::Write => "wrote",
            FileOp::Edit => "edited",
        }
    }

    pub fn is_modification(&self) -> bool {
        matches!(self, FileOp::Write | FileOp::Edit)
    }
}

/// File touch event for swarm coordination
#[derive(Clone, Debug)]
pub struct FileTouch {
    pub session_id: String,
    pub path: PathBuf,
    pub op: FileOp,
    /// Agent-provided intent for the tool call that touched this file.
    pub intent: Option<String>,
    /// Human-readable summary like "edited lines 45-60" or "read 200 lines"
    pub summary: Option<String>,
    /// Optional compact preview of what changed. Keep this short and already truncated.
    pub detail: Option<String>,
}

/// Event sent when a background task completes
#[derive(Debug, Clone)]
pub struct BackgroundTaskCompleted {
    pub task_id: String,
    pub tool_name: String,
    pub display_name: Option<String>,
    pub session_id: String,
    pub status: BackgroundTaskStatus,
    pub exit_code: Option<i32>,
    pub output_preview: String,
    pub output_file: PathBuf,
    pub duration_secs: f64,
    pub notify: bool,
    pub wake: bool,
}

#[derive(Clone, Debug)]
pub struct LoginCompleted {
    pub provider: String,
    pub success: bool,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct InputShellCompleted {
    pub session_id: String,
    pub result: crate::message::InputShellResult,
}

#[derive(Clone, Debug)]
pub enum ClipboardPasteKind {
    Smart,
    ImageOnly,
    ImageUrl { fallback_text: Option<String> },
}

#[derive(Clone, Debug)]
pub enum ClipboardPasteContent {
    Text(String),
    Image {
        media_type: String,
        base64_data: String,
    },
    Empty,
    Error(String),
}

#[derive(Clone, Debug)]
pub struct ClipboardPasteCompleted {
    pub session_id: String,
    pub kind: ClipboardPasteKind,
    pub content: ClipboardPasteContent,
}

#[derive(Clone, Debug)]
pub struct ModelRefreshCompleted {
    pub session_id: String,
    pub result: std::result::Result<crate::provider::ModelCatalogRefreshSummary, String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiActivityKind {
    Auth,
    Catalog,
    Background,
}

impl UiActivityKind {
    pub fn scope(&self) -> &'static str {
        match self {
            Self::Auth => "auth_activity",
            Self::Catalog => "catalog_activity",
            Self::Background => "background_activity",
        }
    }
}

#[derive(Clone, Debug)]
pub struct UiActivity {
    pub session_id: Option<String>,
    pub kind: UiActivityKind,
    pub message: String,
    pub status_notice: Option<String>,
}

impl UiActivity {
    pub fn new(
        session_id: Option<String>,
        kind: UiActivityKind,
        message: impl Into<String>,
        status_notice: Option<impl Into<String>>,
    ) -> Self {
        Self {
            session_id,
            kind,
            message: message.into(),
            status_notice: status_notice.map(Into::into),
        }
    }

    pub fn auth(
        session_id: Option<String>,
        message: impl Into<String>,
        status_notice: Option<impl Into<String>>,
    ) -> Self {
        Self::new(session_id, UiActivityKind::Auth, message, status_notice)
    }

    pub fn catalog(
        session_id: Option<String>,
        message: impl Into<String>,
        status_notice: Option<impl Into<String>>,
    ) -> Self {
        Self::new(session_id, UiActivityKind::Catalog, message, status_notice)
    }

    pub fn background(
        session_id: Option<String>,
        message: impl Into<String>,
        status_notice: Option<impl Into<String>>,
    ) -> Self {
        Self::new(
            session_id,
            UiActivityKind::Background,
            message,
            status_notice,
        )
    }

    pub fn is_visible_to_session(&self, session_id: &str) -> bool {
        match self.session_id.as_deref() {
            Some(target) => target == session_id,
            None => true,
        }
    }
}

#[derive(Clone, Debug)]
pub struct GitStatusCompleted {
    pub session_id: String,
    pub result: std::result::Result<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SidePanelUpdated {
    pub session_id: String,
    pub snapshot: SidePanelSnapshot,
}

#[derive(Clone, Debug)]
pub enum UpdateStatus {
    Checking,
    Available { current: String, latest: String },
    Downloading { version: String },
    Installed { version: String },
    UpToDate,
    Error(String),
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClientMaintenanceAction {
    Update,
    Rebuild,
}

impl ClientMaintenanceAction {
    pub fn noun(&self) -> &'static str {
        match self {
            Self::Update => "update",
            Self::Rebuild => "rebuild",
        }
    }

    pub fn title(&self) -> &'static str {
        match self {
            Self::Update => "Update",
            Self::Rebuild => "Rebuild",
        }
    }
}

#[derive(Clone, Debug)]
pub enum SessionUpdateStatus {
    Status {
        session_id: String,
        action: ClientMaintenanceAction,
        message: String,
    },
    NoUpdate {
        session_id: String,
        current: String,
    },
    ReadyToReload {
        session_id: String,
        action: ClientMaintenanceAction,
        version: String,
    },
    Error {
        session_id: String,
        action: ClientMaintenanceAction,
        message: String,
    },
}

#[derive(Clone, Debug)]
pub enum BusEvent {
    ToolUpdated(ToolEvent),
    TodoUpdated(TodoEvent),
    SubagentStatus(SubagentStatus),
    ManualToolCompleted(ManualToolCompleted),
    BatchProgress(BatchProgress),
    /// File was touched by an agent (for swarm conflict detection)
    FileTouch(FileTouch),
    /// Background task completed
    BackgroundTaskCompleted(BackgroundTaskCompleted),
    /// Background task reported progress
    BackgroundTaskProgress(BackgroundTaskProgressEvent),
    /// Usage report fetched from providers
    UsageReport(Vec<crate::usage::ProviderUsage>),
    /// Progressive usage report update while providers are still loading
    UsageReportProgress(crate::usage::ProviderUsageProgress),
    /// OAuth/login flow completed in the background
    LoginCompleted(LoginCompleted),
    /// Local `!cmd` shell command completed from the input line
    InputShellCompleted(InputShellCompleted),
    /// Clipboard paste/image URL work completed off the UI thread
    ClipboardPasteCompleted(ClipboardPasteCompleted),
    /// Local model catalog refresh completed off the UI thread
    ModelRefreshCompleted(ModelRefreshCompleted),
    /// UI-visible runtime activity from auth/catalog/background operations.
    UiActivity(UiActivity),
    /// Local git status command completed off the UI thread
    GitStatusCompleted(GitStatusCompleted),
    /// Update check status from background thread
    UpdateStatus(UpdateStatus),
    /// Interactive client update status for a specific session
    SessionUpdateStatus(SessionUpdateStatus),
    /// External dictation command completed with transcript text
    DictationCompleted {
        dictation_id: String,
        session_id: Option<String>,
        text: String,
        mode: crate::protocol::TranscriptMode,
    },
    /// External dictation command failed
    DictationFailed {
        dictation_id: String,
        session_id: Option<String>,
        message: String,
    },
    /// Background compaction task finished (check_and_apply should be called)
    CompactionFinished,
    /// Provider's available models list may have changed
    ModelsUpdated,
    /// A background provider setup task selected a model for this session.
    ProviderModelActivated {
        session_id: String,
        model: String,
        message: String,
        open_picker: bool,
    },
    /// Side panel pages were updated for a session
    SidePanelUpdated(SidePanelUpdated),
    /// Deferred Mermaid rendering completed and cached content may now be visible
    MermaidRenderCompleted,
}

pub struct Bus {
    sender: broadcast::Sender<BusEvent>,
}

const MODELS_UPDATED_DEBOUNCE: Duration = Duration::from_millis(750);

#[derive(Default)]
struct ModelsUpdatedPublishState {
    last_published_at: Option<Instant>,
    publish_pending: bool,
}

fn models_updated_publish_state() -> &'static Mutex<ModelsUpdatedPublishState> {
    static STATE: OnceLock<Mutex<ModelsUpdatedPublishState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(ModelsUpdatedPublishState::default()))
}

#[cfg(test)]
pub(crate) fn reset_models_updated_publish_state_for_tests() {
    let mut state = models_updated_publish_state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *state = ModelsUpdatedPublishState::default();
}

impl Bus {
    pub fn global() -> &'static Bus {
        static INSTANCE: OnceLock<Bus> = OnceLock::new();
        INSTANCE.get_or_init(|| {
            let (sender, _) = broadcast::channel(256);
            Bus { sender }
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<BusEvent> {
        self.sender.subscribe()
    }

    pub fn publish(&self, event: BusEvent) {
        let _ = self.sender.send(event);
    }

    pub fn publish_models_updated(&self) {
        let delay = {
            let now = Instant::now();
            let mut state = models_updated_publish_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match state.last_published_at {
                None => {
                    state.last_published_at = Some(now);
                    None
                }
                Some(last) => {
                    let elapsed = now.saturating_duration_since(last);
                    if elapsed >= MODELS_UPDATED_DEBOUNCE {
                        state.last_published_at = Some(now);
                        None
                    } else if state.publish_pending {
                        return;
                    } else {
                        state.publish_pending = true;
                        Some(MODELS_UPDATED_DEBOUNCE - elapsed)
                    }
                }
            }
        };

        if let Some(delay) = delay {
            let Ok(handle) = tokio::runtime::Handle::try_current() else {
                let mut state = models_updated_publish_state()
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state.publish_pending = false;
                state.last_published_at = Some(Instant::now());
                drop(state);
                self.publish(BusEvent::ModelsUpdated);
                return;
            };
            handle.spawn(async move {
                tokio::time::sleep(delay).await;
                let mut state = models_updated_publish_state()
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state.publish_pending = false;
                state.last_published_at = Some(Instant::now());
                drop(state);
                Bus::global().publish(BusEvent::ModelsUpdated);
            });
            return;
        }

        self.publish(BusEvent::ModelsUpdated);
    }
}

#[cfg(test)]
mod tests {
    use super::{Bus, BusEvent, reset_models_updated_publish_state_for_tests};
    use tokio::time::{Duration, timeout};

    #[tokio::test]
    async fn models_updated_publishes_are_coalesced() {
        let mut rx = Bus::global().subscribe();
        while rx.try_recv().is_ok() {}

        reset_models_updated_publish_state_for_tests();

        Bus::global().publish_models_updated();
        Bus::global().publish_models_updated();
        Bus::global().publish_models_updated();

        match timeout(Duration::from_secs(1), rx.recv()).await {
            Ok(Ok(BusEvent::ModelsUpdated)) => {}
            other => panic!("expected immediate ModelsUpdated event, got {other:?}"),
        }

        match timeout(Duration::from_secs(2), rx.recv()).await {
            Ok(Ok(BusEvent::ModelsUpdated)) => {}
            other => panic!("expected coalesced delayed ModelsUpdated event, got {other:?}"),
        }

        assert!(
            timeout(Duration::from_millis(300), rx.recv())
                .await
                .is_err()
        );
    }
}
