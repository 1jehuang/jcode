//! Agente perspectiva para debate multi-perspectiva
//!
//! Las perspectivas son stateless: NO pueden ejecutar tools directamente.
//! En su lugar,-envian ToolRequest al Coordinator y reciben ToolResult
//! para comentar sobre ellos.

use anyhow::Result;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};

use super::types::*;

/// Una perspectiva en el debate
///
/// Caracteristicas:
/// - NO tiene session activa
/// - NO puede ejecutar tools directamente
/// - PUEDE solicitar tool execution al Coordinator
/// - PUEDE comentar sobre tool results
pub struct PerspectiveAgent {
    /// Nombre de la perspectiva (e.g., "security", "performance")
    name: String,
    /// System prompt que define el rol
    system_prompt: String,
    /// Canal para comunicarse con el Coordinator
    coordinator_tx: Option<CoordinatorSender>,
    /// Canal para recibir tool results
    tool_result_rx: mpsc::UnboundedReceiver<ToolResult>,
    /// Tool results recibidos (historial)
    tool_results: Arc<RwLock<Vec<ToolResult>>>,
    /// Comentarios hechos sobre resultados
    comments: Arc<RwLock<Vec<PerspectiveComment>>>,
    /// Config de que tools puede solicitar
    allowed_tools: Option<Vec<String>>,
    /// Si recibe resultados de todos los tools
    receives_all_results: bool,
}

impl std::fmt::Debug for PerspectiveAgent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PerspectiveAgent")
            .field("name", &self.name)
            .field(
                "system_prompt",
                &format!(
                    "{}...",
                    &self.system_prompt[..self.system_prompt.len().min(50)]
                ),
            )
            .field("allowed_tools", &self.allowed_tools)
            .finish()
    }
}

impl PerspectiveAgent {
    /// Crear una nueva perspectiva
    pub fn new(
        name: impl Into<String>,
        system_prompt: impl Into<String>,
    ) -> (Self, mpsc::UnboundedSender<ToolResult>) {
        let (tx, rx) = mpsc::unbounded_channel();

        (
            Self {
                name: name.into(),
                system_prompt: system_prompt.into(),
                coordinator_tx: None,
                tool_result_rx: rx,
                tool_results: Arc::new(RwLock::new(Vec::new())),
                comments: Arc::new(RwLock::new(Vec::new())),
                allowed_tools: None,
                receives_all_results: true,
            },
            tx,
        )
    }

    /// Conectar al Coordinator
    pub fn connect_to_coordinator(&mut self, coordinator_tx: CoordinatorSender) {
        self.coordinator_tx = Some(coordinator_tx);
    }

    /// Configurar que tools puede solicitar esta perspectiva
    pub fn with_allowed_tools(mut self, tools: Vec<String>) -> Self {
        self.allowed_tools = Some(tools);
        self
    }

    /// Configurar si recibe todos los resultados o solo los suyos
    pub fn with_receives_all(self, receives_all: bool) -> Self {
        Self {
            receives_all_results: receives_all,
            ..self
        }
    }

    /// Nombre de la perspectiva
    pub fn name(&self) -> &str {
        &self.name
    }

    /// System prompt
    pub fn system_prompt(&self) -> &str {
        &self.system_prompt
    }

    /// Solicitar ejecucion de un tool al Coordinator
    ///
    /// Ejemplo:
    /// ```ignore
    /// perspective.request_tool("grep", json!({
    ///     "pattern": "api_key",
    ///     "path": "src/"
    /// })).await;
    /// ```
    pub async fn request_tool(
        &mut self,
        tool_name: &str,
        input: serde_json::Value,
    ) -> Result<String> {
        // Verificar si este tool esta permitido
        if let Some(ref allowed) = self.allowed_tools {
            if !allowed.contains(&tool_name.to_string()) {
                anyhow::bail!(
                    "Tool '{}' not allowed for perspective '{}'",
                    tool_name,
                    self.name
                );
            }
        }

        // Obtener canal al coordinator
        let tx = self
            .coordinator_tx
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Not connected to coordinator"))?;

        // Crear request
        let request = ToolRequest::new(self.name.clone(), tool_name, input);

        // Enviar al coordinator
        let request_id = request.request_id.clone();
        let msg = DebateMessage::ToolRequest { request };
        tx.send(msg)
            .map_err(|_| anyhow::anyhow!("Failed to send request to coordinator"))?;

        Ok(request_id)
    }

    /// Solicitar ejecucion con prioridad especifica
    pub async fn request_tool_priority(
        &mut self,
        tool_name: &str,
        input: serde_json::Value,
        priority: ToolRequestPriority,
    ) -> Result<String> {
        let request = ToolRequest::with_priority(self.name.clone(), tool_name, input, priority);
        let request_id = request.request_id.clone();

        let tx = self
            .coordinator_tx
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Not connected to coordinator"))?;

        let msg = DebateMessage::ToolRequest { request };
        tx.send(msg)
            .map_err(|_| anyhow::anyhow!("Failed to send request to coordinator"))?;

        Ok(request_id)
    }

    /// Procesar eventos del loop (tool results, etc)
    pub async fn poll(&mut self) {
        // Procesar tool results recibidos
        while let Ok(result) = self.tool_result_rx.try_recv() {
            self.handle_tool_result(result).await;
        }
    }

