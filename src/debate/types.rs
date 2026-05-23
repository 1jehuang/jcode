//! Tipos core para el sistema de debate multi-perspectiva

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::sync::mpsc;

/// Request de tool enviado por una perspectiva al coordinator
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRequest {
    /// ID unico del request
    pub request_id: String,
    /// Nombre de la perspectiva que solicita
    pub perspective_name: String,
    /// Nombre del tool a ejecutar
    pub tool_name: String,
    /// Input JSON para el tool
    pub input: serde_json::Value,
    /// Prioridad del request (alta = ejecutar primero)
    pub priority: ToolRequestPriority,
    /// Timestamp del request
    pub timestamp_ms: u64,
}

impl PartialEq for ToolRequest {
    fn eq(&self, other: &Self) -> bool {
        self.request_id == other.request_id
    }
}

impl Eq for ToolRequest {}

impl std::cmp::Ord for ToolRequest {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.priority
            .cmp(&other.priority)
            .then_with(|| other.timestamp_ms.cmp(&self.timestamp_ms))
            .then_with(|| self.request_id.cmp(&other.request_id))
    }
}

impl std::cmp::PartialOrd for ToolRequest {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl ToolRequest {
    pub fn new(
        perspective: impl Into<String>,
        tool_name: impl Into<String>,
        input: serde_json::Value,
    ) -> Self {
        Self {
            request_id: uuid_v4(),
            perspective_name: perspective.into(),
            tool_name: tool_name.into(),
            input,
            priority: ToolRequestPriority::Normal,
            timestamp_ms: current_timestamp_ms(),
        }
    }

    pub fn with_priority(
        perspective: impl Into<String>,
        tool_name: impl Into<String>,
        input: serde_json::Value,
        priority: ToolRequestPriority,
    ) -> Self {
        Self {
            request_id: uuid_v4(),
            perspective_name: perspective.into(),
            tool_name: tool_name.into(),
            input,
            priority,
            timestamp_ms: current_timestamp_ms(),
        }
    }
}

/// Prioridad del request de tool
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolRequestPriority {
    Low,
    Normal,
    High,
    Critical,
}

impl Default for ToolRequestPriority {
    fn default() -> Self {
        Self::Normal
    }
}

impl std::cmp::Ord for ToolRequestPriority {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let (a, b) = (self.weight(), other.weight());
        a.cmp(&b)
    }
}

impl std::cmp::PartialOrd for ToolRequestPriority {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl ToolRequestPriority {
    fn weight(&self) -> u8 {
        match self {
            Self::Low => 0,
            Self::Normal => 1,
            Self::High => 2,
            Self::Critical => 3,
        }
    }
}

/// Resultado de ejecutar un tool, enviado a todas las perspectivas
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// ID del request original
    pub request_id: String,
    /// Nombre del tool ejecutado
    pub tool_name: String,
    /// Perspectiva que solicito el tool
    pub requesting_perspective: String,
    /// Output del tool
    pub output: String,
    /// Si hubo error
    pub is_error: bool,
    /// Metadatos adicionales
    pub metadata: Option<serde_json::Value>,
    /// Tiempo de ejecucion en ms
    pub execution_time_ms: u64,
    /// Timestamp del resultado
    pub timestamp_ms: u64,
}

impl ToolResult {
    pub fn success(
        request_id: String,
        tool_name: String,
        perspective: String,
        output: String,
        execution_time_ms: u64,
    ) -> Self {
        Self {
            request_id,
            tool_name,
            requesting_perspective: perspective,
            output,
            is_error: false,
            metadata: None,
            execution_time_ms,
            timestamp_ms: current_timestamp_ms(),
        }
    }

    pub fn error(
        request_id: String,
        tool_name: String,
        perspective: String,
        error: String,
        execution_time_ms: u64,
    ) -> Self {
        Self {
            request_id,
            tool_name,
            requesting_perspective: perspective,
            output: error,
            is_error: true,
            metadata: None,
            execution_time_ms,
            timestamp_ms: current_timestamp_ms(),
        }
    }
}

/// Mensaje interno entre coordinator y perspectivas
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DebateMessage {
    /// Perspectiva solicita ejecucion de un tool
    ToolRequest { request: ToolRequest },
    /// Coordinator notifica resultado de tool a todas
    ToolResult { result: ToolResult },
    /// Perspectiva quiere agregar comentario sobre resultado
    PerspectiveComment {
        perspective: String,
        comment: String,
        request_id: String,
    },
    /// Perspectiva quiere iniciar debate sobre un tema
    StartDebate {
        topic: String,
        perspectives: Vec<String>,
    },
    /// Coordinator notifica fin de debate
    DebateEnded {
        summary: String,
        consensus: Option<String>,
    },
    /// Ping para mantener alive
    Ping { timestamp_ms: u64 },
    /// Pong respuesta
    Pong { timestamp_ms: u64 },
}

