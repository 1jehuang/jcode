//! Coordinator para debate multi-perspectiva
//!
//! Responsabilidades:
//! 1. Recibir tool requests de las perspectivas
//! 2. Ejecutar tools con ToolContext valido (tiene session activa)
//! 3. Distribuir resultados a TODAS las perspectivas
//! 4. Mantener el estado del debate

use crate::tool::{Registry, ToolContext};
use anyhow::Result;
use std::collections::{BinaryHeap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};

use super::types::*;

/// El Coordinator es el unico que tiene una session activa,
/// por lo tanto es el unico que puede ejecutar tools.
pub struct DebateCoordinator {
    /// Session ID del coordinator (para ToolContext)
    session_id: String,
    /// Mensaje ID actual
    message_id: String,
    /// Directorio de trabajo
    working_dir: Option<PathBuf>,
    /// Registry de tools (compartido)
    registry: Registry,
    /// Cola de requests de tools (prioridad)
    tool_request_queue: Arc<RwLock<BinaryHeap<ToolRequest>>>,
    /// Resultados recientes (para historico)
    recent_results: Arc<RwLock<Vec<ToolResult>>>,
    /// Canal para recibir mensajes de perspectivas
    inbox_rx: mpsc::UnboundedReceiver<DebateMessage>,
    /// Canales para enviar a perspectivas especificas
    perspective_senders: Arc<RwLock<HashMap<String, CoordinatorSender>>>,
    /// Broadcast channel para tool results
    tool_result_broadcast: Arc<RwLock<Vec<mpsc::UnboundedSender<ToolResult>>>>,
}

impl DebateCoordinator {
    /// Crear un nuevo coordinator
    pub async fn new(
        session_id: String,
        working_dir: Option<PathBuf>,
        registry: Registry,
    ) -> (Self, CoordinatorSender) {
        let (tx, rx) = mpsc::unbounded_channel();

        let coordinator = Self {
            session_id,
            message_id: uuid_v4(),
            working_dir,
            registry,
            tool_request_queue: Arc::new(RwLock::new(BinaryHeap::new())),
            recent_results: Arc::new(RwLock::new(Vec::new())),
            inbox_rx: rx,
            perspective_senders: Arc::new(RwLock::new(HashMap::new())),
            tool_result_broadcast: Arc::new(RwLock::new(Vec::new())),
        };

        (coordinator, tx)
    }

    /// Registrar una perspectiva
    pub async fn register_perspective(&self, name: String, tx: CoordinatorSender) -> Result<()> {
        // Registrar canal de comunicacion
        {
            let mut senders = self.perspective_senders.write().await;
            senders.insert(name.clone(), tx);
        }

        // Registrar para notificaciones de tool results
        {
            let mut broadcast = self.tool_result_broadcast.write().await;
            let (result_tx, _) = mpsc::unbounded_channel::<ToolResult>();
            broadcast.push(result_tx);
        }

        Ok(())
    }

    /// Loop principal del coordinator
    pub async fn run(&mut self) {
        loop {
            tokio::select! {
                // Procesar mensajes entrantes
                Some(msg) = self.inbox_rx.recv() => {
                    match msg {
                        DebateMessage::ToolRequest { request } => {
                            self.queue_tool_request(request).await;
                        }
                        DebateMessage::PerspectiveComment { .. } => {
                            // Comentarios se manejan en el loop de perspectiva
                        }
                        DebateMessage::StartDebate { .. } => {
                            // Iniciar debate - procesar requests en cola
                            self.process_tool_queue().await;
                        }
                        DebateMessage::Ping { timestamp_ms: _ } => {
                            // Responder pong
                            eprintln!("[coordinator] ping received, responding");
                        }
                        DebateMessage::DebateEnded { .. } => {
                            break;
                        }
                        _ => {}
                    }
                }
                // Procesar tool requests en cola
                _ = tokio::time::sleep(tokio::time::Duration::from_millis(10)) => {
                    self.process_tool_queue().await;
                }
            }
        }
    }

    /// Agregar un tool request a la cola (prioridad)
    async fn queue_tool_request(&self, request: ToolRequest) {
        let mut queue = self.tool_request_queue.write().await;
        queue.push(request);
    }

    /// Procesar todos los tool requests en cola
    async fn process_tool_queue(&mut self) {
        // Tomar todos los requests de la cola
        let requests = {
            let mut queue = self.tool_request_queue.write().await;
            let mut requests = Vec::new();
            while let Some(req) = queue.pop() {
                requests.push(req);
            }
            requests
        };

        if requests.is_empty() {
            return;
        }

        eprintln!("[coordinator] processing {} tool requests", requests.len());

        // Ejecutar cada request
        for request in requests {
            self.execute_tool_request(request).await;
        }
    }

