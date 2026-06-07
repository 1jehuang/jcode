#![allow(dead_code)]

use crate::desktop_app_driver::DesktopUiSnapshot;
use crate::desktop_scene::DesktopScene;
use serde::{Deserialize, Serialize};

pub(crate) const DESKTOP_HOST_WORKER_PROTOCOL_VERSION: u16 = 1;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct DesktopProtocolEnvelope<T> {
    pub(crate) protocol_version: u16,
    pub(crate) sequence: u64,
    pub(crate) payload: T,
}

impl<T> DesktopProtocolEnvelope<T> {
    pub(crate) fn new(sequence: u64, payload: T) -> Self {
        Self {
            protocol_version: DESKTOP_HOST_WORKER_PROTOCOL_VERSION,
            sequence,
            payload,
        }
    }

    pub(crate) fn validate_version(&self) -> Result<(), DesktopProtocolCompatibilityError> {
        validate_desktop_protocol_version(self.protocol_version)
    }
}

pub(crate) fn validate_desktop_protocol_version(
    protocol_version: u16,
) -> Result<(), DesktopProtocolCompatibilityError> {
    if protocol_version == DESKTOP_HOST_WORKER_PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(
            DesktopProtocolCompatibilityError::UnsupportedProtocolVersion {
                expected: DESKTOP_HOST_WORKER_PROTOCOL_VERSION,
                actual: protocol_version,
            },
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DesktopProtocolCompatibilityError {
    UnsupportedProtocolVersion { expected: u16, actual: u16 },
}

impl std::fmt::Display for DesktopProtocolCompatibilityError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedProtocolVersion { expected, actual } => write!(
                formatter,
                "unsupported desktop host-worker protocol version {actual}; expected {expected}"
            ),
        }
    }
}

