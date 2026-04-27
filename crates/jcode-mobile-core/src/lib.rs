use serde::{Deserialize, Serialize};

pub mod protocol;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiNodeAction {
    Tap,
    SetText,
    TypeText,
    Scroll,
}

pub const DEFAULT_VIEWPORT_WIDTH: i32 = 390;
pub const DEFAULT_VIEWPORT_HEIGHT: i32 = 844;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl UiRect {
    pub fn contains_point(&self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.width && y < self.y + self.height
    }

    pub fn center(&self) -> (i32, i32) {
        (self.x + self.width / 2, self.y + self.height / 2)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiNode {
    pub id: String,
    pub role: UiNodeRole,
    pub label: String,
    pub value: Option<String>,
    pub visible: bool,
    pub enabled: bool,
    pub focused: bool,
    pub accessibility_label: Option<String>,
    pub accessibility_value: Option<String>,
    pub supported_actions: Vec<UiNodeAction>,
    pub bounds: Option<UiRect>,
    pub children: Vec<UiNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiTree {
    pub screen: Screen,
    pub root: UiNode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenshotSnapshot {
    pub format: String,
    pub width: i32,
    pub height: i32,
    pub theme: String,
    pub hash: String,
    pub svg: String,
    pub layout: UiTree,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenshotDiff {
    pub matches: bool,
    pub expected_hash: String,
    pub actual_hash: String,
    pub expected_len: usize,
    pub actual_len: usize,
    pub first_difference: Option<usize>,
}

pub fn screenshot_snapshot(tree: &UiTree) -> ScreenshotSnapshot {
    let svg = render_svg(tree);
    let hash = stable_hash_hex(svg.as_bytes());
    ScreenshotSnapshot {
        format: "svg".to_string(),
        width: DEFAULT_VIEWPORT_WIDTH,
        height: DEFAULT_VIEWPORT_HEIGHT,
        theme: "jcode-mobile-sim-light".to_string(),
        hash,
        svg,
        layout: tree.clone(),
    }
}

pub fn diff_screenshots(
    expected: &ScreenshotSnapshot,
    actual: &ScreenshotSnapshot,
) -> ScreenshotDiff {
    let first_difference = expected
        .svg
        .as_bytes()
        .iter()
        .zip(actual.svg.as_bytes().iter())
        .position(|(a, b)| a != b)
        .or_else(|| {
            if expected.svg.len() == actual.svg.len() {
                None
            } else {
                Some(expected.svg.len().min(actual.svg.len()))
            }
        });

    ScreenshotDiff {
        matches: expected.hash == actual.hash && first_difference.is_none(),
        expected_hash: expected.hash.clone(),
        actual_hash: actual.hash.clone(),
        expected_len: expected.svg.len(),
        actual_len: actual.svg.len(),
        first_difference,
    }
}

fn render_svg(tree: &UiTree) -> String {
    let mut svg = String::new();
    svg.push_str(&format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{}" height="{}" viewBox="0 0 {} {}" font-family="Inter, ui-sans-serif, system-ui" font-size="14">"#,
        DEFAULT_VIEWPORT_WIDTH, DEFAULT_VIEWPORT_HEIGHT, DEFAULT_VIEWPORT_WIDTH, DEFAULT_VIEWPORT_HEIGHT
    ));
    svg.push_str(r##"<rect x="0" y="0" width="390" height="844" fill="#f8fafc"/>"##);
    render_svg_node(&mut svg, &tree.root, 0);
    svg.push_str("</svg>\n");
    svg
}

fn render_svg_node(svg: &mut String, node: &UiNode, depth: usize) {
    if !node.visible {
        return;
    }

    if let Some(bounds) = node.bounds {
        let (fill, stroke) = match node.role {
            UiNodeRole::Screen => ("none", "none"),
            UiNodeRole::TextInput | UiNodeRole::Composer => ("#ffffff", "#94a3b8"),
            UiNodeRole::Button => {
                if node.enabled {
                    ("#2563eb", "#1d4ed8")
                } else {
                    ("#cbd5e1", "#94a3b8")
                }
            }
            UiNodeRole::Banner => ("#e0f2fe", "#38bdf8"),
            UiNodeRole::MessageList => ("#eef2ff", "#c7d2fe"),
            UiNodeRole::Message => ("#ffffff", "#cbd5e1"),
        };
        if stroke != "none" {
            svg.push_str(&format!(
                r#"<rect data-node="{}" data-role="{:?}" x="{}" y="{}" width="{}" height="{}" rx="10" fill="{}" stroke="{}"/>"#,
                xml_escape(&node.id),
                node.role,
                bounds.x,
                bounds.y,
                bounds.width,
                bounds.height,
                fill,
                stroke
            ));
        }

        if node.role != UiNodeRole::Screen {
            let text = node
                .value
                .as_deref()
                .filter(|value| !value.is_empty())
                .unwrap_or(&node.label);
            let text = truncate_for_svg(text, 54usize.saturating_sub(depth * 4));
            let fill = if node.role == UiNodeRole::Button && node.enabled {
                "#ffffff"
            } else {
                "#0f172a"
            };
            svg.push_str(&format!(
                r#"<text data-node-label="{}" x="{}" y="{}" fill="{}">{}</text>"#,
                xml_escape(&node.id),
                bounds.x + 12,
                bounds.y + 28,
                fill,
                xml_escape(&text)
            ));
        }
    }

    for child in &node.children {
        render_svg_node(svg, child, depth + 1);
    }
}

fn truncate_for_svg(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_string();
    }
    let mut output: String = input.chars().take(max_chars.saturating_sub(1)).collect();
    output.push('…');
    output
}

fn xml_escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn stable_hash_hex(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("fnv1a64:{hash:016x}")
}

pub fn hit_test(tree: &UiTree, x: i32, y: i32) -> Option<&UiNode> {
    hit_test_node(&tree.root, x, y)
}

pub fn hit_test_actionable(tree: &UiTree, x: i32, y: i32, action: UiNodeAction) -> Option<&UiNode> {
    hit_test_actionable_node(&tree.root, x, y, action)
}

fn hit_test_node(node: &UiNode, x: i32, y: i32) -> Option<&UiNode> {
    if !node.visible
        || !node
            .bounds
            .is_some_and(|bounds| bounds.contains_point(x, y))
    {
        return None;
    }

    node.children
        .iter()
        .rev()
        .find_map(|child| hit_test_node(child, x, y))
        .or(Some(node))
}

fn hit_test_actionable_node(
    node: &UiNode,
    x: i32,
    y: i32,
    action: UiNodeAction,
) -> Option<&UiNode> {
    if !node.visible
        || !node
            .bounds
            .is_some_and(|bounds| bounds.contains_point(x, y))
    {
        return None;
    }

    node.children
        .iter()
        .rev()
        .find_map(|child| hit_test_actionable_node(child, x, y, action))
        .or_else(|| {
            if node.enabled && node.supported_actions.contains(&action) {
                Some(node)
            } else {
                None
            }
        })
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

pub const REPLAY_TRACE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayTrace {
    pub schema_version: u32,
    pub name: String,
    pub initial_state: SimulatorState,
    pub actions: Vec<SimulatorAction>,
    pub transitions: Vec<TransitionRecord>,
    pub effects: Vec<EffectRecord>,
    pub final_state: SimulatorState,
}

impl ReplayTrace {
    pub fn record(
        name: impl Into<String>,
        initial_state: SimulatorState,
        actions: Vec<SimulatorAction>,
    ) -> Self {
        let mut store = SimulatorStore::new(initial_state.clone());
        for action in actions.iter().cloned() {
            store.dispatch(action);
        }
        Self {
            schema_version: REPLAY_TRACE_SCHEMA_VERSION,
            name: name.into(),
            initial_state,
            actions,
            transitions: store.transition_log().to_vec(),
            effects: store.effect_log().to_vec(),
            final_state: store.state().clone(),
        }
    }

    pub fn replay(&self) -> Self {
        Self::record(
            self.name.clone(),
            self.initial_state.clone(),
            self.actions.clone(),
        )
    }

    pub fn assert_replays(&self) -> anyhow::Result<()> {
        if self.schema_version != REPLAY_TRACE_SCHEMA_VERSION {
            anyhow::bail!(
                "unsupported replay trace schema version {}, expected {}",
                self.schema_version,
                REPLAY_TRACE_SCHEMA_VERSION
            );
        }
        let replayed = self.replay();
        if &replayed != self {
            anyhow::bail!(
                "replay trace mismatch for {}\nexpected:\n{}\nactual:\n{}",
                self.name,
                serde_json::to_string_pretty(self)?,
                serde_json::to_string_pretty(&replayed)?
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct SimulatorStore {
    initial_state: SimulatorState,
    state: SimulatorState,
    action_log: Vec<SimulatorAction>,
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
            initial_state: initial_state.clone(),
            state: initial_state,
            action_log: Vec::new(),
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

    pub fn action_log(&self) -> &[SimulatorAction] {
        &self.action_log
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

    pub fn replay_trace(&self, name: impl Into<String>) -> ReplayTrace {
        ReplayTrace {
            schema_version: REPLAY_TRACE_SCHEMA_VERSION,
            name: name.into(),
            initial_state: self.initial_state.clone(),
            actions: self.action_log.clone(),
            transitions: self.transition_log.clone(),
            effects: self.effect_log.clone(),
            final_state: self.state.clone(),
        }
    }

    pub fn dispatch(&mut self, action: SimulatorAction) -> DispatchReport {
        self.action_log.push(action.clone());
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
                let follow_ups = FakeJcodeBackend::default().handle_effect(effect);
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

#[derive(Debug, Clone, Default)]
pub struct FakeJcodeBackend;

impl FakeJcodeBackend {
    pub fn handle_effect(&self, effect: SimulatorEffect) -> Vec<SimulatorAction> {
        match effect {
            SimulatorEffect::PairAndConnect {
                host, pair_code, ..
            } => self.pair_and_connect(&host, &pair_code),
            SimulatorEffect::SendMessage { text } => self.send_message(&text),
        }
    }

    fn pair_and_connect(&self, host: &str, pair_code: &str) -> Vec<SimulatorAction> {
        if host.contains("offline") || host.contains("unreachable") {
            return vec![SimulatorAction::ConnectionFailed {
                message: "Server unreachable. Confirm host/port and gateway status.".to_string(),
            }];
        }

        if pair_code != "123456" {
            return vec![SimulatorAction::PairingFailed {
                message: "Invalid or expired pairing code.".to_string(),
            }];
        }

        vec![
            SimulatorAction::PairingSucceeded {
                server_name: "jcode".to_string(),
                server_version: env!("CARGO_PKG_VERSION").to_string(),
            },
            SimulatorAction::Connected {
                session_id: "session_sim_1".to_string(),
            },
        ]
    }

    fn send_message(&self, text: &str) -> Vec<SimulatorAction> {
        vec![
            SimulatorAction::AppendAssistantText {
                text: format!("Simulated response to: {text}"),
            },
            SimulatorAction::FinishTurn,
        ]
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
            focused: false,
            accessibility_label: None,
            accessibility_value: None,
            supported_actions: Vec::new(),
            bounds: None,
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
            focused: false,
            accessibility_label: None,
            accessibility_value: None,
            supported_actions: Vec::new(),
            bounds: None,
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
                    focused: false,
                    accessibility_label: None,
                    accessibility_value: None,
                    supported_actions: Vec::new(),
                    bounds: None,
                    children: Vec::new(),
                },
                UiNode {
                    id: "pair.port".to_string(),
                    role: UiNodeRole::TextInput,
                    label: "Port".to_string(),
                    value: Some(state.pairing.port.clone()),
                    visible: true,
                    enabled: state.connection_state != ConnectionState::Connecting,
                    focused: false,
                    accessibility_label: None,
                    accessibility_value: None,
                    supported_actions: Vec::new(),
                    bounds: None,
                    children: Vec::new(),
                },
                UiNode {
                    id: "pair.code".to_string(),
                    role: UiNodeRole::TextInput,
                    label: "Pair Code".to_string(),
                    value: Some(state.pairing.pair_code.clone()),
                    visible: true,
                    enabled: state.connection_state != ConnectionState::Connecting,
                    focused: false,
                    accessibility_label: None,
                    accessibility_value: None,
                    supported_actions: Vec::new(),
                    bounds: None,
                    children: Vec::new(),
                },
                UiNode {
                    id: "pair.device_name".to_string(),
                    role: UiNodeRole::TextInput,
                    label: "Device Name".to_string(),
                    value: Some(state.pairing.device_name.clone()),
                    visible: true,
                    enabled: state.connection_state != ConnectionState::Connecting,
                    focused: false,
                    accessibility_label: None,
                    accessibility_value: None,
                    supported_actions: Vec::new(),
                    bounds: None,
                    children: Vec::new(),
                },
                UiNode {
                    id: "pair.submit".to_string(),
                    role: UiNodeRole::Button,
                    label: "Pair & Connect".to_string(),
                    value: None,
                    visible: true,
                    enabled: state.connection_state != ConnectionState::Connecting,
                    focused: false,
                    accessibility_label: None,
                    accessibility_value: None,
                    supported_actions: Vec::new(),
                    bounds: None,
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
                    focused: false,
                    accessibility_label: None,
                    accessibility_value: None,
                    supported_actions: Vec::new(),
                    bounds: None,
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
                focused: false,
                accessibility_label: None,
                accessibility_value: None,
                supported_actions: Vec::new(),
                bounds: None,
                children: message_children,
            });
            children.push(UiNode {
                id: "chat.draft".to_string(),
                role: UiNodeRole::Composer,
                label: "Draft".to_string(),
                value: Some(state.draft_message.clone()),
                visible: true,
                enabled: true,
                focused: false,
                accessibility_label: None,
                accessibility_value: None,
                supported_actions: Vec::new(),
                bounds: None,
                children: Vec::new(),
            });
            children.push(UiNode {
                id: "chat.send".to_string(),
                role: UiNodeRole::Button,
                label: "Send".to_string(),
                value: None,
                visible: true,
                enabled: state.connection_state == ConnectionState::Connected,
                focused: false,
                accessibility_label: None,
                accessibility_value: None,
                supported_actions: Vec::new(),
                bounds: None,
                children: Vec::new(),
            });
            children.push(UiNode {
                id: "chat.interrupt".to_string(),
                role: UiNodeRole::Button,
                label: "Interrupt".to_string(),
                value: None,
                visible: true,
                enabled: state.is_processing,
                focused: false,
                accessibility_label: None,
                accessibility_value: None,
                supported_actions: Vec::new(),
                bounds: None,
                children: Vec::new(),
            });
        }
    }

    with_default_layout(with_agent_metadata(UiTree {
        screen: state.screen,
        root: UiNode {
            id: "root".to_string(),
            role: UiNodeRole::Screen,
            label: format!("{:?}", state.screen),
            value: None,
            visible: true,
            enabled: true,
            focused: false,
            accessibility_label: None,
            accessibility_value: None,
            supported_actions: Vec::new(),
            bounds: None,
            children,
        },
    }))
}

fn with_default_layout(mut tree: UiTree) -> UiTree {
    tree.root.bounds = Some(UiRect {
        x: 0,
        y: 0,
        width: DEFAULT_VIEWPORT_WIDTH,
        height: DEFAULT_VIEWPORT_HEIGHT,
    });

    let mut y = 16;
    for child in &mut tree.root.children {
        match child.id.as_str() {
            "banner.status" | "banner.error" => {
                child.bounds = Some(UiRect {
                    x: 16,
                    y,
                    width: DEFAULT_VIEWPORT_WIDTH - 32,
                    height: 44,
                });
                y += 56;
            }
            _ => {}
        }
    }

    match tree.screen {
        Screen::Onboarding | Screen::Pairing => layout_pairing_screen(&mut tree.root.children, y),
        Screen::Chat => layout_chat_screen(&mut tree.root.children, y),
    }

    tree
}

fn layout_pairing_screen(children: &mut [UiNode], mut y: i32) {
    for id in [
        "pair.host",
        "pair.port",
        "pair.code",
        "pair.device_name",
        "pair.submit",
    ] {
        if let Some(node) = children.iter_mut().find(|node| node.id == id) {
            node.bounds = Some(UiRect {
                x: 16,
                y,
                width: DEFAULT_VIEWPORT_WIDTH - 32,
                height: 52,
            });
            y += 64;
        }
    }
}

fn layout_chat_screen(children: &mut [UiNode], y: i32) {
    if let Some(messages) = children.iter_mut().find(|node| node.id == "chat.messages") {
        messages.bounds = Some(UiRect {
            x: 16,
            y,
            width: DEFAULT_VIEWPORT_WIDTH - 32,
            height: 610 - y,
        });
        let mut message_y = y + 8;
        for message in &mut messages.children {
            message.bounds = Some(UiRect {
                x: 24,
                y: message_y,
                width: DEFAULT_VIEWPORT_WIDTH - 48,
                height: 56,
            });
            message_y += 64;
        }
    }

    if let Some(draft) = children.iter_mut().find(|node| node.id == "chat.draft") {
        draft.bounds = Some(UiRect {
            x: 16,
            y: 690,
            width: DEFAULT_VIEWPORT_WIDTH - 32,
            height: 52,
        });
    }
    if let Some(send) = children.iter_mut().find(|node| node.id == "chat.send") {
        send.bounds = Some(UiRect {
            x: DEFAULT_VIEWPORT_WIDTH - 110,
            y: 766,
            width: 94,
            height: 44,
        });
    }
    if let Some(interrupt) = children.iter_mut().find(|node| node.id == "chat.interrupt") {
        interrupt.bounds = Some(UiRect {
            x: 16,
            y: 766,
            width: 120,
            height: 44,
        });
    }
}

fn with_agent_metadata(mut tree: UiTree) -> UiTree {
    annotate_node_for_agents(&mut tree.root);
    tree
}

fn annotate_node_for_agents(node: &mut UiNode) {
    if node.accessibility_label.is_none() {
        node.accessibility_label = Some(node.label.clone());
    }
    if node.accessibility_value.is_none() {
        node.accessibility_value = node.value.clone();
    }

    node.supported_actions = match node.role {
        UiNodeRole::TextInput | UiNodeRole::Composer if node.enabled => {
            vec![UiNodeAction::SetText, UiNodeAction::TypeText]
        }
        UiNodeRole::Button if node.enabled => vec![UiNodeAction::Tap],
        UiNodeRole::MessageList if node.enabled => vec![UiNodeAction::Scroll],
        _ => Vec::new(),
    };

    for child in &mut node.children {
        annotate_node_for_agents(child);
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

        let last = store.state().messages.last();
        assert!(last.is_some(), "assistant reply present");
        let Some(last) = last else {
            return;
        };
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

    #[test]
    fn semantic_tree_exposes_agent_metadata() {
        let store = SimulatorStore::default();
        let tree = store.semantic_tree();

        let pair_submit = tree
            .root
            .children
            .iter()
            .find(|node| node.id == "pair.submit");
        assert!(pair_submit.is_some(), "pair submit node");
        let Some(pair_submit) = pair_submit else {
            return;
        };
        assert_eq!(
            pair_submit.accessibility_label.as_deref(),
            Some("Pair & Connect")
        );
        assert!(pair_submit.supported_actions.contains(&UiNodeAction::Tap));

        let pair_host = tree
            .root
            .children
            .iter()
            .find(|node| node.id == "pair.host");
        assert!(pair_host.is_some(), "pair host node");
        let Some(pair_host) = pair_host else {
            return;
        };
        assert!(pair_host.supported_actions.contains(&UiNodeAction::SetText));
        assert!(
            pair_host
                .supported_actions
                .contains(&UiNodeAction::TypeText)
        );
    }

    #[test]
    fn all_scenarios_parse_round_trip() {
        for scenario in ScenarioName::ALL {
            assert_eq!(ScenarioName::parse(scenario.as_str()), Some(*scenario));
        }
    }

    #[test]
    fn scenario_fixtures_cover_error_processing_and_offline_states() {
        let invalid = SimulatorState::for_scenario(ScenarioName::PairingInvalidCode);
        assert!(
            invalid
                .error_message
                .as_deref()
                .unwrap_or_default()
                .contains("Invalid")
        );

        let streaming = SimulatorState::for_scenario(ScenarioName::ChatStreaming);
        assert!(streaming.is_processing);
        assert_eq!(streaming.screen, Screen::Chat);

        let offline = SimulatorState::for_scenario(ScenarioName::OfflineQueuedMessage);
        assert_eq!(offline.connection_state, ConnectionState::Disconnected);
        assert!(offline.draft_message.contains("Queued"));
    }

    #[test]
    fn fake_backend_rejects_invalid_pairing_code() {
        let mut store =
            SimulatorStore::new(SimulatorState::for_scenario(ScenarioName::PairingReady));
        store.dispatch(SimulatorAction::SetPairCode {
            value: "000000".to_string(),
        });
        store.dispatch(SimulatorAction::TapNode {
            node_id: "pair.submit".to_string(),
        });

        assert_eq!(
            store.state().connection_state,
            ConnectionState::Disconnected
        );
        assert!(
            store
                .state()
                .error_message
                .as_deref()
                .unwrap_or_default()
                .contains("Invalid")
        );
    }

    #[test]
    fn fake_backend_reports_unreachable_host() {
        let mut store =
            SimulatorStore::new(SimulatorState::for_scenario(ScenarioName::PairingReady));
        store.dispatch(SimulatorAction::SetHost {
            value: "offline.tailnet.ts.net".to_string(),
        });
        store.dispatch(SimulatorAction::TapNode {
            node_id: "pair.submit".to_string(),
        });

        assert_eq!(
            store.state().connection_state,
            ConnectionState::Disconnected
        );
        assert!(
            store
                .state()
                .error_message
                .as_deref()
                .unwrap_or_default()
                .contains("unreachable")
        );
    }

    #[test]
    fn replay_trace_records_and_replays_deterministically() {
        let actions = vec![
            SimulatorAction::TapNode {
                node_id: "pair.submit".to_string(),
            },
            SimulatorAction::SetDraft {
                value: "hello replay".to_string(),
            },
            SimulatorAction::TapNode {
                node_id: "chat.send".to_string(),
            },
        ];
        let trace = ReplayTrace::record(
            "pairing-ready-chat-send",
            SimulatorState::for_scenario(ScenarioName::PairingReady),
            actions,
        );
        trace.assert_replays().expect("trace replays");
        assert_eq!(trace.actions.len(), 3);
        assert_eq!(trace.transitions.len(), 7);
        assert_eq!(trace.effects.len(), 2);
        assert_eq!(trace.final_state.screen, Screen::Chat);
        assert!(
            trace
                .final_state
                .messages
                .iter()
                .any(|message| message.text.contains("hello replay"))
        );
    }

    #[test]
    fn golden_replay_trace_matches_core_behavior() {
        let golden = include_str!("../tests/golden/pairing_ready_chat_send.json");
        let trace: ReplayTrace = serde_json::from_str(golden).expect("parse golden replay trace");
        trace.assert_replays().expect("golden trace replays");
    }

    #[test]
    fn layout_bounds_support_hit_testing() {
        let store = SimulatorStore::new(SimulatorState::for_scenario(ScenarioName::PairingReady));
        let tree = store.semantic_tree();
        let submit = tree
            .root
            .children
            .iter()
            .find(|node| node.id == "pair.submit")
            .expect("pair.submit node");
        let (x, y) = submit.bounds.expect("pair.submit bounds").center();
        assert_eq!(
            hit_test(&tree, x, y).map(|node| node.id.as_str()),
            Some("pair.submit")
        );
        assert_eq!(
            hit_test_actionable(&tree, x, y, UiNodeAction::Tap).map(|node| node.id.as_str()),
            Some("pair.submit")
        );
    }

    #[test]
    fn chat_layout_hit_tests_send_button() {
        let store = SimulatorStore::new(SimulatorState::for_scenario(ScenarioName::ConnectedChat));
        let tree = store.semantic_tree();
        assert_eq!(
            hit_test_actionable(&tree, 330, 788, UiNodeAction::Tap).map(|node| node.id.as_str()),
            Some("chat.send")
        );
    }

    #[test]
    fn screenshot_snapshot_is_deterministic_svg_with_layout() {
        let store = SimulatorStore::new(SimulatorState::for_scenario(ScenarioName::ConnectedChat));
        let tree = store.semantic_tree();
        let first = screenshot_snapshot(&tree);
        let second = screenshot_snapshot(&tree);

        assert_eq!(first, second);
        assert_eq!(first.width, DEFAULT_VIEWPORT_WIDTH);
        assert_eq!(first.height, DEFAULT_VIEWPORT_HEIGHT);
        assert!(first.hash.starts_with("fnv1a64:"));
        assert!(first.svg.contains("data-node=\"chat.send\""));
        assert!(first.layout.root.bounds.is_some());
    }

    #[test]
    fn screenshot_diff_reports_mismatch() {
        let store = SimulatorStore::new(SimulatorState::for_scenario(ScenarioName::ConnectedChat));
        let mut expected = screenshot_snapshot(&store.semantic_tree());
        let actual = expected.clone();
        expected.svg.push_str("<!-- changed -->");
        expected.hash = "fnv1a64:changed".to_string();

        let diff = diff_screenshots(&expected, &actual);
        assert!(!diff.matches);
        assert!(diff.first_difference.is_some());
    }
}
