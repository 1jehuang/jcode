use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const DEFAULT_GATEWAY_PORT: u16 = 7643;

fn is_false(value: &bool) -> bool {
    !*value
}
fn is_empty_images(images: &[(String, String)]) -> bool {
    images.is_empty()
}
fn default_model_direction() -> i8 {
    1
}

/// Requests sent by the mobile app to the jcode gateway.
/// Mirrors the current Swift `Request` enum in `ios/Sources/JCodeKit/Protocol.swift`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MobileRequest {
    Subscribe {
        id: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        working_dir: Option<String>,
    },
    Message {
        id: u64,
        content: String,
        #[serde(default, skip_serializing_if = "is_empty_images")]
        images: Vec<(String, String)>,
    },
    Cancel {
        id: u64,
    },
    Ping {
        id: u64,
    },
    GetHistory {
        id: u64,
    },
    State {
        id: u64,
    },
    Clear {
        id: u64,
    },
    ResumeSession {
        id: u64,
        session_id: String,
    },
    CycleModel {
        id: u64,
        #[serde(default = "default_model_direction")]
        direction: i8,
    },
    SetModel {
        id: u64,
        model: String,
    },
    Compact {
        id: u64,
    },
    SoftInterrupt {
        id: u64,
        content: String,
        #[serde(default, skip_serializing_if = "is_false")]
        urgent: bool,
    },
    CancelSoftInterrupts {
        id: u64,
    },
    BackgroundTool {
        id: u64,
    },
    Split {
        id: u64,
    },
    StdinResponse {
        id: u64,
        request_id: String,
        input: String,
    },
}

impl MobileRequest {
    pub fn id(&self) -> u64 {
        match self {
            Self::Subscribe { id, .. }
            | Self::Message { id, .. }
            | Self::Cancel { id }
            | Self::Ping { id }
            | Self::GetHistory { id }
            | Self::State { id }
            | Self::Clear { id }
            | Self::ResumeSession { id, .. }
            | Self::CycleModel { id, .. }
            | Self::SetModel { id, .. }
            | Self::Compact { id }
            | Self::SoftInterrupt { id, .. }
            | Self::CancelSoftInterrupts { id }
            | Self::BackgroundTool { id }
            | Self::Split { id }
            | Self::StdinResponse { id, .. } => *id,
        }
    }