    /// Ejecutar un tool request y distribuir resultado
    async fn execute_tool_request(&self, request: ToolRequest) {
        let start = std::time::Instant::now();

        // Generar tool_call_id unico
        let tool_call_id = format!("debate-{}-{}", request.tool_name, request.request_id);

        // Construir ToolContext valido
        let ctx = build_tool_context(
            &self.session_id,
            &self.message_id,
            self.working_dir.clone(),
            tool_call_id.clone(),
        );

        eprintln!(
            "[coordinator] executing {} for {} (request_id: {})",
            request.tool_name, request.perspective_name, request.request_id
        );

        // Ejecutar el tool
        let result = self
            .registry
            .execute(&request.tool_name, request.input.clone(), ctx)
            .await;

        let elapsed_ms = start.elapsed().as_millis() as u64;

        // Crear ToolResult
        let tool_result = match result {
            Ok(output) => ToolResult::success(
                request.request_id.clone(),
                request.tool_name.clone(),
                request.perspective_name.clone(),
                output.output,
                elapsed_ms,
            ),
            Err(e) => ToolResult::error(
                request.request_id.clone(),
                request.tool_name.clone(),
                request.perspective_name.clone(),
                e.to_string(),
                elapsed_ms,
            ),
        };

        // Guardar en historico
        {
            let mut recent = self.recent_results.write().await;
            recent.push(tool_result.clone());
            // Mantener solo ultimos 100 resultados
            if recent.len() > 100 {
                let overflow = recent.len() - 100;
                recent.drain(0..overflow);
            }
        }

        // Distribuir a TODAS las perspectivas via broadcast
        self.broadcast_tool_result(tool_result).await;
    }

    /// Broadcast del resultado a todas las perspectivas
    async fn broadcast_tool_result(&self, result: ToolResult) {
        let broadcast = self.tool_result_broadcast.read().await;
        for tx in broadcast.iter() {
            let _ = tx.send(result.clone());
        }
    }

    /// Obtener resultados recientes
    pub async fn get_recent_results(&self) -> Vec<ToolResult> {
        let recent = self.recent_results.read().await;
        recent.clone()
    }

    /// Obtener estado del queue
    pub async fn queue_size(&self) -> usize {
        let queue = self.tool_request_queue.read().await;
        queue.len()
    }
}

// =============================================================================
// Trait para perspectivas (para registrarlas como Weak references)
// =============================================================================

pub trait Perspective: Send + Sync {
    fn name(&self) -> &str;
    fn system_prompt(&self) -> &str;
    fn receive_tool_result(&self, result: &ToolResult) -> bool;
    fn comment_on_result(&self, result: &ToolResult) -> Option<String>;
}

// =============================================================================
// Integracion con Agent existente
// =============================================================================

/// Wrapper que permite al Agent de jcode actuar como Coordinator
/// en un debate multi-perspectiva
pub struct CoordinatorAgent {
    /// Referencia al agent (para ejecutar tools)
    agent: Arc<RwLock<crate::agent::Agent>>,
    /// Perspectivas activas
    perspectives: Arc<RwLock<HashMap<String, CoordinatorSender>>>,
    /// Tool results broadcast
    tool_results_tx: mpsc::UnboundedSender<ToolResult>,
}

impl CoordinatorAgent {
    pub fn new(
        agent: Arc<RwLock<crate::agent::Agent>>,
    ) -> (
        Self,
        mpsc::UnboundedSender<ToolResult>,
        mpsc::UnboundedReceiver<ToolResult>,
    ) {
        let (tool_results_tx, tool_results_rx) = mpsc::unbounded_channel();
        (
            Self {
                agent,
                perspectives: Arc::new(RwLock::new(HashMap::new())),
                tool_results_tx: tool_results_tx.clone(),
            },
            tool_results_tx,
            tool_results_rx,
        )
    }

    /// Recibir request de tool desde una perspectiva
    pub async fn receive_tool_request(&self, request: ToolRequest) -> Result<()> {
        let agent = self.agent.read().await;
        let session_id = agent.session_id();

        // Construir ToolContext
        let ctx = crate::tool::ToolContext {
            session_id: session_id.to_string(),
            message_id: uuid_v4(),
            tool_call_id: format!("debate-{}-{}", request.tool_name, request.request_id),
            working_dir: Some(agent.working_dir().map(PathBuf::from).unwrap_or_default()),
            stdin_request_tx: None,
            graceful_shutdown_signal: None,
            execution_mode: crate::tool::ToolExecutionMode::Direct,
        };

        // Ejecutar tool
        let result = agent
            .registry()
            .execute(&request.tool_name, request.input, ctx)
            .await?;

        // Crear ToolResult
        let tool_result = ToolResult::success(
            request.request_id,
            request.tool_name,
            request.perspective_name,
            result.output,
            0, // TODO: medir tiempo
        );

        // Broadcast a todas las perspectivas
        let perspectives = self.perspectives.read().await;
        for (_, tx) in perspectives.iter() {
            let msg = DebateMessage::ToolResult {
                result: tool_result.clone(),
            };
            let _ = tx.send(msg);
        }

        Ok(())
    }

    /// Registrar una perspectiva
    pub async fn register_perspective(&self, name: String, tx: CoordinatorSender) {
        let mut perspectives = self.perspectives.write().await;
        perspectives.insert(name, tx);
    }

    /// Obtener canal de broadcast para tool results
    pub fn tool_results_channel(&self) -> mpsc::UnboundedSender<ToolResult> {
        self.tool_results_tx.clone()
    }
}

// =============================================================================
// Helpers
// =============================================================================

fn uuid_v4() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let pid = std::process::id();
    format!(
        "{:x}-{:x}-4-{:x}-{:x}",
        now.as_secs(),
        now.subsec_nanos(),
        pid,
        COUNTER.fetch_add(1, Ordering::SeqCst)
    )
}

fn build_tool_context(
    session_id: &str,
    message_id: &str,
    working_dir: Option<PathBuf>,
    tool_call_id: String,
) -> ToolContext {
    ToolContext {
        session_id: session_id.to_string(),
        message_id: message_id.to_string(),
        tool_call_id,
        working_dir,
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: crate::tool::ToolExecutionMode::Direct,
    }
}
