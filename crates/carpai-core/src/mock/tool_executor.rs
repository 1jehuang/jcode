use std::sync::Arc;
use async_trait::async_trait;
use tokio::sync::RwLock;
use carpai_internal::*;

pub struct MockToolExecutor {
    calls: Arc<RwLock<Vec<String>>>,
}

impl Default for MockToolExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl MockToolExecutor {
    pub fn new() -> Self {
        Self {
            calls: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub async fn recorded_calls(&self) -> Vec<String> {
        self.calls.read().await.clone()
    }
}

#[async_trait]
impl ToolExecutor for MockToolExecutor {
    async fn execute(
        &self,
        request: ToolRequest,
    ) -> Result<ToolResponse, ToolExecError> {
        self.calls.write().await.push(request.tool_name.clone());
        Ok(ToolResponse {
            success: true,
            output: serde_json::json!({ "mock": true }).to_string(),
            data: Some(serde_json::json!({ "mock": true })),
            exit_code: None,
            duration_ms: 0,
            request_id: request.request_id.clone(),
            tool_name: request.tool_name.clone(),
            audit_id: None,
        })
    }

    async fn list_tools(&self) -> Result<Vec<ToolSchema>, ToolExecError> {
        Ok(vec![])
    }

    async fn get_tool_schema(&self, _name: &str) -> Result<Option<ToolSchema>, ToolExecError> {
        Ok(None)
    }

    async fn validate(
        &self,
        _name: &str,
        _params: &serde_json::Value,
    ) -> Result<ValidationResult, ToolExecError> {
        Ok(ValidationResult {
            valid: true,
            error: None,
            warnings: vec![],
        })
    }

    async fn check_permission(
        &self,
        _user_id: &str,
        _tool_name: &str,
    ) -> Result<bool, ToolExecError> {
        Ok(true)
    }

    async fn cancel(&self, _request_id: &str) -> Result<(), ToolExecError> {
        Ok(())
    }
}
