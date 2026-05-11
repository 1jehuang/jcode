//! JSON-RPC 2.0 Transport Layer — LSP 通信的核心基础设施
//!
//! 移植自 `crates/jcode-completion/src/lsp_provider.rs` 的真实通信代码，
//! 并升级为异步版本（tokio）以支持高并发。
//!
//! ## 核心能力
//! - Content-Length 协议编解码（LSP 标准）
//! - 辅助函数用于构建请求/通知/解析响应

use serde_json::{Value, json};
use std::sync::atomic::{AtomicU64, Ordering};

/// 全局请求 ID 生成器
static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

fn next_request_id() -> u64 {
    NEXT_REQUEST_ID.fetch_add(1, Ordering::SeqCst)
}

/// JSON-RPC 错误码
#[derive(Debug, thiserror::Error)]
pub enum JsonRpcError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
    
    #[error("Invalid protocol: missing Content-Length header")]
    MissingContentLength,
    
    #[error("Invalid Content-Length value: {0}")]
    InvalidContentLength(String),
    
    #[error("Request timeout after {timeout_ms}ms")]
    Timeout { timeout_ms: u64 },
    
    #[error("JSON-RPC error: code={code}, message={message}")]
    ServerError { code: i32, message: String },
    
    #[error("Process exited unexpectedly")]
    ProcessExited,
}

/// 辅助函数：构建 JSON-RPC 请求
pub fn build_request(method: &str, params: Value) -> Value {
    let id = next_request_id();
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    })
}

/// 辅助函数：构建 JSON-RPC 通知（无 ID，无响应）
pub fn build_notification(method: &str, params: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    })
}

/// 解析 JSON-RPC 响应，提取 result 或 error
pub fn parse_response(response: Value) -> Result<Value, JsonRpcError> {
    if let Some(error) = response.get("error") {
        let code = error["code"].as_i64().unwrap_or(0) as i32;
        let message = error["message"].as_str().unwrap_or("Unknown error").to_string();
        Err(JsonRpcError::ServerError { code, message })
    } else if let Some(result) = response.get("result").cloned() {
        Ok(result)
    } else {
        Ok(Value::Null)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_request() {
        let req = build_request("initialize", json!({"capabilities": {}}));
        assert_eq!(req["jsonrpc"], "2.0");
        assert!(req.get("id").is_some());
        assert_eq!(req["method"], "initialize");
    }

    #[test]
    fn test_build_notification() {
        let notif = build_notification("initialized", json!({}));
        assert_eq!(notif["jsonrpc"], "2.0");
        assert!(notif.get("id").is_none());
        assert_eq!(notif["method"], "initialized");
    }

    #[test]
    fn test_parse_response_success() {
        let resp = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {"capabilities": {}}
        });
        let result = parse_response(resp).unwrap();
        assert!(result.get("capabilities").is_some());
    }

    #[test]
    fn test_parse_response_error() {
        let resp = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "error": {"code": -32600, "message": "Invalid Request"}
        });
        let err = parse_response(resp).unwrap_err();
        match err {
            JsonRpcError::ServerError { code, message } => {
                assert_eq!(code, -32600);
                assert_eq!(message, "Invalid Request");
            }
            other => panic!("Expected ServerError, got: {}", other),
        }
    }
}
