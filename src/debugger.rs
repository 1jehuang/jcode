use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebugSession {
    pub id: String,
    pub name: String,
    pub status: SessionStatus,
    pub created_at: u64,
    pub last_activity: u64,
    pub logs: Vec<DebugLogEntry>,
    pub breakpoints: Vec<Breakpoint>,
    pub current_frame: Option<StackFrame>,
    pub variables: HashMap<String, VariableValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SessionStatus {
    Running,
    Paused,
    Stopped,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebugLogEntry {
    pub timestamp: u64,
    pub level: LogLevel,
    pub message: String,
    pub module: String,
    pub file: Option<String>,
    pub line: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Breakpoint {
    pub id: String,
    pub file_path: String,
    pub line: usize,
    pub enabled: bool,
    pub condition: Option<String>,
    pub hit_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StackFrame {
    pub id: String,
    pub function_name: String,
    pub file_path: String,
    pub line: usize,
    pub column: Option<usize>,
    pub arguments: HashMap<String, VariableValue>,
    pub locals: HashMap<String, VariableValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VariableValue {
    pub name: String,
    pub value: String,
    pub type_name: String,
    pub is_primitive: bool,
    pub children: Option<Vec<VariableValue>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebugCommand {
    pub id: String,
    pub command_type: CommandType,
    pub target_session: String,
    pub arguments: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CommandType {
    Start,
    Pause,
    Continue,
    StepOver,
    StepInto,
    StepOut,
    Stop,
    SetBreakpoint,
    RemoveBreakpoint,
    GetVariables,
    EvaluateExpression,
    GetStack,
}

#[derive(Debug, Clone)]
pub struct DebuggerManager {
    sessions: Arc<RwLock<HashMap<String, DebugSession>>>,
    next_session_id: Arc<RwLock<u64>>,
    log_buffer: Arc<RwLock<VecDeque<DebugLogEntry>>>,
    max_log_entries: usize,
}

impl DebuggerManager {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            next_session_id: Arc::new(RwLock::new(1)),
            log_buffer: Arc::new(RwLock::new(VecDeque::with_capacity(1000))),
            max_log_entries: 1000,
        }
    }

    pub async fn create_session(&self, name: &str) -> Result<String> {
        let id = Uuid::new_v4().to_string();
        let now = Self::current_timestamp();
        
        let session = DebugSession {
            id: id.clone(),
            name: name.to_string(),
            status: SessionStatus::Running,
            created_at: now,
            last_activity: now,
            logs: Vec::new(),
            breakpoints: Vec::new(),
            current_frame: None,
            variables: HashMap::new(),
        };

        self.sessions.write().await.insert(id.clone(), session);
        Ok(id)
    }

    pub async fn get_session(&self, session_id: &str) -> Result<DebugSession> {
        self.sessions
            .read()
            .await
            .get(session_id)
            .cloned()
            .ok_or_else(|| anyhow!("Session not found: {}", session_id))
    }

    pub async fn update_session(&self, session_id: &str, session: DebugSession) -> Result<()> {
        self.sessions.write().await.insert(session_id.to_string(), session);
        Ok(())
    }

    pub async fn delete_session(&self, session_id: &str) -> Result<()> {
        if self.sessions.write().await.remove(session_id).is_none() {
            return Err(anyhow!("Session not found: {}", session_id));
        }
        Ok(())
    }

    pub async fn list_sessions(&self) -> Vec<DebugSession> {
        self.sessions.read().await.values().cloned().collect()
    }

    pub async fn execute_command(&self, command: DebugCommand) -> Result<DebugResponse> {
        let mut sessions = self.sessions.write().await;
        
        let session = sessions.get_mut(&command.target_session)
            .ok_or_else(|| anyhow!("Session not found: {}", command.target_session))?;

        session.last_activity = Self::current_timestamp();

        match command.command_type {
            CommandType::Start => {
                session.status = SessionStatus::Running;
                Ok(DebugResponse::Success("Session started".to_string()))
            }
            CommandType::Pause => {
                session.status = SessionStatus::Paused;
                Ok(DebugResponse::Success("Session paused".to_string()))
            }
            CommandType::Continue => {
                session.status = SessionStatus::Running;
                Ok(DebugResponse::Success("Session resumed".to_string()))
            }
            CommandType::StepOver => {
                session.status = SessionStatus::Running;
                Ok(DebugResponse::Success("Step over executed".to_string()))
            }
            CommandType::StepInto => {
                session.status = SessionStatus::Running;
                Ok(DebugResponse::Success("Step into executed".to_string()))
            }
            CommandType::StepOut => {
                session.status = SessionStatus::Running;
                Ok(DebugResponse::Success("Step out executed".to_string()))
            }
            CommandType::Stop => {
                session.status = SessionStatus::Stopped;
                Ok(DebugResponse::Success("Session stopped".to_string()))
            }
            CommandType::SetBreakpoint => {
                if let Some(args) = command.arguments {
                    let file_path = args["file_path"].as_str().unwrap_or("");
                    let line = args["line"].as_u64().unwrap_or(0) as usize;
                    let condition = args["condition"].as_str().map(|s| s.to_string());
                    
                    let breakpoint = Breakpoint {
                        id: Uuid::new_v4().to_string(),
                        file_path: file_path.to_string(),
                        line,
                        enabled: true,
                        condition,
                        hit_count: 0,
                    };
                    session.breakpoints.push(breakpoint);
                }
                Ok(DebugResponse::Success("Breakpoint set".to_string()))
            }
            CommandType::RemoveBreakpoint => {
                if let Some(args) = command.arguments {
                    let breakpoint_id = args["breakpoint_id"].as_str().unwrap_or("");
                    session.breakpoints.retain(|b| b.id != breakpoint_id);
                }
                Ok(DebugResponse::Success("Breakpoint removed".to_string()))
            }
            CommandType::GetVariables => {
                Ok(DebugResponse::Variables(session.variables.clone()))
            }
            CommandType::EvaluateExpression => {
                if let Some(args) = command.arguments {
                    let expression = args["expression"].as_str().unwrap_or("");
                    let result = self.evaluate_expression(session, expression);
                    Ok(DebugResponse::EvaluationResult(result))
                } else {
                    Err(anyhow!("Missing expression argument"))
                }
            }
            CommandType::GetStack => {
                let frames = session.current_frame.as_ref()
                    .map(|f| vec![f.clone()])
                    .unwrap_or_default();
                Ok(DebugResponse::StackFrames(frames))
            }
        }
    }

    fn evaluate_expression(&self, _session: &DebugSession, expression: &str) -> String {
        format!("Evaluated: {}", expression)
    }

    pub async fn add_log_entry(&self, entry: DebugLogEntry) {
        let mut buffer = self.log_buffer.write().await;
        
        if buffer.len() >= self.max_log_entries {
            buffer.pop_front();
        }
        buffer.push_back(entry.clone());

        let mut sessions = self.sessions.write().await;
        for session in sessions.values_mut() {
            if session.logs.len() >= 100 {
                session.logs.remove(0);
            }
            session.logs.push(entry.clone());
        }
    }

    pub async fn log(&self, level: LogLevel, message: &str, module: &str, file: Option<String>, line: Option<usize>) {
        let entry = DebugLogEntry {
            timestamp: Self::current_timestamp(),
            level,
            message: message.to_string(),
            module: module.to_string(),
            file,
            line,
        };
        self.add_log_entry(entry).await;
    }

    pub async fn get_recent_logs(&self, count: usize) -> Vec<DebugLogEntry> {
        let buffer = self.log_buffer.read().await;
        let start = buffer.len().saturating_sub(count);
        buffer.range(start..).cloned().collect()
    }

    pub async fn get_logs_by_level(&self, level: LogLevel, count: usize) -> Vec<DebugLogEntry> {
        let buffer = self.log_buffer.read().await;
        buffer
            .iter()
            .filter(|e| e.level == level)
            .rev()
            .take(count)
            .cloned()
            .collect()
    }

    pub async fn get_errors(&self, count: usize) -> Vec<DebugLogEntry> {
        self.get_logs_by_level(LogLevel::Error, count).await
    }

    pub async fn get_warnings(&self, count: usize) -> Vec<DebugLogEntry> {
        self.get_logs_by_level(LogLevel::Warn, count).await
    }

    pub async fn clear_logs(&self) {
        self.log_buffer.write().await.clear();
        
        let mut sessions = self.sessions.write().await;
        for session in sessions.values_mut() {
            session.logs.clear();
        }
    }

    pub async fn get_session_stats(&self) -> SessionStats {
        let sessions = self.sessions.read().await;
        let logs = self.log_buffer.read().await;
        
        let running_count = sessions.values().filter(|s| s.status == SessionStatus::Running).count();
        let paused_count = sessions.values().filter(|s| s.status == SessionStatus::Paused).count();
        let stopped_count = sessions.values().filter(|s| s.status == SessionStatus::Stopped).count();
        
        let error_count = logs.iter().filter(|l| l.level == LogLevel::Error).count();
        let warn_count = logs.iter().filter(|l| l.level == LogLevel::Warn).count();

        SessionStats {
            total_sessions: sessions.len(),
            running_sessions: running_count,
            paused_sessions: paused_count,
            stopped_sessions: stopped_count,
            total_log_entries: logs.len(),
            error_count,
            warn_count,
        }
    }

    fn current_timestamp() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DebugResponse {
    Success(String),
    Error(String),
    Variables(HashMap<String, VariableValue>),
    StackFrames(Vec<StackFrame>),
    EvaluationResult(String),
    LogEntries(Vec<DebugLogEntry>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStats {
    pub total_sessions: usize,
    pub running_sessions: usize,
    pub paused_sessions: usize,
    pub stopped_sessions: usize,
    pub total_log_entries: usize,
    pub error_count: usize,
    pub warn_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_session() {
        let manager = DebuggerManager::new();
        let session_id = manager.create_session("test").await.unwrap();
        assert!(!session_id.is_empty());
        
        let session = manager.get_session(&session_id).await.unwrap();
        assert_eq!(session.name, "test");
        assert_eq!(session.status, SessionStatus::Running);
    }

    #[tokio::test]
    async fn test_execute_command() {
        let manager = DebuggerManager::new();
        let session_id = manager.create_session("test").await.unwrap();
        
        let command = DebugCommand {
            id: "cmd1".to_string(),
            command_type: CommandType::Pause,
            target_session: session_id.clone(),
            arguments: None,
        };
        
        let response = manager.execute_command(command).await.unwrap();
        assert!(matches!(response, DebugResponse::Success(_)));
        
        let session = manager.get_session(&session_id).await.unwrap();
        assert_eq!(session.status, SessionStatus::Paused);
    }

    #[tokio::test]
    async fn test_logging() {
        let manager = DebuggerManager::new();
        
        manager.log(LogLevel::Error, "test error", "test_module", None, None).await;
        manager.log(LogLevel::Warn, "test warning", "test_module", None, None).await;
        
        let errors = manager.get_errors(10).await;
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].level, LogLevel::Error);
        
        let warnings = manager.get_warnings(10).await;
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].level, LogLevel::Warn);
    }

    #[tokio::test]
    async fn test_breakpoints() {
        let manager = DebuggerManager::new();
        let session_id = manager.create_session("test").await.unwrap();
        
        let args = serde_json::json!({
            "file_path": "src/main.rs",
            "line": 42
        });
        
        let command = DebugCommand {
            id: "cmd1".to_string(),
            command_type: CommandType::SetBreakpoint,
            target_session: session_id.clone(),
            arguments: Some(args),
        };
        
        manager.execute_command(command).await.unwrap();
        
        let session = manager.get_session(&session_id).await.unwrap();
        assert_eq!(session.breakpoints.len(), 1);
        assert_eq!(session.breakpoints[0].line, 42);
    }

    #[tokio::test]
    async fn test_get_stats() {
        let manager = DebuggerManager::new();
        manager.create_session("test1").await.unwrap();
        manager.create_session("test2").await.unwrap();
        
        let stats = manager.get_session_stats().await;
        assert_eq!(stats.total_sessions, 2);
        assert_eq!(stats.running_sessions, 2);
    }
}