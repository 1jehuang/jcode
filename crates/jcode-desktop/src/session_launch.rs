use anyhow::{Context, Result};
use serde_json::json;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

const SERVER_START_TIMEOUT: Duration = Duration::from_secs(10);
const SERVER_CONNECT_RETRY_DELAY: Duration = Duration::from_millis(50);
const DESKTOP_SESSION_WORKER_LIMIT: usize = 12;
static DESKTOP_SESSION_WORKER_COUNT: AtomicUsize = AtomicUsize::new(0);
struct DesktopSessionWorkerPermit<'a> {
    counter: &'a AtomicUsize,
}

impl Drop for DesktopSessionWorkerPermit<'_> {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::Relaxed);
    }
}

fn try_acquire_desktop_session_worker_slot<'a>(
    counter: &'a AtomicUsize,
    limit: usize,
) -> Result<DesktopSessionWorkerPermit<'a>> {
    let mut current = counter.load(Ordering::Relaxed);
    loop {
        if current >= limit {
            anyhow::bail!("desktop session worker limit reached ({limit})");
        }
        match counter.compare_exchange_weak(
            current,
            current + 1,
            Ordering::Acquire,
            Ordering::Relaxed,
        ) {
            Ok(_) => return Ok(DesktopSessionWorkerPermit { counter }),
            Err(next_current) => current = next_current,
        }
    }
}

fn spawn_bounded_desktop_session_worker(
    name: impl Into<String>,
    job: impl FnOnce() + Send + 'static,
) -> Result<()> {
    let name = name.into();
    let permit = try_acquire_desktop_session_worker_slot(
        &DESKTOP_SESSION_WORKER_COUNT,
        DESKTOP_SESSION_WORKER_LIMIT,
    )
    .with_context(|| format!("failed to start {name}"))?;
    std::thread::Builder::new()
        .name(name.clone())
        .spawn(move || {
            let _permit = permit;
            job();
        })
        .with_context(|| format!("failed to spawn {name}"))?;
    Ok(())
}

mod events;
#[cfg(unix)]
mod server_io;
mod terminal;

#[cfg(unix)]
use server_io::{
    DrainOutcome, connect_server_with_retry, connect_server_with_retry_path, drain_session_events,
    ensure_server_running, establish_session_id, read_control_response, read_model_catalog,
    read_model_changed, subscribe_and_establish_session, subscribe_to_server,
    validate_reload_socket_path, write_json_line,
};
use terminal::{compact_title, launch_first_available_terminal, terminal_candidates};
pub use terminal::{launch_validated_resume_session, validate_resume_session_id};