    /// Manejar un tool result recibido
    async fn handle_tool_result(&mut self, result: ToolResult) {
        if !self.receives_all_results && result.requesting_perspective != self.name {
            return;
        }

        // Guardar en historial
        {
            let mut results = self.tool_results.write().await;
            results.push(result.clone());
        }

        eprintln!(
            "[{}] received tool result: {} (error: {}, {}ms)",
            self.name, result.tool_name, result.is_error, result.execution_time_ms
        );

        // Generar comentario automatico?
        // (El commentary real viene del LLM de la perspectiva)
    }

    /// Obtener ultimo tool result para un tool especifico
    pub async fn last_result(&self, tool_name: &str) -> Option<ToolResult> {
        let results = self.tool_results.read().await;
        results
            .iter()
            .rev()
            .find(|r| r.tool_name == tool_name)
            .cloned()
    }

    /// Obtener todos los tool results
    pub async fn all_results(&self) -> Vec<ToolResult> {
        self.tool_results.read().await.clone()
    }

    /// Registrar un comentario sobre un resultado
    pub async fn add_comment(&mut self, request_id: &str, comment: String) {
        let mut comments = self.comments.write().await;
        comments.push(PerspectiveComment {
            perspective: self.name.clone(),
            request_id: request_id.to_string(),
            comment,
            timestamp_ms: current_timestamp_ms(),
        });
    }

    /// Obtener comentarios
    pub async fn get_comments(&self) -> Vec<PerspectiveComment> {
        self.comments.read().await.clone()
    }
}

/// Comentario hecho por una perspectiva sobre un resultado
#[derive(Debug, Clone)]
pub struct PerspectiveComment {
    pub perspective: String,
    pub request_id: String,
    pub comment: String,
    pub timestamp_ms: u64,
}

// =============================================================================
// Integracion con sistema de debate
// =============================================================================

/// Wrapper para crear perspectivas desde configuraciones
pub struct PerspectiveBuilder {
    config: PerspectiveConfig,
}

impl PerspectiveBuilder {
    pub fn new(name: impl Into<String>, system_prompt: impl Into<String>) -> Self {
        Self {
            config: PerspectiveConfig::new(name, system_prompt),
        }
    }

    pub fn with_tools(mut self, tools: Vec<String>) -> Self {
        self.config.allowed_tools = Some(tools);
        self
    }

    pub fn with_color(mut self, color: impl Into<String>) -> Self {
        self.config.color = Some(color.into());
        self
    }

    /// Solo recibe resultados de tools que ella misma solicita
    pub fn only_own_results(mut self) -> Self {
        self.config.receives_tool_results = false;
        self
    }

    pub fn build(self) -> PerspectiveAgent {
        let (mut agent, _result_rx) =
            PerspectiveAgent::new(self.config.name.clone(), self.config.system_prompt.clone());

        if let Some(tools) = self.config.allowed_tools {
            agent = agent.with_allowed_tools(tools);
        }

        if !self.config.receives_tool_results {
            agent = agent.with_receives_all(false);
        }

        agent
    }
}

// =============================================================================
// Ejemplo de flujo
// =============================================================================

/*
Ejemplo de flujo completo:

// 1. Crear Coordinator (con Agent activo)
let (agent, registry) = create_agent().await;
let coordinator = CoordinatorAgent::new(agent);

// 2. Crear perspectivas
let security = PerspectiveBuilder::new("security", "
    You are a security expert reviewing code changes.
    Look for: API keys, secrets, SQL injection, XSS, etc.
").with_tools(vec!["grep".to_string()]).build();

let perf = PerspectiveBuilder::new("performance", "
    You are a performance engineer.
    Look for: N+1 queries, missing indexes, expensive loops.
").with_tools(vec!["grep".to_string(), "glob".to_string()]).build();

// 3. Conectar perspectivas al coordinator
coordinator.register_perspective("security".to_string(), security.tx()).await;
coordinator.register_perspective("performance".to_string(), perf.tx()).await;

// 4. Flujo de debate
security.request_tool("grep", json!({
    "pattern": "api_key|secret|password",
    "path": "src/"
})).await;

// Coordinator recibe request, lo ejecuta
// Resultado: "Found api_key at src/config.py:42"

// Coordinator hace broadcast a TODAS las perspectivas
// Security recibe -> comenta sobre el hallazgo
// Perf recibe -> puede comentar sobre implicaciones de performance

// 5. Todas las perspectivas pueden comentar
for perspective in [security, perf] {
    perspective.add_comment(request_id, format!(
        "Found potential secret at {}:{}",
        file, line
    )).await;
}
*/

// =============================================================================
// Helpers
// =============================================================================

fn current_timestamp_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn only_own_results_ignores_results_from_other_perspectives() {
        let (mut agent, result_tx) =
            PerspectiveAgent::new("security", "Review the change for security issues");
        agent = agent.with_receives_all(false);

        result_tx
            .send(ToolResult::success(
                "req-other".to_string(),
                "grep".to_string(),
                "performance".to_string(),
                "other result".to_string(),
                10,
            ))
            .expect("send result");
        result_tx
            .send(ToolResult::success(
                "req-own".to_string(),
                "grep".to_string(),
                "security".to_string(),
                "own result".to_string(),
                10,
            ))
            .expect("send result");

        agent.poll().await;

        let results = agent.all_results().await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].request_id, "req-own");
    }
}
