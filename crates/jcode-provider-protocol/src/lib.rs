use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PROTOCOL_VERSION: &str = "1.0";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Frame {
    Hello { protocol_version: String, client: String, capabilities: Vec<String> },
    HelloOk { protocol_version: String, provider: ProviderInfo, capabilities: Vec<String> },
    Request { protocol_version: String, id: String, method: String, params: Value },
    Response { protocol_version: String, id: String, ok: bool, #[serde(default, skip_serializing_if = "Option::is_none")] result: Option<Value>, #[serde(default, skip_serializing_if = "Option::is_none")] error: Option<WireError> },
    Event { protocol_version: String, request_id: String, event: String, payload: Value },
    Cancel { protocol_version: String, request_id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProviderInfo { pub id: String, pub name: String, pub version: String }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WireError { pub code: String, pub message: String, #[serde(default)] pub retryable: bool, #[serde(default)] pub details: Value }

#[derive(Debug, thiserror::Error)]
pub enum ProtocolError { #[error("malformed frame: {0}")] Malformed(#[from] serde_json::Error), #[error("unsupported protocol version: {0}")] Version(String), #[error("frame exceeds maximum size")] TooLarge, #[error("unknown capability: {0}")] UnknownCapability(String) }

pub fn encode(frame: &Frame) -> Result<Vec<u8>, ProtocolError> { let mut bytes = serde_json::to_vec(frame)?; bytes.push(b'\n'); Ok(bytes) }
pub fn decode(bytes: &[u8], max_size: usize) -> Result<Frame, ProtocolError> { if bytes.len() > max_size { return Err(ProtocolError::TooLarge); } let frame: Frame = serde_json::from_slice(bytes)?; let version = match &frame { Frame::Hello { protocol_version, .. } | Frame::HelloOk { protocol_version, .. } | Frame::Request { protocol_version, .. } | Frame::Response { protocol_version, .. } | Frame::Event { protocol_version, .. } | Frame::Cancel { protocol_version, .. } => protocol_version }; if version != PROTOCOL_VERSION { return Err(ProtocolError::Version(version.clone())); } Ok(frame) }

#[cfg(test)]
mod tests { use super::*; #[test] fn handshake_roundtrip() { let f=Frame::Hello{protocol_version:PROTOCOL_VERSION.into(),client:"jcode".into(),capabilities:vec!["streaming".into()]}; assert_eq!(decode(&encode(&f).unwrap(),1024).unwrap(),f); } #[test] fn malformed() { assert!(matches!(decode(b"{",100),Err(ProtocolError::Malformed(_)))); } #[test] fn unknown_capability_is_explicit() { let e=ProtocolError::UnknownCapability("x".into()); assert_eq!(e.to_string(),"unknown capability: x"); } #[test] fn large_payload() { let f=Frame::Event{protocol_version:PROTOCOL_VERSION.into(),request_id:"r".into(),event:"chunk".into(),payload:Value::String("x".repeat(4096))}; assert!(matches!(decode(&encode(&f).unwrap(),100),Err(ProtocolError::TooLarge))); } #[test] fn tool_correlation_and_cancel() { let f=Frame::Event{protocol_version:PROTOCOL_VERSION.into(),request_id:"tool-1".into(),event:"tool_result".into(),payload:serde_json::json!({"call_id":"call-1"})}; assert_eq!(decode(&encode(&f).unwrap(),4096).unwrap(),f); let c=Frame::Cancel{protocol_version:PROTOCOL_VERSION.into(),request_id:"tool-1".into()}; assert_eq!(decode(&encode(&c).unwrap(),4096).unwrap(),c); } }
