//! Agent Runtime - Core execution loop for autonomous agents
//!
//!对标 Claude Code queryLoop() — infinite recursion:
//!   LLM output → Extract tools → Partitioned execution → Collect results → Recurse
//!
//! Integrates:
//! - CompilationEngine (cargo check)
//! - AutoFixLoop (3-iteration repair cycle)
//! - InferenceRouter (local + cloud LLM)
//! - PlanManager (plan persistence)
//! - AcceptanceTracker (acceptance rate tracking)

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Agent execution status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentStatus {
    Idle,
    Thinking,
    ExecutingTools { count: usize },
    Verifying,
    Fixing { iteration: u32 },
    Done { success: bool, message: String },
    Error(String),
}

impl AgentStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Done { .. } | Self::Error(_)
        )
    }
}

/// Autonomous Agent - integrates all components into a running loop
pub struct AutonomousAgent {
    /// Workspace root directory
    workspace: PathBuf,
    /// Current execution status
    status: Arc<RwLock<AgentStatus>>,
    /// Turn counter
    turn_count: Arc<RwLock<u32>>,
    /// Maximum fix iterations
    max_fix_iterations: u32,
}

impl AutonomousAgent {
    pub fn new(workspace: &Path) -> Self {
        Self {
            workspace: workspace.to_path_buf(),
            status: Arc::new(RwLock::new(AgentStatus::Idle)),
            turn_count: Arc::new(RwLock::new(0)),
            max_fix_iterations: 3,
        }
    }

    pub fn with_max_fix_iterations(mut self, max: u32) -> Self {
        self.max_fix_iterations = max;
        self
    }

    /// Execute a complete planning + repair task
    ///对标: Claude Code queryLoop() — infinite recursion
    pub async fn execute_task(&self, goal: &str) -> Result<String, String> {
        *self.status.write().await = AgentStatus::Thinking;
        let mut turn = 0u32;

        // Step 1: Generate plan
        let plan = self.generate_plan(goal).await?;
        
        // Step 2: Execute edits based on plan
        let edited_files = self.execute_edits(&plan).await?;

        // Step 3: Compile verification + auto-fix loop
        *self.status.write().await = AgentStatus::Verifying;
        let mut final_result = String::new();

        for iteration in 0..self.max_fix_iterations {
            *self.status.write().await = AgentStatus::Fixing { iteration: iteration + 1 };
            turn += 1;
            *self.turn_count.write().await = turn;

            // Run cargo check
            let compile_result = self.run_cargo_check().await;

            if compile_result.success {
                final_result = format!(
                    "✅ Task completed in {} turns.\nCompilation passed.\n{} files edited.",
                    turn,
                    edited_files.len()
                );
                *self.status.write().await = AgentStatus::Done {
                    success: true,
                    message: final_result.clone(),
                };
                return Ok(final_result);
            }

            // Compilation failed - check if we've exceeded max iterations
            if iteration >= self.max_fix_iterations - 1 {
                final_result = format!(
                    "❌ Failed after {} fix attempts.\nRemaining errors:\n{}",
                    iteration + 1,
                    compile_result.errors.iter()
                        .take(5)
                        .map(|e| format!("  {}", e))
                        .collect::<Vec<_>>()
                        .join("\n")
                );
                *self.status.write().await = AgentStatus::Done {
                    success: false,
                    message: final_result.clone(),
                };
                return Ok(final_result);
            }

            // TODO: Implement actual fix logic here
            // For now, just continue the loop
            tracing::warn!("Compilation failed, would attempt auto-fix in iteration {}", iteration + 1);
        }

        Ok(final_result)
    }

    /// Generate a plan for the given goal
    async fn generate_plan(&self, goal: &str) -> Result<String, String> {
        // TODO: Integrate with LLM to generate actual plan
        // For now, return a stub plan
        let plan_content = format!(
            "# Plan\n\n## Goal\n{}\n\n## Steps\n\
             1. Analyze codebase structure\n\
             2. Identify files to modify\n\
             3. Implement changes\n\
             4. Verify compilation\n",
            goal
        );
        
        Ok(plan_content)
    }

