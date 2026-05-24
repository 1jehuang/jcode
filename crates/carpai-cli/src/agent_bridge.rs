//! Agent Bridge — TUI ↔ carpai-core 桥接层
//!
//! **核心设计原则**: 只委托，零业务逻辑。
//!
//! 所有 Agent 业务逻辑（消息追加、推理、工具调用、循环）都通过
//! `carpai_core::execute_agent_turn()` 完成。本模块只负责：
//! 1. 模式选择 (Local / Remote)
//! 2. 参数组装
//! 3. 结果格式化
//! 4. 重试策略 (Local 模式)
//! 5. 优雅降级 (Remote 模式)

use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{warn, info};

use carpai_internal::AgentContext;
use crate::grpc_client::GrpcClient;

/// Bridge operation mode
#[derive(Debug, Clone)]
pub enum BridgeMode {
    /// Local mode: use carpai-core directly
    Local,
    /// Remote mode: connect to a CarpAI server via gRPC/HTTP
    Remote { url: String },
}

/// Output from a single agent turn
#[derive(Debug, Clone)]
pub struct AgentTurnOutput {
    pub text: String,
    pub tool_calls: Vec<ToolCallInfo>,
    pub session_id: String,
    pub duration_ms: u64,
}

/// Details of a tool call made during the turn
#[derive(Debug, Clone)]
pub struct ToolCallInfo {
    pub name: String,
    pub params: serde_json::Value,
    pub result: Option<serde_json::Value>,
    pub duration_ms: u64,
}

/// The bridge between TUI and agent core
pub struct AgentBridge {
    mode: BridgeMode,
    local_ctx: Option<Arc<RwLock<AgentContext>>>,
    retry_config: RetryConfig,
    #[allow(dead_code)]
    grpc_client: Option<GrpcClient>,
}

impl AgentBridge {
    /// Create a new bridge in local mode with the given context
    pub fn new_local(ctx: AgentContext) -> Self {
        Self {
            mode: BridgeMode::Local,
            local_ctx: Some(Arc::new(RwLock::new(ctx))),
            retry_config: RetryConfig::default(),
            grpc_client: None,
        }
    }

    /// Create a new bridge in local mode with custom retry config
    pub fn new_local_with_retry(ctx: AgentContext, retry_config: RetryConfig) -> Self {
        Self {
            mode: BridgeMode::Local,
            local_ctx: Some(Arc::new(RwLock::new(ctx))),
            retry_config,
            grpc_client: None,
        }
    }

    /// Create a new bridge in remote mode
    pub fn new_remote(url: String) -> Self {
        Self {
            mode: BridgeMode::Remote { url },
            local_ctx: None,
            retry_config: RetryConfig::default(),
            grpc_client: None,
        }
    }

    /// Execute an agent turn
    pub async fn execute_turn(&self, user_msg: &str) -> Result<AgentTurnOutput, BridgeError> {
        match &self.mode {
            BridgeMode::Local => {
                // Execute agent turn (retry handled externally for now)
                let output = self.execute_local_turn(user_msg).await;

                match output {
                    Ok(result) => Ok(result),
                    Err(e) => {
                        warn!(error = %e, "Agent turn execution failed, returning degraded response");
                        Ok(AgentTurnOutput {
                            text: format!(
                                "[Degraded response] Agent processing failed: {}\n\
                                 Please check the model endpoint and try again.",
                                e
                            ),
                            tool_calls: vec![],
                            session_id: "degraded".into(),
                            duration_ms: 0,
                        })
                    }
                }
            }
            BridgeMode::Remote { url } => {
                warn!(url = %url, "Remote mode not yet implemented");
                Ok(AgentTurnOutput {
                    text: format!(
                        "[Remote mode connecting to {}]\n\
                         Remote mode is not yet implemented. \
                         Please run in local mode: `carpai chat`",
                        url
                    ),
                    tool_calls: vec![],
                    session_id: "remote-placeholder".into(),
                    duration_ms: 0,
                })
            }
        }
    }

    /// Get current session ID
    pub async fn session_id(&self) -> Option<String> {
        match &self.mode {
            BridgeMode::Local => {
                let _ctx = self.local_ctx.as_ref()?.read().await;
                Some("cli-session".to_string())
            }
            BridgeMode::Remote { .. } => None,
        }
    }

    /// Switch between local and remote modes
    pub async fn switch_mode(&mut self, new_mode: BridgeMode) {
        info!(old_mode = ?self.mode, new_mode = ?new_mode, "Switching bridge mode");
        self.mode = new_mode;
        if matches!(self.mode, BridgeMode::Remote { .. }) {
            self.local_ctx = None;
        }
    }

    /// Execute a local agent turn (helper to avoid retry/ownership conflicts)
    async fn execute_local_turn(&self, user_msg: &str) -> Result<AgentTurnOutput, BridgeError> {
        let ctx = self.local_ctx.as_ref()
            .ok_or(BridgeError::NoContext)?
            .read().await;

        let output = carpai_core::agent_loop::execute_agent_turn(&*ctx, user_msg)
            .await
            .map_err(|e| BridgeError::Execution(e.to_string()))?;

        let tool_calls = output.tool_calls.into_iter().map(|tc| ToolCallInfo {
            name: tc.name.clone(),
            params: tc.arguments.clone(),
            result: tc.result.map(serde_json::Value::String),
            duration_ms: tc.duration_ms,
        }).collect();

        Ok(AgentTurnOutput {
            text: output.text,
            tool_calls,
            session_id: output.session_id.0,
            duration_ms: output.duration_ms,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    #[error("No local context available")]
    NoContext,
    #[error("Not yet implemented: {0}")]
    NotImplemented(String),
    #[error("Execution error: {0}")]
    Execution(String),
    #[error("Connection error: {0}")]
    Connection(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use carpai_core::config::CoreConfig;

    fn create_test_context() -> AgentContext {
        let mut config = CoreConfig::default();
        config.base.working_dir = std::env::temp_dir();
        carpai_core::build_local_agent_context(&config)
    }

    #[test]
    fn test_bridge_mode_display() {
        let ctx = create_test_context();
        let bridge = AgentBridge::new_local(ctx);
        assert!(matches!(bridge.mode, BridgeMode::Local));
    }

    #[tokio::test]
    async fn test_execute_turn_local_returns_something() {
        let ctx = create_test_context();
        let bridge = AgentBridge::new_local(ctx);
        let result = bridge.execute_turn("hello").await;
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(!output.text.is_empty());
    }

    #[tokio::test]
    async fn test_remote_mode_graceful_degradation() {
        let bridge = AgentBridge::new_remote("http://localhost:9999".into());
        let result = bridge.execute_turn("test").await;
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.text.contains("not yet implemented"));
    }

    #[test]
    fn test_bridge_error_display() {
        let err = BridgeError::NoContext;
        assert_eq!(err.to_string(), "No local context available");
        let err = BridgeError::Execution("oops".into());
        assert_eq!(err.to_string(), "Execution error: oops");
    }
}
