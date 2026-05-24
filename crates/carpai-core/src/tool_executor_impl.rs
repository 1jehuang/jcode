use std::collections::HashMap;
use std::sync::Arc;
#[allow(dead_code)]
use async_trait::async_trait;
use tokio::sync::{Semaphore, Mutex};
#[allow(dead_code)]
use carpai_internal::tool_executor::*;
use tracing::{info, debug, warn};

pub struct LocalToolExecutor {
    max_concurrent: Arc<Semaphore>,
    registry: Arc<Mutex<HashMap<String, ToolSchema>>>,
}

impl LocalToolExecutor {
    pub fn new(max_concurrent: usize) -> Self {
        Self {
            max_concurrent: Arc::new(Semaphore::new(max_concurrent)),
            registry: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn register_tool(&self, name: String, schema: ToolSchema) {
        let mut reg = self.registry.lock().await;
        reg.insert(name.clone(), schema);
        info!(tool = %name, "Tool registered");
    }

    async fn execute_local(
        &self,
        tool_name: &str,
        parameters: &serde_json::Value,
    ) -> Result<String, ToolExecError> {
        info!(
            tool = %tool_name,
            params = %serde_json::to_string(parameters).unwrap_or_default(),
            "Executing local tool (stub)"
        );

        Ok(format!(
            "[STUB] Tool '{}' executed with params: {}",
            tool_name,
            serde_json::to_string(parameters).unwrap_or_default()
        ))
    }
}

#[async_trait]
impl ToolExecutor for LocalToolExecutor {
    async fn execute(
        &self,
        request: ToolRequest,
    ) -> Result<ToolResponse, ToolExecError> {
        let start_time = std::time::Instant::now();

        let _permit = self.max_concurrent.acquire().await.map_err(|_| {
            ToolExecError::Internal(anyhow::anyhow!("Semaphore closed"))
        })?;

        info!(
            tool = %request.tool_name,
            request_id = %request.request_id,
            mode = ?request.mode_override,
            user_id = %request.context.user_id,
            "Executing tool"
        );

        let mode = request.mode_override.as_ref().unwrap_or(&ExecutionMode::Local);

        match mode {
            ExecutionMode::Local => {
                let output = self.execute_local(&request.tool_name, &request.parameters).await?;

                let duration_ms = start_time.elapsed().as_millis() as u64;

                Ok(ToolResponse {
                    success: true,
                    output,
                    data: None,
                    exit_code: Some(0),
                    duration_ms,
                    request_id: request.request_id.clone(),
                    tool_name: request.tool_name.clone(),
                    audit_id: None,
                })
            }
            ExecutionMode::Sandboxed => {
                Err(ToolExecError::ExecutionFailed(
                    format!("Sandbox execution not yet implemented in LocalToolExecutor: {}", request.tool_name),
                ))
            }
            ExecutionMode::Remote { endpoint } => {
                Err(ToolExecError::ExecutionFailed(
                    format!("{}: Remote execution not supported by LocalToolExecutor (endpoint: {})", request.tool_name, endpoint),
                ))
            }
            ExecutionMode::DryRun => {
                let duration_ms = start_time.elapsed().as_millis() as u64;

                Ok(ToolResponse {
                    success: true,
                    output: "[DRY RUN] Validation passed".to_string(),
                    data: Some(request.parameters.clone()),
                    exit_code: Some(0),
                    duration_ms,
                    request_id: request.request_id.clone(),
                    tool_name: request.tool_name.clone(),
                    audit_id: None,
                })
            }
        }
    }

    async fn list_tools(&self) -> Result<Vec<ToolSchema>, ToolExecError> {
        let reg = self.registry.lock().await;
        Ok(reg.values().cloned().collect())
    }

    async fn get_tool_schema(&self, name: &str) -> Result<Option<ToolSchema>, ToolExecError> {
        let reg = self.registry.lock().await;
        Ok(reg.get(name).cloned())
    }

    async fn validate(
        &self,
        name: &str,
        params: &serde_json::Value,
    ) -> Result<ValidationResult, ToolExecError> {
        let reg = self.registry.lock().await;

        if let Some(schema) = reg.get(name) {
            let mut warnings = Vec::new();

            if let Some(required) = schema.parameters_json_schema.get("required") {
                if let Some(required_arr) = required.as_array() {
                    for field in required_arr {
                        if let Some(field_name) = field.as_str() {
                            if !params.get(field_name).is_some() {
                                return Ok(ValidationResult {
                                    valid: false,
                                    error: Some(format!("Missing required parameter: {}", field_name)),
                                    warnings: vec![],
                                });
                            }
                        }
                    }
                }
            }

            if params.as_object().map_or(false, |obj| obj.is_empty()) {
                warnings.push("Empty parameters object".to_string());
            }

            Ok(ValidationResult {
                valid: true,
                error: None,
                warnings,
            })
        } else {
            warn!(tool = %name, "Tool not found in registry, allowing validation");
            Ok(ValidationResult {
                valid: true,
                error: None,
                warnings: vec![format!("Tool '{}' not registered", name)],
            })
        }
    }

    async fn check_permission(
        &self,
        _user_id: &str,
        _tool_name: &str,
    ) -> Result<bool, ToolExecError> {
        debug!(
            user = %_user_id,
            tool = %_tool_name,
            "Checking permission (default allow)"
        );
        Ok(true)
    }

    async fn cancel(&self, _request_id: &str) -> Result<(), ToolExecError> {
        warn!(
            request_id = %_request_id,
            "Cancel requested but not implemented in LocalToolExecutor"
        );
        Err(ToolExecError::Cancelled)
    }
}
