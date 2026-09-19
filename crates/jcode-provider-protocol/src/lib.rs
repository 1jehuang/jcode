use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Wire protocol version. This is independent from jcode's application version.
pub const PROTOCOL_VERSION: &str = "0.1";
pub const DEFAULT_MAX_FRAME_SIZE: usize = 16 * 1024 * 1024;
pub const CAP_STREAMING: &str = "streaming";
pub const CAP_NATIVE_TOOLS: &str = "native_tools";
pub const CAP_CANCELLATION: &str = "cancellation";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Frame {
    Hello {
        protocol_version: String,
        client: String,
        #[serde(default)]
        capabilities: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_frame_size: Option<usize>,
    },
    HelloOk {
        protocol_version: String,
        provider: ProviderInfo,
        #[serde(default)]
        capabilities: Vec<String>,
    },
    Request {
        protocol_version: String,
        id: String,
        method: String,
        params: Value,
    },
    Response {
        protocol_version: String,
        id: String,
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<WireError>,
    },
    Event {
        protocol_version: String,
        request_id: String,
        event: String,
        payload: Value,
    },
    Cancel {
        protocol_version: String,
        request_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProviderInfo {
    pub id: String,
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WireError {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub retryable: bool,
    #[serde(default)]
    pub details: Value,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ProtocolError {
    #[error("malformed frame: {0}")]
    Malformed(String),
    #[error("unsupported protocol version: {0}")]
    Version(String),
    #[error("frame exceeds maximum size")]
    TooLarge,
    #[error("unknown capability: {0}")]
    UnknownCapability(String),
}

impl From<serde_json::Error> for ProtocolError {
    fn from(error: serde_json::Error) -> Self {
        Self::Malformed(error.to_string())
    }
}

pub fn encode(frame: &Frame) -> Result<Vec<u8>, ProtocolError> {
    let mut bytes = serde_json::to_vec(frame)?;
    bytes.push(b'\n');
    Ok(bytes)
}

pub fn decode(bytes: &[u8], max_size: usize) -> Result<Frame, ProtocolError> {
    if bytes.len() > max_size {
        return Err(ProtocolError::TooLarge);
    }
    let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
    let frame: Frame = serde_json::from_slice(bytes)?;
    let version = frame.protocol_version();
    if version != PROTOCOL_VERSION {
        return Err(ProtocolError::Version(version.to_string()));
    }
    Ok(frame)
}

impl Frame {
    pub fn protocol_version(&self) -> &str {
        match self {
            Self::Hello {
                protocol_version, ..
            }
            | Self::HelloOk {
                protocol_version, ..
            }
            | Self::Request {
                protocol_version, ..
            }
            | Self::Response {
                protocol_version, ..
            }
            | Self::Event {
                protocol_version, ..
            }
            | Self::Cancel {
                protocol_version, ..
            } => protocol_version,
        }
    }
}

/// Return the capabilities supported by both sides, preserving the client's order.
/// Unknown capabilities are ignored so newer providers remain interoperable with
/// older clients. `UnknownCapability` remains available for strict callers.
pub fn negotiate_capabilities(requested: &[String], offered: &[String]) -> Vec<String> {
    requested
        .iter()
        .filter(|capability| offered.iter().any(|offered| offered == *capability))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version() -> String {
        PROTOCOL_VERSION.to_string()
    }

    #[test]
    fn handshake_roundtrip() {
        let frame = Frame::Hello {
            protocol_version: version(),
            client: "jcode".into(),
            capabilities: vec![CAP_STREAMING.into(), CAP_NATIVE_TOOLS.into()],
            max_frame_size: Some(1024),
        };
        assert_eq!(decode(&encode(&frame).unwrap(), 4096).unwrap(), frame);
    }

    #[test]
    fn all_frames_roundtrip() {
        let frames = vec![
            Frame::HelloOk {
                protocol_version: version(),
                provider: ProviderInfo {
                    id: "p".into(),
                    name: "Provider".into(),
                    version: "1".into(),
                },
                capabilities: vec![CAP_CANCELLATION.into()],
            },
            Frame::Request {
                protocol_version: version(),
                id: "r".into(),
                method: "complete".into(),
                params: serde_json::json!({"prompt":"hello"}),
            },
            Frame::Response {
                protocol_version: version(),
                id: "r".into(),
                ok: false,
                result: None,
                error: Some(WireError {
                    code: "unavailable".into(),
                    message: "busy".into(),
                    retryable: true,
                    details: serde_json::json!({"retry_after_ms": 10}),
                }),
            },
            Frame::Event {
                protocol_version: version(),
                request_id: "r".into(),
                event: "native_tool_call".into(),
                payload: serde_json::json!({"call_id":"call-1","name":"read"}),
            },
            Frame::Cancel {
                protocol_version: version(),
                request_id: "r".into(),
            },
        ];
        for frame in frames {
            assert_eq!(decode(&encode(&frame).unwrap(), 4096).unwrap(), frame);
        }
    }

    #[test]
    fn malformed_and_wrong_version_are_rejected() {
        assert!(matches!(
            decode(b"{", 100),
            Err(ProtocolError::Malformed(_))
        ));
        let frame = br#"{"kind":"cancel","protocol_version":"9.0","request_id":"r"}"#;
        assert_eq!(
            decode(frame, 100),
            Err(ProtocolError::Version("9.0".into()))
        );
    }

    #[test]
    fn size_limit_and_crlf_are_handled() {
        let frame = Frame::Event {
            protocol_version: version(),
            request_id: "r".into(),
            event: "chunk".into(),
            payload: Value::String("x".repeat(4096)),
        };
        assert_eq!(
            decode(&encode(&frame).unwrap(), 100),
            Err(ProtocolError::TooLarge)
        );
        let mut encoded = encode(&frame).unwrap();
        encoded.insert(encoded.len() - 1, b'\r');
        assert_eq!(decode(&encoded, 8192).unwrap(), frame);
    }

    #[test]
    fn capability_negotiation_is_deterministic() {
        let requested = vec![
            CAP_NATIVE_TOOLS.into(),
            CAP_STREAMING.into(),
            "future".into(),
        ];
        let offered = vec![
            CAP_STREAMING.into(),
            CAP_CANCELLATION.into(),
            CAP_NATIVE_TOOLS.into(),
        ];
        assert_eq!(
            negotiate_capabilities(&requested, &offered),
            vec![CAP_NATIVE_TOOLS, CAP_STREAMING]
        );
    }
}
