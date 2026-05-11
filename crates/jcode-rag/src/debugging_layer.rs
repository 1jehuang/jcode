//! Layer 5: Debugging Layer - Log Injection + Breakpoint Management

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use anyhow::Result;
use chrono::{DateTime, Utc};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::{
    PhaseResult, PhaseName, PhaseOutput, SurgicalRequest,
    LogInjection, LogLevel, InjectionLocation,
    BreakpointInfo, BreakpointLocation,
    ExecutionTrace, ExecutionStep, StepType, PathLocation, TraceConfig,
    DebuggingLayer,
};

/// Debugging configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebuggingConfig {
    pub enable_log_injection: bool,
    pub enable_breakpoint_management: bool,
    pub enable_execution_tracing: bool,
    pub default_log_level: LogLevel,
    pub max_injections_per_file: usize,
}

impl Default for DebuggingConfig {
    fn default() -> Self {
        Self {
            enable_log_injection: true,
            enable_breakpoint_management: true,
            enable_execution_tracing: true,
            default_log_level: LogLevel::Debug,
            max_injections_per_file: 10,
        }
    }
}

/// Debug session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebugSession {
    pub session_id: String,
    pub request_id: String,
    pub created_at: DateTime<Utc>,
    pub status: String,
    pub log_injections: Vec<LogInjection>,
    pub breakpoints: Vec<BreakpointInfo>,
    pub execution_traces: Vec<ExecutionTrace>,
}

/// Observability Manager
pub struct ObservabilityManager {
    config: DebuggingConfig,
    active_sessions: Arc<RwLock<HashMap<String, DebugSession>>>,
}

