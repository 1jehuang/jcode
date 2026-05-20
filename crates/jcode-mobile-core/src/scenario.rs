use serde::{Deserialize, Serialize};

use super::{
    ChatMessage, ConnectionState, MessageRole, PairingForm, Screen, ServerSummary, SimulatorState,
};

impl SimulatorState {
    pub fn for_scenario(scenario: ScenarioName) -> Self {
        match scenario {
            ScenarioName::Onboarding => Self {
                screen: Screen::Onboarding,
                connection_state: ConnectionState::Disconnected,
                pairing: PairingForm::default(),
                saved_servers: Vec::new(),
                selected_server: None,
                status_message: Some("Ready to pair with a jcode server.".to_string()),
                error_message: None,
                messages: Vec::new(),
                draft_message: String::new(),
                active_session_id: None,
                sessions: Vec::new(),
                available_models: Vec::new(),
                model_name: None,
                is_processing: false,
            },
            ScenarioName::PairingReady => Self {
                pairing: PairingForm {
                    host: "devbox.tailnet.ts.net".to_string(),
                    port: "7643".to_string(),
                    pair_code: "123456".to_string(),
                    device_name: "jcode simulator".to_string(),
                },
                status_message: Some("Fields prefilled for simulated pairing.".to_string()),
                ..Self::for_scenario(ScenarioName::Onboarding)
            },
            ScenarioName::ConnectedChat => {
                let server = ServerSummary {
                    host: "devbox.tailnet.ts.net".to_string(),
                    port: "7643".to_string(),
                    server_name: "jcode".to_string(),
                    server_version: env!("CARGO_PKG_VERSION").to_string(),
                };
                Self {
                    screen: Screen::Chat,
                    connection_state: ConnectionState::Connected,
                    pairing: PairingForm {
                        host: server.host.clone(),
                        port: server.port.clone(),
                        pair_code: String::new(),
                        device_name: "jcode simulator".to_string(),
                    },
                    saved_servers: vec![server.clone()],
                    selected_server: Some(server),
                    status_message: Some("Connected to simulated jcode server.".to_string()),
                    error_message: None,
                    messages: vec![
                        ChatMessage {
                            id: "msg-user-1".to_string(),
                            role: MessageRole::User,
                            text: "Can you summarize the simulator architecture?".to_string(),
                        },
                        ChatMessage {
                            id: "msg-assistant-1".to_string(),
                            role: MessageRole::Assistant,
                            text: "The simulator is headless-first, automation-first, and shares state semantics with the future iOS app.".to_string(),
                        },
                    ],
                    draft_message: String::new(),
                    active_session_id: Some("session_sim_1".to_string()),
                    sessions: vec!["session_sim_1".to_string(), "session_sim_2".to_string()],
                    available_models: vec!["gpt-5".to_string(), "claude-sonnet-4".to_string()],
                    model_name: Some("gpt-5".to_string()),
                    is_processing: false,
                }
            }
            ScenarioName::PairingInvalidCode => Self {
                pairing: PairingForm {
                    host: "devbox.tailnet.ts.net".to_string(),
                    port: "7643".to_string(),
                    pair_code: "000000".to_string(),
                    device_name: "jcode simulator".to_string(),
                },
                status_message: None,
                error_message: Some("Invalid or expired pairing code.".to_string()),
                ..Self::for_scenario(ScenarioName::Onboarding)
            },
            ScenarioName::ServerUnreachable => Self {
                pairing: PairingForm {
                    host: "offline.tailnet.ts.net".to_string(),
                    port: "7643".to_string(),
                    pair_code: "123456".to_string(),
                    device_name: "jcode simulator".to_string(),
                },
                status_message: None,
                error_message: Some(
                    "Server unreachable. Confirm host/port and gateway status.".to_string(),
                ),
                ..Self::for_scenario(ScenarioName::Onboarding)
            },
            ScenarioName::ConnectedEmptyChat => {
                let mut state = Self::for_scenario(ScenarioName::ConnectedChat);
                state.messages.clear();
                state.status_message = Some("Connected to simulated empty chat.".to_string());
                state
            }
            ScenarioName::ChatStreaming => {
                let mut state = Self::for_scenario(ScenarioName::ConnectedChat);
                state.messages.push(ChatMessage {
                    id: "msg-user-streaming".to_string(),
                    role: MessageRole::User,
                    text: "Run the mobile simulator smoke test.".to_string(),
                });
                state.messages.push(ChatMessage {
                    id: "msg-assistant-streaming".to_string(),
                    role: MessageRole::Assistant,
                    text: "Running the Linux-native simulator".to_string(),
                });
                state.status_message = Some("Assistant response is streaming.".to_string());
                state.is_processing = true;
                state
            }
            ScenarioName::ToolApprovalRequired => {
                let mut state = Self::for_scenario(ScenarioName::ConnectedChat);
                state.messages.push(ChatMessage {
                    id: "msg-tool-approval".to_string(),
                    role: MessageRole::System,
                    text: "Tool approval required: bash: cargo test -p jcode-mobile-core."
                        .to_string(),
                });
                state.status_message = Some("Waiting for simulated tool approval.".to_string());
                state.is_processing = true;
                state
            }
            ScenarioName::ToolFailed => {
                let mut state = Self::for_scenario(ScenarioName::ConnectedChat);
                state.messages.push(ChatMessage {
                    id: "msg-tool-failed".to_string(),
                    role: MessageRole::System,
                    text: "Simulated tool failed: exit status 1.".to_string(),
                });
                state.error_message = Some("Last simulated tool failed.".to_string());
                state
            }
            ScenarioName::NetworkReconnect => {
                let mut state = Self::for_scenario(ScenarioName::ConnectedChat);
                state.connection_state = ConnectionState::Connecting;
                state.status_message =
                    Some("Reconnecting to simulated jcode server...".to_string());
                state
            }
            ScenarioName::OfflineQueuedMessage => {
                let mut state = Self::for_scenario(ScenarioName::ConnectedChat);
                state.connection_state = ConnectionState::Disconnected;
                state.draft_message = "Queued while offline".to_string();
                state.status_message =
                    Some("Message queued until simulated reconnect.".to_string());
                state
            }
            ScenarioName::LongRunningTask => {
                let mut state = Self::for_scenario(ScenarioName::ConnectedChat);
                state.messages.push(ChatMessage {
                    id: "msg-long-running".to_string(),
                    role: MessageRole::Assistant,
                    text: "Long-running simulated task is still in progress.".to_string(),
                });
                state.status_message = Some("Long-running simulated task in progress.".to_string());
                state.is_processing = true;
                state
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScenarioName {
    Onboarding,
    PairingReady,
    ConnectedChat,
    PairingInvalidCode,
    ServerUnreachable,
    ConnectedEmptyChat,
    ChatStreaming,
    ToolApprovalRequired,
    ToolFailed,
    NetworkReconnect,
    OfflineQueuedMessage,
    LongRunningTask,
}

impl ScenarioName {
    pub const ALL: &'static [Self] = &[
        Self::Onboarding,
        Self::PairingReady,
        Self::ConnectedChat,
        Self::PairingInvalidCode,
        Self::ServerUnreachable,
        Self::ConnectedEmptyChat,
        Self::ChatStreaming,
        Self::ToolApprovalRequired,
        Self::ToolFailed,
        Self::NetworkReconnect,
        Self::OfflineQueuedMessage,
        Self::LongRunningTask,
    ];

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "onboarding" => Some(Self::Onboarding),
            "pairing_ready" => Some(Self::PairingReady),
            "connected_chat" => Some(Self::ConnectedChat),
            "pairing_invalid_code" => Some(Self::PairingInvalidCode),
            "server_unreachable" => Some(Self::ServerUnreachable),
            "connected_empty_chat" => Some(Self::ConnectedEmptyChat),
            "chat_streaming" => Some(Self::ChatStreaming),
            "tool_approval_required" => Some(Self::ToolApprovalRequired),
            "tool_failed" => Some(Self::ToolFailed),
            "network_reconnect" => Some(Self::NetworkReconnect),
            "offline_queued_message" => Some(Self::OfflineQueuedMessage),
            "long_running_task" => Some(Self::LongRunningTask),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Onboarding => "onboarding",
            Self::PairingReady => "pairing_ready",
            Self::ConnectedChat => "connected_chat",
            Self::PairingInvalidCode => "pairing_invalid_code",
            Self::ServerUnreachable => "server_unreachable",
            Self::ConnectedEmptyChat => "connected_empty_chat",
            Self::ChatStreaming => "chat_streaming",
            Self::ToolApprovalRequired => "tool_approval_required",
            Self::ToolFailed => "tool_failed",
            Self::NetworkReconnect => "network_reconnect",
            Self::OfflineQueuedMessage => "offline_queued_message",
            Self::LongRunningTask => "long_running_task",
        }
    }
}
