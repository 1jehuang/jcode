use super::workflow::WorkflowConfig;
use super::step::WorkflowStep;

/// Predefined workflow templates for common tasks
pub struct WorkflowTemplate;

impl WorkflowTemplate {
    pub fn build_and_test() -> WorkflowConfig {
        WorkflowConfig::new("Build & Test")
            .with_var("target", "debug")
            .with_step(WorkflowStep::cmd("Check", "cargo check"))
            .with_step(WorkflowStep::cmd("Clippy", "cargo clippy"))
            .with_step(WorkflowStep::cmd("Unit Tests", "cargo test --lib"))
            .with_step(WorkflowStep::cmd("Build", "cargo build"))
    }

    pub fn full_ci() -> WorkflowConfig {
        WorkflowConfig::new("Full CI Pipeline")
            .with_var("profile", "release")
            .with_step(WorkflowStep::cmd("Format Check", "cargo fmt --check"))
            .with_step(WorkflowStep::cmd("Lint", "cargo clippy -- -D warnings"))
            .with_step(WorkflowStep::cmd("Build", "cargo build --release"))
            .with_step(WorkflowStep::cmd("Test All", "cargo test"))
            .with_step(WorkflowStep::cmd("Doc Tests", "cargo test --doc"))
    }

    pub fn review_and_deploy() -> WorkflowConfig {
        WorkflowConfig::new("Review & Deploy")
            .with_var("environment", "production")
            .with_step(WorkflowStep::cmd("Run Tests", "cargo test"))
            .with_step(WorkflowStep::approval("Approval", "Approve deployment?"))
            .with_step(WorkflowStep::cmd("Build Release", "cargo build --release"))
    }

    pub fn git_sync() -> WorkflowConfig {
        WorkflowConfig::new("Git Sync")
            .with_step(WorkflowStep::cmd("Fetch", "git fetch --all"))
            .with_step(WorkflowStep::cmd("Status", "git status"))
            .with_step(WorkflowStep::cmd("Pull Main", "git pull origin main"))
    }

    pub fn security_check() -> WorkflowConfig {
        WorkflowConfig::new("Security Check")
            .with_step(WorkflowStep::cmd("Audit Dependencies", "cargo audit"))
            .with_step(WorkflowStep::cmd("Secret Scan", "git secrets --scan"))
            .with_step(WorkflowStep::cmd("Dependencies Outdated", "cargo outdated"))
    }
}