impl std::error::Error for DesktopProtocolCompatibilityError {}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum DesktopHostToWorkerMessage {
    Initialize(DesktopWorkerInit),
    Input(DesktopInputEvent),
    SessionEvents(DesktopSessionEventBatchWire),
    SnapshotRequest { request_id: u64 },
    MetricsAck { through_sequence: u64 },
    Shutdown { reason: DesktopWorkerShutdownReason },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct DesktopWorkerInit {
    pub(crate) mode: DesktopWorkerMode,
    pub(crate) snapshot: Option<DesktopUiSnapshot>,
    pub(crate) window: DesktopWindowState,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct DesktopSessionEventBatchWire {
    pub(crate) events: Vec<DesktopSessionEventWire>,
    pub(crate) raw_event_count: usize,
    pub(crate) raw_payload_bytes: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) enum DesktopSessionEventWire {
    Status {
        message: String,
    },
    AssistantTextDelta {
        text: String,
    },
    ToolStarted {
        id: String,
        title: String,
    },
    ToolFinished {
        id: String,
        title: String,
        success: bool,
    },
    Error {
        message: String,
    },
    RawJson {
        event_type: String,
        payload: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) enum DesktopWorkerMode {
    SingleSession,
    Workspace,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct DesktopWindowState {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) scale_factor: f32,
    pub(crate) focused: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum DesktopInputEvent {
    Key(DesktopKeyEvent),
    Mouse(DesktopMouseEvent),
    Window(DesktopWindowEvent),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct DesktopKeyEvent {
    pub(crate) key: String,
    pub(crate) text: Option<String>,
    pub(crate) pressed: bool,
    pub(crate) modifiers: DesktopKeyModifiers,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct DesktopKeyModifiers {
    pub(crate) shift: bool,
    pub(crate) ctrl: bool,
    pub(crate) alt: bool,
    pub(crate) super_key: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum DesktopMouseEvent {
    Move {
        x: f32,
        y: f32,
    },
    Button {
        button: DesktopMouseButton,
        pressed: bool,
    },
    Wheel {
        delta_x: f32,
        delta_y: f32,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) enum DesktopMouseButton {
    Left,
    Right,
    Middle,
    Other(u16),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum DesktopWindowEvent {
    Resized {
        width: u32,
        height: u32,
        scale_factor: f32,
    },
    Focused(bool),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) enum DesktopWorkerShutdownReason {
    Reload,
    HostExit,
    ProtocolMismatch,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum DesktopWorkerToHostMessage {
    Ready(DesktopWorkerReady),
    Scene(DesktopSceneUpdate),
    Snapshot(DesktopSnapshotResponse),
    ReloadRequested,
    Metrics(DesktopWorkerMetricBatch),
    Log(DesktopWorkerLog),
    Exited(DesktopWorkerExit),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct DesktopWorkerReady {
    pub(crate) worker_pid: u32,
    pub(crate) mode: DesktopWorkerMode,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct DesktopSceneUpdate {
    pub(crate) scene: DesktopScene,
    pub(crate) animation_active: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct DesktopSnapshotResponse {
    pub(crate) request_id: u64,
    pub(crate) snapshot: DesktopUiSnapshot,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct DesktopWorkerMetricBatch {
    pub(crate) metrics: Vec<DesktopWorkerMetric>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct DesktopWorkerMetric {
    pub(crate) name: DesktopMetricName,
    pub(crate) value: f64,
    pub(crate) unit: DesktopMetricUnit,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) enum DesktopMetricName {
    WindowCreated,
    EventLoopEntered,
    WgpuInitStarted,
    WgpuReady,
    FirstHostPixels,
    FirstGpuFrame,
    FirstLiveContentFrame,
    WorkerSpawned,
    WorkerReady,
    WorkerRestarted,
    ReloadBlackout,
}

impl DesktopMetricName {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::WindowCreated => "window_created",
            Self::EventLoopEntered => "event_loop_entered",
            Self::WgpuInitStarted => "wgpu_init_started",
            Self::WgpuReady => "wgpu_ready",
            Self::FirstHostPixels => "first_host_pixels",
            Self::FirstGpuFrame => "first_gpu_frame",
            Self::FirstLiveContentFrame => "first_live_content_frame",
            Self::WorkerSpawned => "worker_spawned",
            Self::WorkerReady => "worker_ready",
            Self::WorkerRestarted => "worker_restarted",
            Self::ReloadBlackout => "reload_blackout",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) enum DesktopMetricUnit {
    Milliseconds,
    Count,
}

impl DesktopMetricUnit {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Milliseconds => "ms",
            Self::Count => "count",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct DesktopWorkerLog {
    pub(crate) level: DesktopWorkerLogLevel,
    pub(crate) message: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) enum DesktopWorkerLogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct DesktopWorkerExit {
    pub(crate) code: Option<i32>,
    pub(crate) reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_to_worker_message_round_trips() {
        let message = DesktopProtocolEnvelope::new(
            7,
            DesktopHostToWorkerMessage::Input(DesktopInputEvent::Key(DesktopKeyEvent {
                key: "Enter".to_string(),
                text: Some("\n".to_string()),
                pressed: true,
                modifiers: DesktopKeyModifiers {
                    shift: true,
                    ..Default::default()
                },
            })),
        );

        let encoded = serde_json::to_string(&message).expect("serialize message");
        let decoded: DesktopProtocolEnvelope<DesktopHostToWorkerMessage> =
            serde_json::from_str(&encoded).expect("deserialize message");

        assert_eq!(decoded, message);
    }

    #[test]
    fn protocol_envelope_validates_version() {
        let mut message = DesktopProtocolEnvelope::new(
            1,
            DesktopHostToWorkerMessage::SnapshotRequest { request_id: 99 },
        );

        assert_eq!(message.validate_version(), Ok(()));

        message.protocol_version = DESKTOP_HOST_WORKER_PROTOCOL_VERSION + 1;
        assert_eq!(
            message.validate_version(),
            Err(
                DesktopProtocolCompatibilityError::UnsupportedProtocolVersion {
                    expected: DESKTOP_HOST_WORKER_PROTOCOL_VERSION,
                    actual: DESKTOP_HOST_WORKER_PROTOCOL_VERSION + 1,
                }
            )
        );
        assert!(
            message
                .validate_version()
                .unwrap_err()
                .to_string()
                .contains("unsupported")
        );
    }

    #[test]
    fn worker_to_host_scene_message_round_trips() {
        let message = DesktopProtocolEnvelope::new(
            9,
            DesktopWorkerToHostMessage::Scene(DesktopSceneUpdate {
                scene: DesktopScene::default(),
                animation_active: false,
            }),
        );

        let encoded = serde_json::to_string(&message).expect("serialize message");
        let decoded: DesktopProtocolEnvelope<DesktopWorkerToHostMessage> =
            serde_json::from_str(&encoded).expect("deserialize message");

        assert_eq!(decoded, message);
    }

    #[test]
    fn metric_names_cover_reload_and_startup_milestones() {
        let names = [
            DesktopMetricName::WindowCreated,
            DesktopMetricName::EventLoopEntered,
            DesktopMetricName::WgpuInitStarted,
            DesktopMetricName::WgpuReady,
            DesktopMetricName::FirstHostPixels,
            DesktopMetricName::FirstGpuFrame,
            DesktopMetricName::FirstLiveContentFrame,
            DesktopMetricName::WorkerSpawned,
            DesktopMetricName::WorkerReady,
            DesktopMetricName::WorkerRestarted,
            DesktopMetricName::ReloadBlackout,
        ];
        let labels = names
            .iter()
            .map(DesktopMetricName::as_str)
            .collect::<Vec<_>>();

        assert_eq!(labels.len(), 11);
        assert!(labels.contains(&"first_host_pixels"));
        assert!(labels.contains(&"reload_blackout"));
    }

    #[test]
    fn worker_metrics_round_trip_with_typed_names() {
        let message = DesktopProtocolEnvelope::new(
            10,
            DesktopWorkerToHostMessage::Metrics(DesktopWorkerMetricBatch {
                metrics: vec![DesktopWorkerMetric {
                    name: DesktopMetricName::ReloadBlackout,
                    value: 12.5,
                    unit: DesktopMetricUnit::Milliseconds,
                }],
            }),
        );

        let encoded = serde_json::to_string(&message).expect("serialize message");
        let decoded: DesktopProtocolEnvelope<DesktopWorkerToHostMessage> =
            serde_json::from_str(&encoded).expect("deserialize message");

        assert_eq!(decoded, message);
    }
}