/// Canal para enviar mensajes al coordinator
pub type CoordinatorSender = mpsc::UnboundedSender<DebateMessage>;

/// Canal para recibir mensajes del coordinator
pub type CoordinatorReceiver = mpsc::UnboundedReceiver<DebateMessage>;

/// Configuracion de una perspectiva en el debate
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerspectiveConfig {
    /// Nombre unico de la perspectiva
    pub name: String,
    /// System prompt que define el rol/perspectiva
    pub system_prompt: String,
    /// Si esta perspectiva puede solicitar tools
    pub can_request_tools: bool,
    /// Si esta perspectiva recibe resultados de tools
    pub receives_tool_results: bool,
    /// Tools que esta perspectiva puede solicitar
    pub allowed_tools: Option<Vec<String>>,
    /// Color/etiqueta visual para esta perspectiva
    pub color: Option<String>,
}

impl PerspectiveConfig {
    pub fn new(name: impl Into<String>, system_prompt: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            system_prompt: system_prompt.into(),
            can_request_tools: true,
            receives_tool_results: true,
            allowed_tools: None,
            color: None,
        }
    }
}

// =============================================================================
// Helpers
// =============================================================================

fn uuid_v4() -> String {
    use std::time::SystemTime;
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let pid = std::process::id();
    format!(
        "{:x}-{:x}-4-{:x}-{:x}",
        now.as_secs(),
        now.subsec_nanos(),
        pid,
        rand_u64()
    )
}

fn rand_u64() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::SeqCst)
}

fn current_timestamp_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Helper para construir un ToolContext valido
pub fn build_tool_context(
    session_id: &str,
    message_id: &str,
    working_dir: Option<PathBuf>,
    tool_call_id: String,
) -> jcode_tool_core::ToolContext {
    use jcode_tool_core::ToolExecutionMode;

    jcode_tool_core::ToolContext {
        session_id: session_id.to_string(),
        message_id: message_id.to_string(),
        tool_call_id,
        working_dir,
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: ToolExecutionMode::Direct,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_tool_request_creation() {
        let request = ToolRequest::new("security", "grep", json!({"pattern": "api_key"}));
        assert_eq!(request.perspective_name, "security");
        assert_eq!(request.tool_name, "grep");
        assert!(!request.request_id.is_empty());
    }

    #[test]
    fn test_tool_result_success() {
        let result = ToolResult::success(
            "req-1".to_string(),
            "grep".to_string(),
            "security".to_string(),
            "Found api_key at line 42".to_string(),
            150,
        );
        assert!(!result.is_error);
        assert!(result.output.contains("line 42"));
    }

    #[test]
    fn test_tool_result_error() {
        let result = ToolResult::error(
            "req-1".to_string(),
            "grep".to_string(),
            "security".to_string(),
            "Pattern not found".to_string(),
            50,
        );
        assert!(result.is_error);
    }

    #[test]
    fn test_priority_ordering() {
        assert!(ToolRequestPriority::Critical > ToolRequestPriority::High);
        assert!(ToolRequestPriority::High > ToolRequestPriority::Normal);
        assert!(ToolRequestPriority::Normal > ToolRequestPriority::Low);
    }

    #[test]
    fn test_tool_request_ordering_prefers_priority_then_fifo() {
        let mut low = ToolRequest::new("security", "grep", json!({}));
        low.priority = ToolRequestPriority::Low;
        low.timestamp_ms = 10;

        let mut high_old = ToolRequest::new("security", "grep", json!({}));
        high_old.priority = ToolRequestPriority::High;
        high_old.timestamp_ms = 20;

        let mut high_new = ToolRequest::new("security", "grep", json!({}));
        high_new.priority = ToolRequestPriority::High;
        high_new.timestamp_ms = 30;

        let mut heap = std::collections::BinaryHeap::new();
        heap.push(low);
        heap.push(high_new);
        heap.push(high_old.clone());

        assert_eq!(heap.pop().unwrap().request_id, high_old.request_id);
        assert_eq!(heap.pop().unwrap().priority, ToolRequestPriority::High);
        assert_eq!(heap.pop().unwrap().priority, ToolRequestPriority::Low);
    }
}
