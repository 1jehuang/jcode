//! 跨文件规划引擎
//!
//! 提供自主跨文件规划的完整框架：
//! - 计划生成与验证
//! - 文件依赖分析 (import/use/mod)
//! - 变更影响评估
//! - 执行顺序拓扑排序
//! - CLI 集成

pub mod plan;
pub mod dependency;
pub mod integration;
pub mod llm_planner;

pub use plan::{Plan, PlanStep, PlanValidator, PlanExecutionResult, StepType, StepStatus, ImpactLevel};
pub use dependency::{DependencyAnalyzer, FileDependency, ChangeImpact, ImpactType};
pub use llm_planner::{LlmPlanner, LlmPlannerConfig, PlanningRequest, PlanningResult, PlanProgress, format_progress};
