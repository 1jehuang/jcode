use super::workflow::WorkflowConfig;
use super::step::WorkflowStep;

/// Predefined workflow templates for common tasks
pub struct WorkflowTemplate;

/// Information about a workflow template
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TemplateInfo {
    pub name: String,
    pub description: String,
    pub steps: Vec<TemplateStepInfo>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TemplateStepInfo {
    pub name: String,
    pub description: String,
}

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

    /// Return all template names and their metadata
    pub fn all() -> Vec<TemplateInfo> {
        vec![
            TemplateInfo {
                name: "build-and-test".to_string(),
                description: "cargo check, clippy, test, build".to_string(),
                steps: vec![
                    TemplateStepInfo { name: "Check".to_string(), description: "cargo check".to_string() },
                    TemplateStepInfo { name: "Clippy".to_string(), description: "cargo clippy".to_string() },
                    TemplateStepInfo { name: "Unit Tests".to_string(), description: "cargo test --lib".to_string() },
                    TemplateStepInfo { name: "Build".to_string(), description: "cargo build".to_string() },
                ],
            },
            TemplateInfo {
                name: "full-ci".to_string(),
                description: "format check, lint, build, test all, doc tests".to_string(),
                steps: vec![
                    TemplateStepInfo { name: "Format Check".to_string(), description: "cargo fmt --check".to_string() },
                    TemplateStepInfo { name: "Lint".to_string(), description: "cargo clippy -- -D warnings".to_string() },
                    TemplateStepInfo { name: "Build".to_string(), description: "cargo build --release".to_string() },
                    TemplateStepInfo { name: "Test All".to_string(), description: "cargo test".to_string() },
                    TemplateStepInfo { name: "Doc Tests".to_string(), description: "cargo test --doc".to_string() },
                ],
            },
            TemplateInfo {
                name: "review-and-deploy".to_string(),
                description: "test, approval, build release".to_string(),
                steps: vec![
                    TemplateStepInfo { name: "Run Tests".to_string(), description: "cargo test".to_string() },
                    TemplateStepInfo { name: "Approval".to_string(), description: "Approve deployment?".to_string() },
                    TemplateStepInfo { name: "Build Release".to_string(), description: "cargo build --release".to_string() },
                ],
            },
            TemplateInfo {
                name: "git-sync".to_string(),
                description: "fetch, status, pull".to_string(),
                steps: vec![
                    TemplateStepInfo { name: "Fetch".to_string(), description: "git fetch --all".to_string() },
                    TemplateStepInfo { name: "Status".to_string(), description: "git status".to_string() },
                    TemplateStepInfo { name: "Pull Main".to_string(), description: "git pull origin main".to_string() },
                ],
            },
            TemplateInfo {
                name: "security-check".to_string(),
                description: "audit deps, secret scan, outdated".to_string(),
                steps: vec![
                    TemplateStepInfo { name: "Audit Dependencies".to_string(), description: "cargo audit".to_string() },
                    TemplateStepInfo { name: "Secret Scan".to_string(), description: "git secrets --scan".to_string() },
                    TemplateStepInfo { name: "Dependencies Outdated".to_string(), description: "cargo outdated".to_string() },
                ],
            },
        ]
    }

    /// Find a template by name
    pub fn find(name: &str) -> Option<TemplateInfo> {
        Self::all().into_iter().find(|t| t.name == name)
    }

    /// Convert template name to WorkflowConfig for execution
    pub fn to_config(name: &str) -> Option<WorkflowConfig> {
        match name {
            "build-and-test" => Some(Self::build_and_test()),
            "full-ci" => Some(Self::full_ci()),
            "review-and-deploy" => Some(Self::review_and_deploy()),
            "git-sync" => Some(Self::git_sync()),
            "security-check" => Some(Self::security_check()),
            _ => None,
        }
    }
}