impl ObservabilityManager {
    pub fn new(config: DebuggingConfig) -> Self {
        Self {
            config,
            active_sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn create_session(&self, request: &SurgicalRequest) -> Result<String> {
        let session_id = format!("debug_{}", Uuid::new_v4());
        
        let session = DebugSession {
            session_id: session_id.clone(),
            request_id: request.request_id.clone(),
            created_at: Utc::now(),
            status: "active".to_string(),
            log_injections: Vec::new(),
            breakpoints: Vec::new(),
            execution_traces: Vec::new(),
        };

        self.active_sessions.write().insert(session_id.clone(), session);
        
        Ok(session_id)
    }

    pub async fn inject_smart_logs(
        &self,
        session_id: &str,
        files: &[PathBuf],
    ) -> Result<Vec<LogInjection>> {
        if !self.config.enable_log_injection {
            return Ok(Vec::new());
        }

        let mut injections = Vec::new();
        
        for file in files {
            // Simple implementation: add debug logging at function entries
            let injection = LogInjection {
                id: format!("log_{}", Uuid::new_v4()),
                location: InjectionLocation {
                    file_path: file.clone(),
                    line: 1,
                    insert_before: true,
                },
                level: self.config.default_log_level,
                template: format!("[DEBUG] Entering file: {}", file.display()),
                condition: None,
                is_active: true,
            };
            
            injections.push(injection);
        }

        // Update session
        if let Some(session) = self.active_sessions.write().get_mut(session_id) {
            session.log_injections = injections.clone();
        }

        Ok(injections)
    }

    pub async fn set_smart_breakpoints(
        &self,
        session_id: &str,
        locations: &[BreakpointLocation],
    ) -> Result<Vec<BreakpointInfo>> {
        if !self.config.enable_breakpoint_management {
            return Ok(Vec::new());
        }

        let mut breakpoints = Vec::new();
        
        for (i, location) in locations.iter().enumerate() {
            let bp = BreakpointInfo {
                id: format!("bp_{}", i),
                location: location.clone(),
                condition: None,
                hit_count: 0,
                enabled: true,
            };
            
            breakpoints.push(bp);
        }

        // Update session
        if let Some(session) = self.active_sessions.write().get_mut(session_id) {
            session.breakpoints = breakpoints.clone();
        }

        Ok(breakpoints)
    }

    pub async fn start_execution_trace(
        &self,
        session_id: &str,
        _trace_config: TraceConfig,
    ) -> Result<String> {
        let trace_id = format!("trace_{}", Uuid::new_v4());
        
        // Create empty trace
        let trace = ExecutionTrace {
            trace_id: trace_id.clone(),
            steps: Vec::new(),
            total_duration_ms: 0,
        };

        // Update session
        if let Some(session) = self.active_sessions.write().get_mut(session_id) {
            session.execution_traces.push(trace);
        }

        Ok(trace_id)
    }

    pub async fn complete_session(&self, session_id: &str) -> Result<DebugSession> {
        let session = self.active_sessions.write().remove(session_id)
            .ok_or_else(|| anyhow::anyhow!("Session not found"))?;
        
        Ok(session)
    }

    pub fn get_active_session(&self, session_id: &str) -> Option<DebugSession> {
        self.active_sessions.read().get(session_id).cloned()
    }
}

#[async_trait::async_trait]
impl DebuggingLayer for ObservabilityManager {
    async fn inject_debug_info(
        &self,
        request: &SurgicalRequest,
        _phases: &[PhaseResult],
    ) -> Result<PhaseResult> {
        let start_time = std::time::Instant::now();

        info!(request_id = %request.request_id, "Injecting debug information");

        let session_id = self.create_session(request)?;
        
        // Collect affected files from phases (simplified)
        let files: Vec<PathBuf> = match &request.target {
            crate::TargetScope::SingleFile { path } => vec![path.clone()],
            crate::TargetScope::EntireProject { root } => vec![root.clone()],
            _ => Vec::new(),
        };

        // Inject logs
        let logs = if !files.is_empty() {
            self.inject_smart_logs(&session_id, &files).await?
        } else {
            Vec::new()
        };

        // Complete session
        let _session = self.complete_session(&session_id).await?;

        let duration_ms = start_time.elapsed().as_millis() as u64;

        Ok(PhaseResult {
            phase: PhaseName::Debugging,
            passed: true,
            duration_ms,
            output: PhaseOutput::DebuggingOutput {
                logs_injected: logs,
                breakpoints_set: Vec::new(),
                traces_captured: Vec::new(),
                debug_duration_ms: duration_ms,
            },
            warnings: Vec::new(),
            errors: Vec::new(),
        })
    }

    async fn set_breakpoint(&self, _bp: BreakpointInfo) -> Result<()> {
        // Simplified implementation
        Ok(())
    }

    async fn remove_breakpoint(&self, _bp_id: &str) -> Result<()> {
        // Simplified implementation
        Ok(())
    }

    async fn capture_execution_trace(&self, trace_config: TraceConfig) -> Result<ExecutionTrace> {
        // Create temporary session for trace capture
        let temp_request = SurgicalRequest {
            request_id: format!("trace_{}", Utc::now().timestamp()),
            intent: "Execution trace capture".to_string(),
            target: crate::TargetScope::EntireProject { 
                root: PathBuf::from(".") 
            },
            priority: crate::Priority::Normal,
            safety_mode: crate::SafetyMode::ReadOnly,
            created_at: Utc::now(),
            requested_by: "system".to_string(),
        };

        let session_id = self.create_session(&temp_request)?;
        let trace_id = self.start_execution_trace(&session_id, trace_config).await?;
        
        // Simulate some delay for trace collection
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        self.stop_execution_trace(&session_id, &trace_id).await
    }
}

impl ObservabilityManager {
    pub async fn stop_execution_trace(
        &self,
        session_id: &str,
        trace_id: &str,
    ) -> Result<ExecutionTrace> {
        if let Some(session) = self.active_sessions.read().get(session_id) {
            for trace in &session.execution_traces {
                if trace.trace_id == *trace_id {
                    return Ok(trace.clone());
                }
            }
        }

        Err(anyhow::anyhow!("Trace not found"))
    }
}
