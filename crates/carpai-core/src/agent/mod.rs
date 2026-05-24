//! Agent System - Business Logic Layer (Layer 1)
//!
//! This module contains all agent-related business logic implementations:
//! - Core Agent types and state management
//! - Runtime execution loop (对标 Claude Code queryLoop)
//! - Sub-agent orchestration (parallel task execution)
//! - Skill system integration
//! - Plan mode support
//! - Task planning, decomposition, scheduling, and verification

// --- Core Agent Types ---
pub mod runtime;
pub mod sub_agents;
pub mod skill_system;
pub mod plan_mode;

// --- Task Planning System ---
pub mod task {
    pub mod planner;
    pub mod manager;
    pub mod decomposer;
    pub mod scheduler;
    pub mod verifier;
    pub mod ultraplan;
}

// Re-export key public types for convenience
pub use runtime::{AutonomousAgent, CrossFileAgent, AgentStatus};
pub use sub_agents::{
    SubAgentTask, SubAgentConfig, SubAgentResult, SubAgentStatus,
    ParallelTaskScheduler, OrchestrationResult,
};
pub use plan_mode::{PlanModeState, Plan, PlanStep, StepStatus, PLAN_MODE_SYSTEM_PROMPT};
pub use skill_system::SkillRegistry;

// Task system re-exports
pub use task::planner::TaskPlanner;
pub use task::manager::TaskManager;
pub use task::decomposer::TaskDecomposer;
pub use task::scheduler::TaskScheduler;
pub use task::verifier::PlanVerifier;