    pub fn to_gateway_json(&self) -> anyhow::Result<String> {
        Ok(serde_json::to_string(self)?)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileGatewayConfig {
    pub host: String,
    pub port: u16,
    pub use_tls: bool,
}

impl MobileGatewayConfig {
    pub fn new(host: impl Into<String>, port: u16, mut use_tls: bool) -> anyhow::Result<Self> {
        let host_str = host.into();
        let trimmed = host_str.trim();
        if trimmed.starts_with("https://") || trimmed.starts_with("wss://") {
            use_tls = true;
        }
        let host = normalize_gateway_host(trimmed)?;
        Ok(Self {
            host,
            port,
            use_tls,
        })
    }

    pub fn endpoints(&self) -> MobileGatewayEndpoints {
        let http_scheme = if self.use_tls { "https" } else { "http" };
        let ws_scheme = if self.use_tls { "wss" } else { "ws" };
        let authority = format!("{}:{}", self.host, self.port);
        MobileGatewayEndpoints {
            base_http_url: format!("{http_scheme}://{authority}"),
            health_url: format!("{http_scheme}://{authority}/health"),
            pair_url: format!("{http_scheme}://{authority}/pair"),
            websocket_url: format!("{ws_scheme}://{authority}/ws"),
        }
    }
}

impl Default for MobileGatewayConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: DEFAULT_GATEWAY_PORT,
            use_tls: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileGatewayEndpoints {
    pub base_http_url: String,
    pub health_url: String,
    pub pair_url: String,
    pub websocket_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobilePairingConfig {
    pub code: String,
    pub device_id: String,
    pub device_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apns_token: Option<String>,
}

impl From<MobilePairingConfig> for PairRequest {
    fn from(value: MobilePairingConfig) -> Self {
        Self {
            code: value.code,
            device_id: value.device_id,
            device_name: value.device_name,
            apns_token: value.apns_token,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SerializedMobileRequest {
    pub id: u64,
    pub json: String,
}

pub fn serialize_mobile_request(
    request: &MobileRequest,
) -> anyhow::Result<SerializedMobileRequest> {
    Ok(SerializedMobileRequest {
        id: request.id(),
        json: request.to_gateway_json()?,
    })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DecodedMobileServerEvent {
    Known(MobileServerEvent),
    Unknown(RawMobileServerEvent),
}

pub fn decode_mobile_server_event_lossy(value: Value) -> anyhow::Result<DecodedMobileServerEvent> {
    match serde_json::from_value::<MobileServerEvent>(value.clone()) {
        Ok(event) => Ok(DecodedMobileServerEvent::Known(event)),
        Err(_) => {
            let event_type = value
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string();
            Ok(DecodedMobileServerEvent::Unknown(RawMobileServerEvent {
                event_type,
                raw: value,
            }))
        }
    }
}

fn normalize_gateway_host(input: &str) -> anyhow::Result<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        anyhow::bail!("gateway host cannot be empty");
    }
    let without_scheme = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .or_else(|| trimmed.strip_prefix("wss://"))
        .or_else(|| trimmed.strip_prefix("ws://"))
        .unwrap_or(trimmed);
    let host = without_scheme
        .split('/')
        .next()
        .unwrap_or(without_scheme)
        .trim_end_matches('/');
    if host.is_empty() {
        anyhow::bail!("gateway host cannot be empty");
    }
    Ok(host.to_string())
}

/// Events received by the mobile app from the jcode gateway.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MobileServerEvent {
    Ack {
        id: u64,
    },
    TextDelta {
        text: String,
    },
    TextReplace {
        text: String,
    },
    ToolStart {
        id: String,
        name: String,
    },
    ToolInput {
        delta: String,
    },
    ToolExec {
        id: String,
        name: String,
    },
    ToolDone {
        id: String,
        name: String,
        output: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    #[serde(rename = "tokens")]
    TokenUsage {
        input: u64,
        output: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_read_input: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_creation_input: Option<u64>,
    },
    UpstreamProvider {
        provider: String,
    },
    Done {
        id: u64,
    },
    Error {
        id: u64,
        message: String,
    },
    Pong {
        id: u64,
    },
    State {
        id: u64,
        session_id: String,
        message_count: usize,
        is_processing: bool,
    },
    SessionId {
        session_id: String,
    },
    History(HistoryPayload),
    Reloading {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        new_socket: Option<String>,
    },
    ReloadProgress {
        step: String,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        success: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<String>,
    },
    ModelChanged {
        id: u64,
        model: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    Notification(MobileNotification),
    SwarmStatus {
        members: Vec<SwarmMemberStatus>,
    },
    McpStatus {
        servers: Vec<String>,
    },
    SoftInterruptInjected {
        content: String,
        point: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tools_skipped: Option<usize>,
    },
    Interrupted,
    MemoryInjected {
        count: usize,
        prompt: String,
        prompt_chars: usize,
        computed_age_ms: u64,
    },
    SplitResponse {
        id: u64,
        new_session_id: String,
        new_session_name: String,
    },
    CompactResult {
        id: u64,
        message: String,
        success: bool,
    },
    StdinRequest {
        request_id: String,
        prompt: String,
        is_password: bool,
        tool_call_id: String,
    },
}

/// Lossless event envelope for preserving unknown gateway events in simulator/fake-backend work.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawMobileServerEvent {
    pub event_type: String,
    pub raw: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryPayload {
    pub session_id: String,
    #[serde(default)]
    pub messages: Vec<HistoryMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_type: Option<String>,
    #[serde(default)]
    pub available_models: Vec<String>,
    #[serde(default)]
    pub all_sessions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_canary: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub was_interrupted: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<TokenTotals>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenTotals {
    pub input: u64,
    pub output: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryMessage {
    pub role: String,
    #[serde(default)]
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_data: Option<HistoryToolData>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryToolData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileNotification {
    pub title: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwarmMemberStatus {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairRequest {
    pub code: String,
    pub device_id: String,
    pub device_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apns_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairResponse {
    pub token: String,
    pub server_name: String,
    pub server_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairErrorBody {
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub gateway: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn mobile_request_matches_gateway_json_shape() {
        let request = MobileRequest::Message {
            id: 7,
            content: "hello".to_string(),
            images: vec![("image/jpeg".to_string(), "abc".to_string())],
        };
        let value = serde_json::to_value(request);
        assert!(value.is_ok(), "request should serialize");
        let Ok(value) = value else {
            return;
        };
        assert_eq!(
            value,
            json!({"type":"message","id":7,"content":"hello","images":[["image/jpeg","abc"]]})
        );
    }

    #[test]
    fn mobile_request_omits_empty_optional_fields() {
        let request = MobileRequest::Subscribe {
            id: 1,
            working_dir: None,
        };
        let value = serde_json::to_value(request);
        assert!(value.is_ok(), "request should serialize");
        let Ok(value) = value else {
            return;
        };
        assert_eq!(value, json!({"type":"subscribe","id":1}));
    }

    #[test]
    fn mobile_event_decodes_text_replace() {
        let event: Result<MobileServerEvent, _> =
            serde_json::from_value(json!({"type":"text_replace","text":"replacement"}));
        assert!(event.is_ok(), "text_replace event should decode");
        let Ok(event) = event else {
            return;
        };
        assert_eq!(
            event,
            MobileServerEvent::TextReplace {
                text: "replacement".to_string()
            }
        );
    }

    #[test]
    fn history_payload_decodes_server_metadata() {
        let event: Result<MobileServerEvent, _> = serde_json::from_value(
            json!({"type":"history","session_id":"s1","server_name":"jcode","provider_model":"gpt-5","available_models":["gpt-5","claude-sonnet-4"],"all_sessions":["s1","s2"],"messages":[{"role":"assistant","content":"hi"}]}),
        );
        assert!(event.is_ok(), "history event should decode");
        let Ok(event) = event else {
            return;
        };
        assert!(
            matches!(event, MobileServerEvent::History(_)),
            "expected history event"
        );
        let MobileServerEvent::History(payload) = event else {
            return;
        };
        assert_eq!(payload.session_id, "s1");
        assert_eq!(payload.provider_model.as_deref(), Some("gpt-5"));
        assert_eq!(payload.messages[0].content, "hi");
    }

    #[test]
    fn pairing_models_match_swift_sdk_shape() {
        let request = PairRequest {
            code: "123456".to_string(),
            device_id: "ios-test".to_string(),
            device_name: "simulator".to_string(),
            apns_token: None,
        };
        let value = serde_json::to_value(request);
        assert!(value.is_ok(), "pair request should serialize");
        let Ok(value) = value else {
            return;
        };
        assert_eq!(
            value,
            json!({"code":"123456","device_id":"ios-test","device_name":"simulator"})
        );
    }

    #[test]
    fn gateway_config_derives_http_and_websocket_endpoints() {
        let config = MobileGatewayConfig::new("https://devbox.tailnet.ts.net/", 7643, true);
        assert!(config.is_ok(), "gateway config should normalize host");
        let Ok(config) = config else {
            return;
        };
        assert_eq!(config.host, "devbox.tailnet.ts.net");
        let endpoints = config.endpoints();
        assert_eq!(
            endpoints.base_http_url,
            "https://devbox.tailnet.ts.net:7643"
        );
        assert_eq!(
            endpoints.health_url,
            "https://devbox.tailnet.ts.net:7643/health"
        );
        assert_eq!(
            endpoints.pair_url,
            "https://devbox.tailnet.ts.net:7643/pair"
        );
        assert_eq!(
            endpoints.websocket_url,
            "wss://devbox.tailnet.ts.net:7643/ws"
        );
    }

    #[test]
    fn gateway_config_auto_enables_tls_for_secure_schemes() {
        let config = MobileGatewayConfig::new("https://secure.example.com", 7643, false);
        assert!(config.is_ok());
        let config = config.unwrap();
        assert!(config.use_tls, "TLS should be enabled for https:// scheme");
        assert_eq!(config.host, "secure.example.com");

        let config = MobileGatewayConfig::new("wss://secure.example.com", 7643, false);
        assert!(config.is_ok());
        let config = config.unwrap();
        assert!(config.use_tls, "TLS should be enabled for wss:// scheme");
    }

    #[test]
    fn serialized_request_preserves_id_and_json_shape() {
        let request = MobileRequest::Ping { id: 42 };
        let serialized = serialize_mobile_request(&request);
        assert!(serialized.is_ok(), "request serializes");
        let Ok(serialized) = serialized else {
            return;
        };
        assert_eq!(serialized.id, 42);
        assert_eq!(serialized.json, r#"{"type":"ping","id":42}"#);
    }

    #[test]
    fn pairing_config_builds_pair_request() {
        let request = PairRequest::from(MobilePairingConfig {
            code: "654321".to_string(),
            device_id: "device-1".to_string(),
            device_name: "Linux simulator".to_string(),
            apns_token: Some("token".to_string()),
        });

        assert_eq!(request.code, "654321");
        assert_eq!(request.device_id, "device-1");
        assert_eq!(request.apns_token.as_deref(), Some("token"));
    }

    #[test]
    fn lossy_event_decoder_preserves_unknown_events() {
        let decoded = decode_mobile_server_event_lossy(json!({
            "type": "future_event",
            "payload": 123
        }));
        assert!(decoded.is_ok(), "unknown events are preserved");
        let Ok(decoded) = decoded else {
            return;
        };
        assert!(matches!(decoded, DecodedMobileServerEvent::Unknown(_)));
        let DecodedMobileServerEvent::Unknown(raw) = decoded else {
            return;
        };
        assert_eq!(raw.event_type, "future_event");
        assert_eq!(raw.raw["payload"], 123);
    }
}
