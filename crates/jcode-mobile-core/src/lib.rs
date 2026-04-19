use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Screen {
    Onboarding,
    Pairing,
    Chat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: String,
    pub role: MessageRole,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerSummary {
    pub host: String,
    pub port: String,
    pub server_name: String,
    pub server_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingForm {
    pub host: String,
    pub port: String,
    pub pair_code: String,
    pub device_name: String,
}

impl Default for PairingForm {
    fn default() -> Self {
        Self {
            host: String::new(),
            port: "7643".to_string(),
            pair_code: String::new(),
            device_name: "jcode simulator".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimulatorState {
    pub screen: Screen,
    pub connection_state: ConnectionState,
    pub pairing: PairingForm,
    pub saved_servers: Vec<ServerSummary>,
    pub selected_server: Option<ServerSummary>,
    pub status_message: Option<String>,
    pub error_message: Option<String>,
    pub messages: Vec<ChatMessage>,
    pub draft_message: String,
    pub active_session_id: Option<String>,
    pub sessions: Vec<String>,
    pub available_models: Vec<String>,
    pub model_name: Option<String>,
    pub is_processing: bool,
}

impl Default for SimulatorState {
    fn default() -> Self {
        Self::for_scenario(ScenarioName::Onboarding)
    }
}

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
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScenarioName {
    Onboarding,
    PairingReady,
    ConnectedChat,
}

impl ScenarioName {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "onboarding" => Some(Self::Onboarding),
            "pairing_ready" => Some(Self::PairingReady),
            "connected_chat" => Some(Self::ConnectedChat),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Onboarding => "onboarding",
            Self::PairingReady => "pairing_ready",
            Self::ConnectedChat => "connected_chat",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SimulatorAction {
    Reset,
    LoadScenario {
        scenario: ScenarioName,
    },
    SetHost {
        value: String,
    },
    SetPort {
        value: String,
    },
    SetPairCode {
        value: String,
    },
    SetDeviceName {
        value: String,
    },
    SetDraft {
        value: String,
    },
    TapNode {
        node_id: String,
    },
    PairingSucceeded {
        server_name: String,
        server_version: String,
    },
    PairingFailed {
        message: String,
    },
    Connected {
        session_id: String,
    },
    ConnectionFailed {
        message: String,
    },
    AppendAssistantText {
        text: String,
    },
    FinishTurn,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SimulatorEffect {
    PairAndConnect {
        host: String,
        port: String,
        pair_code: String,
        device_name: String,
    },
    SendMessage {
        text: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiNodeRole {
    Screen,
    TextInput,
    Button,
    Banner,
    MessageList,
    Message,
    Composer,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiNode {
    pub id: String,
    pub role: UiNodeRole,
    pub label: String,
    pub value: Option<String>,
    pub visible: bool,
    pub enabled: bool,
    pub children: Vec<UiNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiTree {
    pub screen: Screen,
    pub root: UiNode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransitionRecord {
    pub seq: u64,
    pub timestamp_ms: u64,
    pub action: SimulatorAction,
    pub before: SimulatorState,
    pub after: SimulatorState,
    pub effects: Vec<SimulatorEffect>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectRecord {
    pub seq: u64,
    pub timestamp_ms: u64,
    pub effect: SimulatorEffect,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispatchReport {
    pub transitions: Vec<TransitionRecord>,
    pub effect_records: Vec<EffectRecord>,
    pub final_state: SimulatorState,
}

#[derive(Debug, Clone)]
pub struct SimulatorStore {
    state: SimulatorState,
    transition_log: Vec<TransitionRecord>,
    effect_log: Vec<EffectRecord>,
    next_seq: u64,
    now_ms: u64,
}

impl Default for SimulatorStore {
    fn default() -> Self {
        Self::new(SimulatorState::default())
    }
}

impl SimulatorStore {
    pub fn new(initial_state: SimulatorState) -> Self {
        Self {
            state: initial_state,
            transition_log: Vec::new(),
            effect_log: Vec::new(),
            next_seq: 1,
            now_ms: 0,
        }
    }

    pub fn state(&self) -> &SimulatorState {
        &self.state
    }

    pub fn transition_log(&self) -> &[TransitionRecord] {
        &self.transition_log
    }

    pub fn effect_log(&self) -> &[EffectRecord] {
        &self.effect_log
    }

    pub fn semantic_tree(&self) -> UiTree {
        build_ui_tree(&self.state)
    }

    pub fn state_json(&self) -> anyhow::Result<String> {
        Ok(serde_json::to_string_pretty(&self.state)?)
    }

    pub fn tree_json(&self) -> anyhow::Result<String> {
        Ok(serde_json::to_string_pretty(&self.semantic_tree())?)
    }

    pub fn transition_log_json(&self) -> anyhow::Result<String> {
        Ok(serde_json::to_string_pretty(&self.transition_log)?)
    }

    pub fn dispatch(&mut self, action: SimulatorAction) -> DispatchReport {
        let mut pending = vec![action];
        let mut transitions = Vec::new();
        let mut effect_records = Vec::new();

        while let Some(action) = pending.pop() {
            let before = self.state.clone();
            let reduction = reduce(before.clone(), action.clone());
            self.state = reduction.after.clone();

            let seq = self.next_seq;
            self.next_seq += 1;
            self.now_ms += 1;

            let transition = TransitionRecord {
                seq,
                timestamp_ms: self.now_ms,
                action,
                before,
                after: reduction.after,
                effects: reduction.effects.clone(),
            };
            self.transition_log.push(transition.clone());
            transitions.push(transition);

            for effect in reduction.effects {
                self.now_ms += 1;
                let effect_record = EffectRecord {
                    seq,
                    timestamp_ms: self.now_ms,
                    effect: effect.clone(),
                };
                self.effect_log.push(effect_record.clone());
                effect_records.push(effect_record);
                let follow_ups = auto_follow_up_actions(effect);
                for next in follow_ups.into_iter().rev() {
                    pending.push(next);
                }
            }
        }

        DispatchReport {
            transitions,
            effect_records,
            final_state: self.state.clone(),
        }
    }
}

#[derive(Debug, Clone)]
struct Reduction {
    after: SimulatorState,
    effects: Vec<SimulatorEffect>,
}

fn reduce(mut state: SimulatorState, action: SimulatorAction) -> Reduction {
    let mut effects = Vec::new();
    match action {
        SimulatorAction::Reset => {
            state = SimulatorState::default();
        }
        SimulatorAction::LoadScenario { scenario } => {
            state = SimulatorState::for_scenario(scenario);
        }
        SimulatorAction::SetHost { value } => {
            state.pairing.host = value;
            state.error_message = None;
        }
        SimulatorAction::SetPort { value } => {
            state.pairing.port = value;
            state.error_message = None;
        }
        SimulatorAction::SetPairCode { value } => {
            state.pairing.pair_code = value;
            state.error_message = None;
        }
        SimulatorAction::SetDeviceName { value } => {
            state.pairing.device_name = value;
            state.error_message = None;
        }
        SimulatorAction::SetDraft { value } => {
            state.draft_message = value;
            state.error_message = None;
        }
        SimulatorAction::TapNode { node_id } => match node_id.as_str() {
            "pair.submit" => {
                if state.pairing.host.trim().is_empty() {
                    state.error_message = Some("Host cannot be empty.".to_string());
                } else if state.pairing.pair_code.trim().is_empty() {
                    state.error_message = Some("Enter a simulated pairing code first.".to_string());
                } else if state.pairing.device_name.trim().is_empty() {
                    state.error_message = Some("Device name cannot be empty.".to_string());
                } else {
                    state.screen = Screen::Pairing;
                    state.connection_state = ConnectionState::Connecting;
                    state.status_message = Some(format!(
                        "Pairing to {}:{}...",
                        state.pairing.host, state.pairing.port
                    ));
                    state.error_message = None;
                    effects.push(SimulatorEffect::PairAndConnect {
                        host: state.pairing.host.clone(),
                        port: state.pairing.port.clone(),
                        pair_code: state.pairing.pair_code.clone(),
                        device_name: state.pairing.device_name.clone(),
                    });
                }
            }
            "chat.send" => {
                if state.connection_state != ConnectionState::Connected {
                    state.error_message = Some("Not connected.".to_string());
                } else if state.draft_message.trim().is_empty() {
                    state.error_message = Some("Draft is empty.".to_string());
                } else {
                    let text = state.draft_message.trim().to_string();
                    state.messages.push(ChatMessage {
                        id: format!("msg-user-{}", state.messages.len() + 1),
                        role: MessageRole::User,
                        text: text.clone(),
                    });
                    state.draft_message.clear();
                    state.status_message = Some("Sending simulated message...".to_string());
                    state.error_message = None;
                    state.is_processing = true;
                    effects.push(SimulatorEffect::SendMessage { text });
                }
            }
            "chat.interrupt" => {
                state.is_processing = false;
                state.status_message = Some("Interrupted simulated turn.".to_string());
            }
            _ => {
                state.error_message = Some(format!("Unknown node id: {node_id}"));
            }
        },
        SimulatorAction::PairingSucceeded {
            server_name,
            server_version,
        } => {
            let server = ServerSummary {
                host: state.pairing.host.clone(),
                port: state.pairing.port.clone(),
                server_name,
                server_version,
            };
            state
                .saved_servers
                .retain(|existing| existing.host != server.host || existing.port != server.port);
            state.saved_servers.push(server.clone());
            state.selected_server = Some(server);
            state.status_message = Some("Simulated pairing succeeded.".to_string());
            state.error_message = None;
        }
        SimulatorAction::PairingFailed { message }
        | SimulatorAction::ConnectionFailed { message } => {
            state.screen = Screen::Onboarding;
            state.connection_state = ConnectionState::Disconnected;
            state.status_message = None;
            state.error_message = Some(message);
            state.is_processing = false;
        }
        SimulatorAction::Connected { session_id } => {
            state.screen = Screen::Chat;
            state.connection_state = ConnectionState::Connected;
            state.active_session_id = Some(session_id.clone());
            state.sessions = vec![session_id];
            state.available_models = vec!["gpt-5".to_string(), "claude-sonnet-4".to_string()];
            state.model_name = Some("gpt-5".to_string());
            state.status_message = Some("Connected to simulated jcode server.".to_string());
            state.error_message = None;
            if state.messages.is_empty() {
                state.messages.push(ChatMessage {
                    id: "msg-system-connected".to_string(),
                    role: MessageRole::System,
                    text: "Simulator connected. Send a message to begin.".to_string(),
                });
            }
        }
        SimulatorAction::AppendAssistantText { text } => {
            state.messages.push(ChatMessage {
                id: format!("msg-assistant-{}", state.messages.len() + 1),
                role: MessageRole::Assistant,
                text,
            });
        }
        SimulatorAction::FinishTurn => {
            state.is_processing = false;
            state.status_message = Some("Simulated turn finished.".to_string());
        }
    }

    Reduction {
        after: state,
        effects,
    }
}

fn auto_follow_up_actions(effect: SimulatorEffect) -> Vec<SimulatorAction> {
    match effect {
        SimulatorEffect::PairAndConnect { .. } => vec![
            SimulatorAction::PairingSucceeded {
                server_name: "jcode".to_string(),
                server_version: env!("CARGO_PKG_VERSION").to_string(),
            },
            SimulatorAction::Connected {
                session_id: "session_sim_1".to_string(),
            },
        ],
        SimulatorEffect::SendMessage { text } => vec![
            SimulatorAction::AppendAssistantText {
                text: format!("Simulated response to: {text}"),
            },
            SimulatorAction::FinishTurn,
        ],
    }
}

fn build_ui_tree(state: &SimulatorState) -> UiTree {
    let mut children = Vec::new();

    if let Some(status) = &state.status_message {
        children.push(UiNode {
            id: "banner.status".to_string(),
            role: UiNodeRole::Banner,
            label: "Status".to_string(),
            value: Some(status.clone()),
            visible: true,
            enabled: true,
            children: Vec::new(),
        });
    }

    if let Some(error) = &state.error_message {
        children.push(UiNode {
            id: "banner.error".to_string(),
            role: UiNodeRole::Banner,
            label: "Error".to_string(),
            value: Some(error.clone()),
            visible: true,
            enabled: true,
            children: Vec::new(),
        });
    }

    match state.screen {
        Screen::Onboarding | Screen::Pairing => {
            children.extend([
                UiNode {
                    id: "pair.host".to_string(),
                    role: UiNodeRole::TextInput,
                    label: "Host".to_string(),
                    value: Some(state.pairing.host.clone()),
                    visible: true,
                    enabled: state.connection_state != ConnectionState::Connecting,
                    children: Vec::new(),
                },
                UiNode {
                    id: "pair.port".to_string(),
                    role: UiNodeRole::TextInput,
                    label: "Port".to_string(),
                    value: Some(state.pairing.port.clone()),
                    visible: true,
                    enabled: state.connection_state != ConnectionState::Connecting,
                    children: Vec::new(),
                },
                UiNode {
                    id: "pair.code".to_string(),
                    role: UiNodeRole::TextInput,
                    label: "Pair Code".to_string(),
                    value: Some(state.pairing.pair_code.clone()),
                    visible: true,
                    enabled: state.connection_state != ConnectionState::Connecting,
                    children: Vec::new(),
                },
                UiNode {
                    id: "pair.device_name".to_string(),
                    role: UiNodeRole::TextInput,
                    label: "Device Name".to_string(),
                    value: Some(state.pairing.device_name.clone()),
                    visible: true,
                    enabled: state.connection_state != ConnectionState::Connecting,
                    children: Vec::new(),
                },
                UiNode {
                    id: "pair.submit".to_string(),
                    role: UiNodeRole::Button,
                    label: "Pair & Connect".to_string(),
                    value: None,
                    visible: true,
                    enabled: state.connection_state != ConnectionState::Connecting,
                    children: Vec::new(),
                },
            ]);
        }
        Screen::Chat => {
            let message_children = state
                .messages
                .iter()
                .enumerate()
                .map(|(idx, message)| UiNode {
                    id: format!("message.{idx}"),
                    role: UiNodeRole::Message,
                    label: format!("{:?} message", message.role),
                    value: Some(message.text.clone()),
                    visible: true,
                    enabled: true,
                    children: Vec::new(),
                })
                .collect();
            children.push(UiNode {
                id: "chat.messages".to_string(),
                role: UiNodeRole::MessageList,
                label: "Messages".to_string(),
                value: None,
                visible: true,
                enabled: true,
                children: message_children,
            });
            children.push(UiNode {
                id: "chat.draft".to_string(),
                role: UiNodeRole::Composer,
                label: "Draft".to_string(),
                value: Some(state.draft_message.clone()),
                visible: true,
                enabled: true,
                children: Vec::new(),
            });
            children.push(UiNode {
                id: "chat.send".to_string(),
                role: UiNodeRole::Button,
                label: "Send".to_string(),
                value: None,
                visible: true,
                enabled: state.connection_state == ConnectionState::Connected,
                children: Vec::new(),
            });
            children.push(UiNode {
                id: "chat.interrupt".to_string(),
                role: UiNodeRole::Button,
                label: "Interrupt".to_string(),
                value: None,
                visible: true,
                enabled: state.is_processing,
                children: Vec::new(),
            });
        }
    }

    UiTree {
        screen: state.screen,
        root: UiNode {
            id: "root".to_string(),
            role: UiNodeRole::Screen,
            label: format!("{:?}", state.screen),
            value: None,
            visible: true,
            enabled: true,
            children,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_flow_reaches_connected_chat() {
        let mut store = SimulatorStore::default();
        store.dispatch(SimulatorAction::SetHost {
            value: "devbox.tailnet.ts.net".to_string(),
        });
        store.dispatch(SimulatorAction::SetPairCode {
            value: "123456".to_string(),
        });
        let report = store.dispatch(SimulatorAction::TapNode {
            node_id: "pair.submit".to_string(),
        });

        assert!(!report.transitions.is_empty());
        assert_eq!(store.state().connection_state, ConnectionState::Connected);
        assert_eq!(store.state().screen, Screen::Chat);
    }

    #[test]
    fn sending_message_creates_assistant_reply() {
        let mut store =
            SimulatorStore::new(SimulatorState::for_scenario(ScenarioName::ConnectedChat));
        store.dispatch(SimulatorAction::SetDraft {
            value: "hello simulator".to_string(),
        });
        store.dispatch(SimulatorAction::TapNode {
            node_id: "chat.send".to_string(),
        });

        let last = store
            .state()
            .messages
            .last()
            .expect("assistant reply present");
        assert_eq!(last.role, MessageRole::Assistant);
        assert!(last.text.contains("hello simulator"));
        assert!(!store.state().is_processing);
    }

    #[test]
    fn semantic_tree_reflects_current_screen() {
        let store = SimulatorStore::default();
        let tree = store.semantic_tree();
        assert_eq!(tree.screen, Screen::Onboarding);
        assert!(
            tree.root
                .children
                .iter()
                .any(|node| node.id == "pair.submit")
        );
    }
}