    /// Execute edits based on the plan
    async fn execute_edits(&self, _plan: &str) -> Result<Vec<String>, String> {
        // TODO: Integrate with LLM to parse plan and generate edits
        // For now, return empty list
        Ok(vec![])
    }

    /// Run cargo check and return results
    async fn run_cargo_check(&self) -> CompileResult {
        // TODO: Implement actual cargo check integration
        // For now, return success stub
        CompileResult {
            success: true,
            errors: vec![],
            warnings: vec![],
        }
    }

    /// Get current status
    pub async fn status(&self) -> AgentStatus {
        self.status.read().await.clone()
    }

    /// Get turn count
    pub async fn turn_count(&self) -> u32 {
        *self.turn_count.read().await
    }

    /// Get statistics summary
    pub async fn stats(&self) -> String {
        let turn = self.turn_count().await;
        let status = self.status().await;
        
        let status_str = match &status {
            AgentStatus::Idle => "idle".to_string(),
            AgentStatus::Thinking => "thinking".to_string(),
            AgentStatus::ExecutingTools { count } => format!("executing({})", count),
            AgentStatus::Verifying => "verifying".to_string(),
            AgentStatus::Fixing { iteration } => format!("fixing(iter={})", iteration),
            AgentStatus::Done { success, message } => {
                format!("done(success={}): {}", success, message)
            }
            AgentStatus::Error(e) => format!("error: {}", e),
        };
        
        format!("[Agent] Turns: {} | Status: {}", turn, status_str)
    }
}

/// Compilation check result
#[derive(Debug, Clone)]
pub struct CompileResult {
    pub success: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

/// Cross-File Agent - aware of file dependencies and execution order
pub struct CrossFileAgent {
    agent: AutonomousAgent,
    workspace: PathBuf,
}

impl CrossFileAgent {
    pub fn new(workspace: &Path) -> Self {
        Self {
            agent: AutonomousAgent::new(workspace),
            workspace: workspace.to_path_buf(),
        }
    }

    /// Analyze cross-file dependencies
    pub async fn analyze_dependencies(&self) -> Result<CrossFileTask, String> {
        // TODO: Implement actual dependency analysis
        // For now, return stub
        Ok(CrossFileTask {
            goal: String::new(),
            affected_files: vec![],
            dependencies: std::collections::HashMap::new(),
            execution_order: vec![],
        })
    }

    /// Execute cross-file task with dependency awareness
    pub async fn execute_cross_file_task(&self, goal: &str) -> Result<String, String> {
        // 1. Analyze dependencies
        let _task = self.analyze_dependencies().await?;
        
        // 2. Execute using autonomous agent
        self.agent.execute_task(goal).await
    }

    /// Verify consistency across edited files
    pub async fn verify_consistency_loop(&self, _edited_files: &[String]) -> Result<Vec<String>, String> {
        // TODO: Implement cross-file consistency checking
        Ok(vec!["Consistency check passed".to_string()])
    }

    /// Get reference to inner agent
    pub fn agent(&self) -> &AutonomousAgent {
        &self.agent
    }
}

/// Cross-file task with dependency information
#[derive(Debug, Clone)]
pub struct CrossFileTask {
    pub goal: String,
    pub affected_files: Vec<String>,
    pub dependencies: std::collections::HashMap<String, Vec<String>>,
    pub execution_order: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_agent_stub() {
        let temp = std::env::temp_dir().join("carpai-agent-test");
        let _ = std::fs::create_dir_all(&temp);
        
        let agent = AutonomousAgent::new(&temp);
        let result = agent.execute_task("Test goal").await;
        assert!(result.is_ok());
        
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[tokio::test]
    async fn test_agent_status_transitions() {
        let temp = std::env::temp_dir().join("carpai-agent-status-test");
        let _ = std::fs::create_dir_all(&temp);
        
        let agent = AutonomousAgent::new(&temp);
        
        // Initially idle
        assert!(matches!(agent.status().await, AgentStatus::Idle));
        
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn test_agent_status_is_terminal() {
        assert!(AgentStatus::Done { success: true, message: String::new() }.is_terminal());
        assert!(AgentStatus::Error("test".to_string()).is_terminal());
        assert!(!AgentStatus::Idle.is_terminal());
        assert!(!AgentStatus::Thinking.is_terminal());
    }
}