pub(super) fn default_desktop_working_dir() -> Option<PathBuf> {
    if let Ok(raw) = std::env::var("JCODE_DESKTOP_WORKING_DIR") {
        let path = PathBuf::from(raw);
        if is_usable_directory(&path) {
            return Some(path);
        }
        crate::desktop_log::warn(format_args!(
            "jcode-desktop: ignoring JCODE_DESKTOP_WORKING_DIR because it is not a directory: {}",
            path.display()
        ));
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if is_usable_directory(&manifest_dir) {
        return Some(manifest_dir);
    }

    None
}

fn is_usable_directory(path: &Path) -> bool {
    path.is_dir()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DesktopModelChoice {
    pub model: String,
    pub provider: Option<String>,
    pub api_method: Option<String>,
    pub detail: Option<String>,
    pub available: bool,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum DesktopSessionStatus {
    StartingSharedServer,
    ConnectingSharedServer,
    SendingMessage,
    SwitchingModel,
    LoadingModels,
    ServerDisconnectedReconnecting,
    ServerReloadingReconnecting,
    Cancelling,
    Cancelled,
    SendingInteractiveInput,
    Interrupted,
    ReasoningEffort(String),
    ReasoningEffortFailed(String),
    ServiceTier(String),
    ServiceTierFailed(String),
    Transport(String),
    TransportFailed(String),
    CompactionMode(String),
    CompactionModeFailed(String),
    CompactResult { message: String, success: bool },
    External { label: String, in_flight: bool },
}

impl DesktopSessionStatus {
    pub fn external(label: impl Into<String>) -> Self {
        let label = label.into();
        Self::External {
            in_flight: desktop_status_label_is_in_flight(&label),
            label,
        }
    }

    pub fn label(&self) -> String {
        match self {
            Self::StartingSharedServer => "starting shared server".to_string(),
            Self::ConnectingSharedServer => "connecting to shared server".to_string(),
            Self::SendingMessage => "sending message".to_string(),
            Self::SwitchingModel => "switching model".to_string(),
            Self::LoadingModels => "loading models".to_string(),
            Self::ServerDisconnectedReconnecting => "server disconnected, reconnecting".to_string(),
            Self::ServerReloadingReconnecting => "server reloading, reconnecting".to_string(),
            Self::Cancelling => "cancelling".to_string(),
            Self::Cancelled => "cancelled".to_string(),
            Self::SendingInteractiveInput => "sending interactive input".to_string(),
            Self::Interrupted => "interrupted".to_string(),
            Self::ReasoningEffort(effort) => format!("thinking level: {effort}"),
            Self::ReasoningEffortFailed(error) => format!("effort switch failed: {error}"),
            Self::ServiceTier(tier) => format!("fast mode: {tier}"),
            Self::ServiceTierFailed(error) => format!("fast mode failed: {error}"),
            Self::Transport(transport) => format!("transport: {transport}"),
            Self::TransportFailed(error) => format!("transport failed: {error}"),
            Self::CompactionMode(mode) => format!("compaction: {mode}"),
            Self::CompactionModeFailed(error) => format!("compaction mode failed: {error}"),
            Self::CompactResult { message, success } => {
                if *success {
                    "compacting context".to_string()
                } else {
                    format!("compaction failed: {message}")
                }
            }
            Self::External { label, .. } => label.clone(),
        }
    }

    pub fn is_in_flight(&self) -> bool {
        match self {
            Self::StartingSharedServer
            | Self::ConnectingSharedServer
            | Self::SendingMessage
            | Self::SwitchingModel
            | Self::LoadingModels
            | Self::ServerDisconnectedReconnecting
            | Self::ServerReloadingReconnecting
            | Self::Cancelling
            | Self::SendingInteractiveInput => true,
            Self::External { in_flight, .. } => *in_flight,
            Self::Cancelled
            | Self::Interrupted
            | Self::ReasoningEffort(_)
            | Self::ReasoningEffortFailed(_)
            | Self::ServiceTier(_)
            | Self::ServiceTierFailed(_)
            | Self::Transport(_)
            | Self::TransportFailed(_)
            | Self::CompactionMode(_)
            | Self::CompactionModeFailed(_)
            | Self::CompactResult { .. } => false,
        }
    }

    pub fn payload_bytes(&self) -> usize {
        self.label().len()
    }
}

fn desktop_status_label_is_in_flight(status: &str) -> bool {
    matches!(
        status,
        "loading models"
            | "loading recent sessions"
            | "receiving"
            | "connected"
            | "sending"
            | "sending message"
            | "sending interactive input"
            | "switching model"
            | "switching reasoning effort"
            | "refreshing model list"
            | "setting fast mode"
            | "setting transport"
            | "setting compaction mode"
            | "requesting compaction"
            | "renaming session"
            | "clearing session"
            | "cancelling"
            | "starting shared server"
            | "connecting to shared server"
            | "server disconnected, reconnecting"
            | "server reloading, reconnecting"
    ) || status.starts_with("using tool ")
        || status.starts_with("preparing tool ")
        || status.starts_with("attached ")
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DesktopSessionEvent {
    Status(DesktopSessionStatus),
    SessionStarted {
        session_id: String,
    },
    SessionRenamed {
        title: Option<String>,
        display_title: String,
    },
    TextDelta(String),
    TextReplace(String),
    ToolStarted {
        id: Option<String>,
        name: String,
    },
    ToolExecuting {
        id: Option<String>,
        name: String,
    },
    ToolInput {
        id: Option<String>,
        delta: String,
    },
    ToolFinished {
        id: Option<String>,
        name: String,
        summary: String,
        is_error: bool,
    },
    ModelChanged {
        model: String,
        provider_name: Option<String>,
        error: Option<String>,
    },
    ModelCatalog {
        current_model: Option<String>,
        provider_name: Option<String>,
        models: Vec<DesktopModelChoice>,
        reasoning_effort: Option<String>,
        service_tier: Option<String>,
        compaction_mode: Option<String>,
    },
    ModelCatalogError {
        error: String,
    },
    StdinRequest {
        request_id: String,
        prompt: String,
        is_password: bool,
        tool_call_id: String,
    },
    ReloadProgress {
        step: String,
        message: String,
        success: Option<bool>,
        output: Option<String>,
    },
    RuntimeMetadata {
        connection_type: Option<String>,
        status_detail: Option<String>,
        upstream_provider: Option<String>,
    },
    TokenUsage {
        input: u64,
        output: u64,
        cache_read_input: Option<u64>,
        cache_creation_input: Option<u64>,
    },
    SystemNotice {
        title: String,
        message: Option<String>,
    },
    SessionCloseRequested {
        reason: String,
    },
    Reloading {
        new_socket: Option<String>,
    },
    Reloaded {
        session_id: String,
    },
    Done,
    Error(String),
}

pub type DesktopSessionEventSender = Sender<DesktopSessionEvent>;

#[derive(Clone, Debug)]
pub struct DesktopSessionHandle {
    command_tx: Sender<DesktopSessionCommand>,
}

impl DesktopSessionHandle {
    pub fn cancel(&self) -> Result<()> {
        self.command_tx
            .send(DesktopSessionCommand::Cancel)
            .context("failed to send cancel to desktop session worker")
    }

    pub fn send_stdin_response(&self, request_id: String, input: String) -> Result<()> {
        self.command_tx
            .send(DesktopSessionCommand::StdinResponse { request_id, input })
            .context("failed to send stdin response to desktop session worker")
    }

    pub fn set_reasoning_effort(&self, effort: String) -> Result<()> {
        self.command_tx
            .send(DesktopSessionCommand::SetReasoningEffort { effort })
            .context("failed to send reasoning effort change to desktop session worker")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum DesktopSessionCommand {
    Cancel,
    StdinResponse { request_id: String, input: String },
    SetReasoningEffort { effort: String },
}

pub fn launch_resume_session(session_id: &str, title: &str) -> Result<()> {
    let title = format!("jcode · {}", compact_title(title));
    let candidates = terminal_candidates(&title, &["--resume", session_id]);
    launch_first_available_terminal(candidates, &format!("jcode --resume {session_id}"))
}

pub fn launch_new_session() -> Result<()> {
    let candidates = terminal_candidates("jcode · new session", &["--fresh-spawn"]);
    launch_first_available_terminal(candidates, "jcode")
}

pub fn launch_selfdev_session() -> Result<()> {
    let candidates = terminal_candidates("jcode · self-dev", &["self-dev"]);
    launch_first_available_terminal(candidates, "jcode self-dev")
}

pub fn launch_home_session() -> Result<()> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
    let candidates =
        terminal::terminal_candidates_in_dir("jcode · home", &["--fresh-spawn"], home.as_path());
    launch_first_available_terminal(candidates, "jcode in home directory")
}

pub fn send_message_to_session(session_id: &str, _title: &str, message: &str) -> Result<()> {
    validate_resume_session_id(session_id).context("refusing to send to invalid session id")?;
    if message.trim().is_empty() {
        anyhow::bail!("empty draft message");
    }

    let session_id = session_id.to_string();
    let message = message.to_string();
    spawn_bounded_desktop_session_worker("jcode-desktop-workspace-message", move || {
        let (_command_tx, command_rx) = mpsc::channel();
        if let Err(error) =
            run_server_session(Some(&session_id), &message, Vec::new(), None, command_rx)
        {
            crate::desktop_log::error(format_args!(
                "jcode-desktop: workspace server message failed session_id={session_id}: {error:#}"
            ));
        }
    })
    .context("failed to spawn desktop workspace message worker")?;

    Ok(())
}

pub fn spawn_fresh_server_session(
    message: String,
    images: Vec<(String, String)>,
    event_tx: DesktopSessionEventSender,
) -> Result<DesktopSessionHandle> {
    if message.trim().is_empty() && images.is_empty() {
        anyhow::bail!("empty draft message");
    }

    let (command_tx, command_rx) = mpsc::channel();
    let handle = DesktopSessionHandle { command_tx };
    spawn_bounded_desktop_session_worker("jcode-desktop-fresh-session", move || {
        if let Err(error) =
            run_server_session(None, &message, images, Some(event_tx.clone()), command_rx)
        {
            crate::desktop_log::error(format_args!(
                "jcode-desktop: fresh server session failed: {error:#}"
            ));
            send_desktop_event_ref(
                Some(&event_tx),
                DesktopSessionEvent::Error(format!("{error:#}")),
            );
        }
    })
    .context("failed to spawn desktop session worker")?;
    Ok(handle)
}

pub fn spawn_message_to_session(
    session_id: String,
    message: String,
    images: Vec<(String, String)>,
    event_tx: DesktopSessionEventSender,
) -> Result<DesktopSessionHandle> {
    validate_resume_session_id(&session_id).context("refusing to send to invalid session id")?;
    if message.trim().is_empty() && images.is_empty() {
        anyhow::bail!("empty draft message");
    }

    let (command_tx, command_rx) = mpsc::channel();
    let handle = DesktopSessionHandle { command_tx };
    spawn_bounded_desktop_session_worker("jcode-desktop-session-message", move || {
        if let Err(error) = run_server_session(
            Some(&session_id),
            &message,
            images,
            Some(event_tx.clone()),
            command_rx,
        ) {
            crate::desktop_log::error(format_args!(
                "jcode-desktop: server session message failed session_id={session_id}: {error:#}"
            ));
            send_desktop_event_ref(
                Some(&event_tx),
                DesktopSessionEvent::Error(format!("{error:#}")),
            );
        }
    })
    .context("failed to spawn desktop session worker")?;
    Ok(handle)
}

#[cfg(unix)]
pub fn spawn_cycle_model(
    direction: i8,
    target_session_id: Option<String>,
    event_tx: DesktopSessionEventSender,
) -> Result<()> {
    spawn_bounded_desktop_session_worker("jcode-desktop-cycle-model", move || {
            if let Err(error) = cycle_model(
                direction,
                target_session_id.as_deref(),
                Some(event_tx.clone()),
            ) {
                crate::desktop_log::error(format_args!(
                    "jcode-desktop: model cycle failed direction={direction} target_session={}: {error:#}",
                    target_session_id.as_deref().unwrap_or("<current>")
                ));
                send_desktop_event_ref(
                    Some(&event_tx),
                    DesktopSessionEvent::ModelCatalogError {
                        error: format!("{error:#}"),
                    },
                );
            }
        })
        .context("failed to spawn desktop model switch worker")?;
    Ok(())
}

#[cfg(not(unix))]
pub fn spawn_cycle_model(
    _direction: i8,
    _target_session_id: Option<String>,
    event_tx: DesktopSessionEventSender,
) -> Result<()> {
    send_desktop_event_ref(
        Some(&event_tx),
        DesktopSessionEvent::ModelCatalogError {
            error: "desktop model switching is not implemented on this platform yet".to_string(),
        },
    );
    Ok(())
}

#[cfg(unix)]
pub fn spawn_load_model_catalog(
    target_session_id: Option<String>,
    event_tx: DesktopSessionEventSender,
) -> Result<()> {
    spawn_bounded_desktop_session_worker("jcode-desktop-load-model-catalog", move || {
        if let Err(error) = load_model_catalog(target_session_id.as_deref(), Some(event_tx.clone()))
        {
            crate::desktop_log::error(format_args!(
                "jcode-desktop: model catalog load failed target_session={}: {error:#}",
                target_session_id.as_deref().unwrap_or("<current>")
            ));
            send_desktop_event_ref(
                Some(&event_tx),
                DesktopSessionEvent::ModelCatalogError {
                    error: format!("{error:#}"),
                },
            );
        }
    })
    .context("failed to spawn desktop model catalog worker")?;
    Ok(())
}

#[cfg(not(unix))]
pub fn spawn_load_model_catalog(
    _target_session_id: Option<String>,
    event_tx: DesktopSessionEventSender,
) -> Result<()> {
    send_desktop_event_ref(
        Some(&event_tx),
        DesktopSessionEvent::ModelCatalogError {
            error: "desktop model catalog loading is not implemented on this platform yet"
                .to_string(),
        },
    );
    Ok(())
}

#[cfg(unix)]
pub fn spawn_set_model(
    model: String,
    target_session_id: Option<String>,
    event_tx: DesktopSessionEventSender,
) -> Result<()> {
    spawn_bounded_desktop_session_worker("jcode-desktop-set-model", move || {
        if let Err(error) = set_model(&model, target_session_id.as_deref(), Some(event_tx.clone()))
        {
            crate::desktop_log::error(format_args!(
                "jcode-desktop: set model failed model={} target_session={}: {error:#}",
                crate::desktop_log::truncate_for_log(&model, 256),
                target_session_id.as_deref().unwrap_or("<current>")
            ));
            send_desktop_event_ref(
                Some(&event_tx),
                DesktopSessionEvent::ModelCatalogError {
                    error: format!("{error:#}"),
                },
            );
        }
    })
    .context("failed to spawn desktop set model worker")?;
    Ok(())
}

#[cfg(not(unix))]
pub fn spawn_set_model(
    _model: String,
    _target_session_id: Option<String>,
    event_tx: DesktopSessionEventSender,
) -> Result<()> {
    send_desktop_event_ref(
        Some(&event_tx),
        DesktopSessionEvent::ModelCatalogError {
            error: "desktop model switching is not implemented on this platform yet".to_string(),
        },
    );
    Ok(())
}

#[cfg(unix)]
pub fn spawn_refresh_models(
    target_session_id: Option<String>,
    event_tx: DesktopSessionEventSender,
) -> Result<()> {
    spawn_control_request(
        "jcode-desktop-refresh-models",
        target_session_id,
        event_tx,
        DesktopSessionStatus::external("refreshing model list"),
        |id| json!({ "type": "refresh_models", "id": id }),
        &["available_models_updated"],
        "refreshing model list",
    )
}

#[cfg(not(unix))]
pub fn spawn_refresh_models(
    _target_session_id: Option<String>,
    event_tx: DesktopSessionEventSender,
) -> Result<()> {
    send_desktop_event_ref(
        Some(&event_tx),
        DesktopSessionEvent::ModelCatalogError {
            error: "desktop model refresh is not implemented on this platform yet".to_string(),
        },
    );
    Ok(())
}

#[cfg(unix)]
pub fn spawn_set_service_tier(
    service_tier: String,
    target_session_id: Option<String>,
    event_tx: DesktopSessionEventSender,
) -> Result<()> {
    spawn_control_request(
        "jcode-desktop-set-fast-mode",
        target_session_id,
        event_tx,
        DesktopSessionStatus::external("setting fast mode"),
        move |id| json!({ "type": "set_service_tier", "id": id, "service_tier": service_tier }),
        &["service_tier_changed"],
        "setting fast mode",
    )
}

#[cfg(not(unix))]
pub fn spawn_set_service_tier(
    _service_tier: String,
    _target_session_id: Option<String>,
    event_tx: DesktopSessionEventSender,
) -> Result<()> {
    send_desktop_event_ref(
        Some(&event_tx),
        DesktopSessionEvent::Status(DesktopSessionStatus::ServiceTierFailed(
            "desktop fast mode switching is not implemented on this platform yet".to_string(),
        )),
    );
    Ok(())
}

#[cfg(unix)]
pub fn spawn_set_transport(
    transport: String,
    target_session_id: Option<String>,
    event_tx: DesktopSessionEventSender,
) -> Result<()> {
    spawn_control_request(
        "jcode-desktop-set-transport",
        target_session_id,
        event_tx,
        DesktopSessionStatus::external("setting transport"),
        move |id| json!({ "type": "set_transport", "id": id, "transport": transport }),
        &["transport_changed"],
        "setting transport",
    )
}

#[cfg(not(unix))]
pub fn spawn_set_transport(
    _transport: String,
    _target_session_id: Option<String>,
    event_tx: DesktopSessionEventSender,
) -> Result<()> {
    send_desktop_event_ref(
        Some(&event_tx),
        DesktopSessionEvent::Status(DesktopSessionStatus::TransportFailed(
            "desktop transport switching is not implemented on this platform yet".to_string(),
        )),
    );
    Ok(())
}

#[cfg(unix)]
pub fn spawn_set_compaction_mode(
    mode: String,
    target_session_id: Option<String>,
    event_tx: DesktopSessionEventSender,
) -> Result<()> {
    spawn_control_request(
        "jcode-desktop-set-compaction-mode",
        target_session_id,
        event_tx,
        DesktopSessionStatus::external("setting compaction mode"),
        move |id| json!({ "type": "set_compaction_mode", "id": id, "mode": mode }),
        &["compaction_mode_changed"],
        "setting compaction mode",
    )
}

#[cfg(not(unix))]
pub fn spawn_set_compaction_mode(
    _mode: String,
    _target_session_id: Option<String>,
    event_tx: DesktopSessionEventSender,
) -> Result<()> {
    send_desktop_event_ref(
        Some(&event_tx),
        DesktopSessionEvent::Status(DesktopSessionStatus::CompactionModeFailed(
            "desktop compaction mode switching is not implemented on this platform yet".to_string(),
        )),
    );
    Ok(())
}

#[cfg(unix)]
pub fn spawn_compact_session(
    target_session_id: Option<String>,
    event_tx: DesktopSessionEventSender,
) -> Result<()> {
    spawn_control_request(
        "jcode-desktop-compact-session",
        target_session_id,
        event_tx,
        DesktopSessionStatus::external("requesting compaction"),
        |id| json!({ "type": "compact", "id": id }),
        &["compact_result"],
        "requesting compaction",
    )
}

#[cfg(not(unix))]
pub fn spawn_compact_session(
    _target_session_id: Option<String>,
    event_tx: DesktopSessionEventSender,
) -> Result<()> {
    send_desktop_event_ref(
        Some(&event_tx),
        DesktopSessionEvent::Status(DesktopSessionStatus::CompactResult {
            message: "desktop compaction is not implemented on this platform yet".to_string(),
            success: false,
        }),
    );
    Ok(())
}

#[cfg(unix)]
pub fn spawn_rename_session(
    title: Option<String>,
    target_session_id: Option<String>,
    event_tx: DesktopSessionEventSender,
) -> Result<()> {
    spawn_control_request(
        "jcode-desktop-rename-session",
        target_session_id,
        event_tx,
        DesktopSessionStatus::external("renaming session"),
        move |id| json!({ "type": "rename_session", "id": id, "title": title }),
        &["session_renamed"],
        "renaming session",
    )
}

#[cfg(not(unix))]
pub fn spawn_rename_session(
    _title: Option<String>,
    _target_session_id: Option<String>,
    event_tx: DesktopSessionEventSender,
) -> Result<()> {
    send_desktop_event_ref(
        Some(&event_tx),
        DesktopSessionEvent::Status(DesktopSessionStatus::external(
            "desktop session renaming is not implemented on this platform yet",
        )),
    );
    Ok(())
}

#[cfg(unix)]
pub fn spawn_clear_server_session(
    target_session_id: Option<String>,
    event_tx: DesktopSessionEventSender,
) -> Result<()> {
    spawn_control_request(
        "jcode-desktop-clear-session",
        target_session_id,
        event_tx,
        DesktopSessionStatus::external("clearing session"),
        |id| json!({ "type": "clear", "id": id }),
        &["done"],
        "clearing session",
    )
}

#[cfg(not(unix))]
pub fn spawn_clear_server_session(
    _target_session_id: Option<String>,
    event_tx: DesktopSessionEventSender,
) -> Result<()> {
    send_desktop_event_ref(
        Some(&event_tx),
        DesktopSessionEvent::Error(
            "desktop session clearing is not implemented on this platform yet".to_string(),
        ),
    );
    Ok(())
}

#[cfg(unix)]
fn spawn_control_request<F>(
    worker_name: &'static str,
    target_session_id: Option<String>,
    event_tx: DesktopSessionEventSender,
    status: DesktopSessionStatus,
    build_request: F,
    expected_event_types: &'static [&'static str],
    action_label: &'static str,
) -> Result<()>
where
    F: FnOnce(u64) -> serde_json::Value + Send + 'static,
{
    spawn_bounded_desktop_session_worker(worker_name, move || {
        if let Err(error) = run_control_request(
            target_session_id.as_deref(),
            Some(event_tx.clone()),
            status,
            build_request,
            expected_event_types,
            action_label,
        ) {
            crate::desktop_log::error(format_args!(
                "jcode-desktop: {action_label} failed target_session={}: {error:#}",
                target_session_id.as_deref().unwrap_or("<current>")
            ));
            send_desktop_event_ref(
                Some(&event_tx),
                DesktopSessionEvent::Status(DesktopSessionStatus::external(format!(
                    "{action_label} failed: {error:#}"
                ))),
            );
        }
    })
    .with_context(|| format!("failed to spawn desktop worker for {action_label}"))?;
    Ok(())
}

#[cfg(unix)]
fn run_control_request<F>(
    target_session_id: Option<&str>,
    event_tx: Option<DesktopSessionEventSender>,
    status: DesktopSessionStatus,
    build_request: F,
    expected_event_types: &[&str],
    action_label: &str,
) -> Result<()>
where
    F: FnOnce(u64) -> serde_json::Value,
{
    send_desktop_status(&event_tx, status);
    ensure_server_running()?;
    let stream = connect_server_with_retry(SERVER_START_TIMEOUT)?;
    let mut writer = stream
        .try_clone()
        .context("failed to clone server socket writer")?;
    let mut reader = BufReader::new(stream);
    let mut next_request_id = 1_u64;
    subscribe_and_establish_session(
        &mut reader,
        &mut writer,
        &mut next_request_id,
        target_session_id,
        event_tx.as_ref(),
    )?;
    let request_id = next_request_id;
    write_json_line(&mut writer, build_request(request_id))?;
    read_control_response(
        &mut reader,
        SERVER_START_TIMEOUT,
        event_tx.as_ref(),
        request_id,
        expected_event_types,
        action_label,
    )
}

#[cfg(unix)]
pub fn set_reasoning_effort(
    effort: &str,
    target_session_id: Option<&str>,
    event_tx: Option<DesktopSessionEventSender>,
) -> Result<()> {
    ensure_server_running()?;
    let stream = connect_server_with_retry(SERVER_START_TIMEOUT)?;
    let mut writer = stream
        .try_clone()
        .context("failed to clone server socket writer")?;
    let mut reader = BufReader::new(stream);
    let request_id = 1_u64;
    write_json_line(
        &mut writer,
        json!({
            "type": "set_reasoning_effort",
            "id": request_id,
            "effort": effort,
            "target_session_id": target_session_id,
        }),
    )?;
    read_control_response(
        &mut reader,
        SERVER_START_TIMEOUT,
        event_tx.as_ref(),
        request_id,
        &["reasoning_effort_changed"],
        "setting reasoning effort",
    )
}

#[cfg(not(unix))]
pub fn set_reasoning_effort(
    _effort: &str,
    _target_session_id: Option<&str>,
    event_tx: Option<DesktopSessionEventSender>,
) -> Result<()> {
    send_desktop_event_ref(
        event_tx.as_ref(),
        DesktopSessionEvent::Status(DesktopSessionStatus::ReasoningEffortFailed(
            "desktop reasoning effort switching is not implemented on this platform yet"
                .to_string(),
        )),
    );
    Ok(())
}

#[cfg(unix)]
fn cycle_model(
    direction: i8,
    target_session_id: Option<&str>,
    event_tx: Option<DesktopSessionEventSender>,
) -> Result<()> {
    send_desktop_status(&event_tx, DesktopSessionStatus::SwitchingModel);
    ensure_server_running()?;
    let stream = connect_server_with_retry(SERVER_START_TIMEOUT)?;
    let mut writer = stream
        .try_clone()
        .context("failed to clone server socket writer")?;
    let mut reader = BufReader::new(stream);
    let mut next_request_id = 1_u64;
    subscribe_and_establish_session(
        &mut reader,
        &mut writer,
        &mut next_request_id,
        target_session_id,
        event_tx.as_ref(),
    )?;
    let request_id = next_request_id;
    write_json_line(
        &mut writer,
        json!({
            "type": "cycle_model",
            "id": request_id,
            "direction": direction,
        }),
    )?;
    read_model_changed(
        &mut reader,
        SERVER_START_TIMEOUT,
        event_tx.as_ref(),
        request_id,
    )
}

#[cfg(unix)]
fn load_model_catalog(
    target_session_id: Option<&str>,
    event_tx: Option<DesktopSessionEventSender>,
) -> Result<()> {
    send_desktop_status(&event_tx, DesktopSessionStatus::LoadingModels);
    ensure_server_running()?;
    let stream = connect_server_with_retry(SERVER_START_TIMEOUT)?;
    let mut writer = stream
        .try_clone()
        .context("failed to clone server socket writer")?;
    let mut reader = BufReader::new(stream);
    let mut next_request_id = 1_u64;
    subscribe_and_establish_session(
        &mut reader,
        &mut writer,
        &mut next_request_id,
        target_session_id,
        event_tx.as_ref(),
    )?;
    let request_id = next_request_id;
    write_json_line(
        &mut writer,
        json!({
            "type": "get_model_catalog",
            "id": request_id,
        }),
    )?;
    read_model_catalog(
        &mut reader,
        SERVER_START_TIMEOUT,
        event_tx.as_ref(),
        request_id,
    )
}

#[cfg(unix)]
fn set_model(
    model: &str,
    target_session_id: Option<&str>,
    event_tx: Option<DesktopSessionEventSender>,
) -> Result<()> {
    send_desktop_status(&event_tx, DesktopSessionStatus::SwitchingModel);
    ensure_server_running()?;
    let stream = connect_server_with_retry(SERVER_START_TIMEOUT)?;
    let mut writer = stream
        .try_clone()
        .context("failed to clone server socket writer")?;
    let mut reader = BufReader::new(stream);
    let mut next_request_id = 1_u64;
    subscribe_and_establish_session(
        &mut reader,
        &mut writer,
        &mut next_request_id,
        target_session_id,
        event_tx.as_ref(),
    )?;
    let request_id = next_request_id;
    write_json_line(
        &mut writer,
        json!({
            "type": "set_model",
            "id": request_id,
            "model": model,
        }),
    )?;
    read_model_changed(
        &mut reader,
        SERVER_START_TIMEOUT,
        event_tx.as_ref(),
        request_id,
    )
}

#[cfg(unix)]
fn run_server_session(
    target_session_id: Option<&str>,
    message: &str,
    images: Vec<(String, String)>,
    event_tx: Option<DesktopSessionEventSender>,
    command_rx: Receiver<DesktopSessionCommand>,
) -> Result<String> {
    send_desktop_status(&event_tx, DesktopSessionStatus::StartingSharedServer);
    ensure_server_running()?;
    send_desktop_status(&event_tx, DesktopSessionStatus::ConnectingSharedServer);
    let stream = connect_server_with_retry(SERVER_START_TIMEOUT)?;
    let mut writer = stream
        .try_clone()
        .context("failed to clone server socket writer")?;
    let mut reader = BufReader::new(stream);
    let mut next_request_id = 1_u64;

    let subscribe_request_id = next_request_id;
    subscribe_to_server(&mut writer, subscribe_request_id, target_session_id)?;
    next_request_id += 1;

    let session_id = establish_session_id(
        &mut reader,
        &mut writer,
        &mut next_request_id,
        subscribe_request_id,
        event_tx.as_ref(),
    )?;
    send_desktop_event(
        &event_tx,
        DesktopSessionEvent::SessionStarted {
            session_id: session_id.clone(),
        },
    );

    send_desktop_status(&event_tx, DesktopSessionStatus::SendingMessage);
    let message_request_id = next_request_id;
    write_json_line(
        &mut writer,
        json!({
            "type": "message",
            "id": message_request_id,
            "content": message,
            "images": images,
        }),
    )?;
    next_request_id += 1;

    let mut current_socket_path = socket_path();
    loop {
        match drain_session_events(
            reader,
            &mut writer,
            &mut next_request_id,
            event_tx.as_ref(),
            &command_rx,
            message_request_id,
        )? {
            DrainOutcome::Terminal => break,
            DrainOutcome::Disconnected => {
                send_desktop_status(
                    &event_tx,
                    DesktopSessionStatus::ServerDisconnectedReconnecting,
                );
            }
            DrainOutcome::Reloading { new_socket } => {
                if let Some(path) = new_socket {
                    current_socket_path = validate_reload_socket_path(&current_socket_path, &path)?;
                }
                send_desktop_status(&event_tx, DesktopSessionStatus::ServerReloadingReconnecting);
            }
        }

        let stream = connect_server_with_retry_path(&current_socket_path, SERVER_START_TIMEOUT)?;
        writer = stream
            .try_clone()
            .context("failed to clone reconnected server socket writer")?;
        reader = BufReader::new(stream);
        let subscribe_request_id = next_request_id;
        subscribe_to_server(&mut writer, subscribe_request_id, Some(&session_id))?;
        next_request_id += 1;
        let reconnected_session_id = establish_session_id(
            &mut reader,
            &mut writer,
            &mut next_request_id,
            subscribe_request_id,
            event_tx.as_ref(),
        )?;
        if reconnected_session_id != session_id {
            anyhow::bail!(
                "jcode server reconnected to unexpected session id: expected {session_id}, got {reconnected_session_id}"
            );
        }
        send_desktop_event(
            &event_tx,
            DesktopSessionEvent::Reloaded {
                session_id: reconnected_session_id,
            },
        );
    }
    Ok(session_id)
}

#[cfg(not(unix))]
fn run_server_session(
    _target_session_id: Option<&str>,
    _message: &str,
    _images: Vec<(String, String)>,
    _event_tx: Option<DesktopSessionEventSender>,
    _command_rx: Receiver<DesktopSessionCommand>,
) -> Result<String> {
    anyhow::bail!("desktop server sessions are not implemented on this platform yet")
}

#[cfg(unix)]
fn send_desktop_status(event_tx: &Option<DesktopSessionEventSender>, status: DesktopSessionStatus) {
    send_desktop_event(event_tx, DesktopSessionEvent::Status(status));
}

fn send_desktop_event(event_tx: &Option<DesktopSessionEventSender>, event: DesktopSessionEvent) {
    send_desktop_event_ref(event_tx.as_ref(), event);
}

pub(super) fn send_desktop_event_ref(
    event_tx: Option<&DesktopSessionEventSender>,
    event: DesktopSessionEvent,
) {
    if let Some(event_tx) = event_tx {
        let event_kind = desktop_session_event_kind(&event);
        if event_tx.send(event).is_err() {
            crate::desktop_log::warn(format_args!(
                "jcode-desktop: failed to deliver backend event {event_kind}, receiver is closed"
            ));
        }
    }
}

fn desktop_session_event_kind(event: &DesktopSessionEvent) -> &'static str {
    match event {
        DesktopSessionEvent::Status(_) => "status",
        DesktopSessionEvent::SessionStarted { .. } => "session_started",
        DesktopSessionEvent::SessionRenamed { .. } => "session_renamed",
        DesktopSessionEvent::TextDelta(_) => "text_delta",
        DesktopSessionEvent::TextReplace(_) => "text_replace",
        DesktopSessionEvent::ToolStarted { .. } => "tool_started",
        DesktopSessionEvent::ToolExecuting { .. } => "tool_executing",
        DesktopSessionEvent::ToolInput { .. } => "tool_input",
        DesktopSessionEvent::ToolFinished { .. } => "tool_finished",
        DesktopSessionEvent::ModelChanged { .. } => "model_changed",
        DesktopSessionEvent::ModelCatalog { .. } => "model_catalog",
        DesktopSessionEvent::ModelCatalogError { .. } => "model_catalog_error",
        DesktopSessionEvent::StdinRequest { .. } => "stdin_request",
        DesktopSessionEvent::ReloadProgress { .. } => "reload_progress",
        DesktopSessionEvent::RuntimeMetadata { .. } => "runtime_metadata",
        DesktopSessionEvent::TokenUsage { .. } => "tokens",
        DesktopSessionEvent::SystemNotice { .. } => "system_notice",
        DesktopSessionEvent::SessionCloseRequested { .. } => "session_close_requested",
        DesktopSessionEvent::Reloading { .. } => "reloading",
        DesktopSessionEvent::Reloaded { .. } => "reloaded",
        DesktopSessionEvent::Done => "done",
        DesktopSessionEvent::Error(_) => "error",
    }
}

pub(super) fn socket_path() -> PathBuf {
    if let Ok(custom) = std::env::var("JCODE_SOCKET") {
        return PathBuf::from(custom);
    }
    if let Ok(dir) = std::env::var("JCODE_RUNTIME_DIR") {
        return PathBuf::from(dir).join("jcode.sock");
    }
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        return PathBuf::from(dir).join("jcode.sock");
    }
    std::env::temp_dir()
        .join(format!("jcode-{}", runtime_user_discriminator()))
        .join("jcode.sock")
}

#[cfg(unix)]
fn runtime_user_discriminator() -> String {
    unsafe { libc::geteuid() }.to_string()
}

#[cfg(not(unix))]
fn runtime_user_discriminator() -> String {
    std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "user".to_string())
}

#[cfg(test)]
mod tests;
