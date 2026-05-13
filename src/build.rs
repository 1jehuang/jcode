//! Build system entry point.
//!
//! This module bridges the `jcode_build_support` utilities (binary paths, versioning, etc.)
//! with the multi-language `BuildExecutor` from `build_module` and the `jcode-build-engine` crate.
//!
//! The `run_build_command` in `cli/commands.rs` uses the types defined here
//! to provide a unified CLI → BuildExecutor → MicroCi pipeline.

// ── Re-export build-support utilities (binary management, selfdev, etc.) ──
pub use jcode_build_support::{
    binary_name, binary_stem, build_log_path, build_progress_path, builds_dir,
    canary_binary_path, clear_build_progress, client_update_candidate,
    complete_pending_activation_for_session,
    current_binary_build_time_string, current_binary_built_at, current_binary_path,
    current_build_info, current_git_diff, current_git_hash, current_git_hash_full,
    current_source_state, current_version_file, ensure_source_state_matches,
    find_dev_binary, get_repo_dir, install_binary_at_version, install_local_release,
    install_version, is_jcode_repo, is_working_tree_dirty, launcher_binary_path,
    launcher_dir, manifest_path, PendingActivation, preferred_reload_candidate,
    promote_version_to_shared_server,
    publish_local_current_build, publish_local_current_build_for_source,
    read_build_progress, read_stable_version, read_current_version, read_shared_server_version,
    release_binary_path, rollback_pending_activation_for_session, selfdev_binary_path,
    selfdev_build_command, SelfDevBuildCommand, SelfDevBuildTarget,
    shared_server_binary_path, shared_server_version_file,
    smoke_test_binary, smoke_test_server_binary, stable_binary_path, stable_version_file,
    update_canary_symlink, update_current_symlink, update_launcher_symlink_to_current,
    update_launcher_symlink_to_stable, update_shared_server_symlink, update_stable_symlink,
    version_binary_path, write_build_progress, write_current_dev_binary_source_metadata,
    BuildManifest, CanaryStatus,
};

pub use jcode_selfdev_types::SourceState;

// ── Re-export the real multi-language build executor from build_module ──
pub use crate::build_module::{
    BuildExecutor, BuildProfile, BuildRequest, BuildResult, BuildTool, TestTool,
    WorkspaceBuildResult,
};
pub use crate::workspace_manager::{ProjectBuildConfig, ProjectType, WorkspaceManager};

// ── CLI-facing configuration types ──

/// High-level configuration for the CLI build command.
pub struct BuildConfig {
    /// Whether to auto-approve each step in the AI-driven flow.
    pub auto_approve: bool,
    /// Maximum number of retries per failed step.
    pub max_retries: u32,
    /// Whether to run micro-ci after the build succeeds.
    pub run_ci_after_build: bool,
    /// Optional path to write a build report file.
    pub report_path: Option<String>,
}

impl BuildConfig {
    /// Create a `BuildRequest` from this config, suitable for the executor.
    pub fn to_build_request(&self) -> BuildRequest {
        BuildRequest::default()
    }
}

/// Outcome status of a build pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildStatus {
    Pending,
    InProgress,
    Success,
    Failed,
    Cancelled,
}

/// Summary report produced after a build pipeline finishes.
pub struct BuildReport {
    /// The original user message / goal description.
    pub message: String,
    /// Final status.
    pub status: BuildStatus,
    /// Total execution wall-clock time in milliseconds.
    pub execution_time_ms: u64,
    /// Total time including post-build verification.
    pub total_time_ms: u64,
    /// Whether optional micro-ci checks passed.
    pub ci_passed: bool,
    /// Number of warnings reported by the build tool.
    pub warning_count: usize,
    /// Number of errors reported by the build tool.
    pub error_count: usize,
}

impl BuildReport {
    pub fn new(message: &str) -> Self {
        Self {
            message: message.to_string(),
            status: BuildStatus::Pending,
            execution_time_ms: 0,
            total_time_ms: 0,
            ci_passed: false,
            warning_count: 0,
            error_count: 0,
        }
    }

    /// Build a report from an executed `BuildResult`.
    pub fn from_build_result(message: &str, result: &BuildResult, ci_passed: bool) -> Self {
        Self {
            message: message.to_string(),
            status: if result.success {
                BuildStatus::Success
            } else {
                BuildStatus::Failed
            },
            execution_time_ms: result.duration.as_millis() as u64,
            total_time_ms: result.duration.as_millis() as u64,
            ci_passed,
            warning_count: result.warning_count,
            error_count: result.error_count,
        }
    }
}

impl std::fmt::Display for BuildReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let status_str = match self.status {
            BuildStatus::Success => "✅ SUCCESS",
            BuildStatus::Failed => "❌ FAILED",
            BuildStatus::Cancelled => "🚫 CANCELLED",
            BuildStatus::InProgress => "⏳ IN PROGRESS",
            BuildStatus::Pending => "⏸ PENDING",
        };
        writeln!(f, "─── Build Report ───")?;
        writeln!(f, "  Goal:        {}", self.message)?;
        writeln!(f, "  Status:      {}", status_str)?;
        writeln!(f, "  Build time:  {:.2}s", self.execution_time_ms as f64 / 1000.0)?;
        if self.total_time_ms != self.execution_time_ms {
            writeln!(
                f,
                "  Total time:  {:.2}s",
                self.total_time_ms as f64 / 1000.0
            )?;
        }
        writeln!(f, "  Errors:      {}", self.error_count)?;
        writeln!(f, "  Warnings:    {}", self.warning_count)?;
        writeln!(
            f,
            "  CI:          {}",
            if self.ci_passed {
                "✅ passed"
            } else {
                "⚠️  not run / had issues"
            }
        )?;
        Ok(())
    }
}

// ── Progress display ──

/// Minimal progress bar for CLI display.
pub struct ProgressBar {
    total: u64,
    current: u64,
    message: String,
}

impl ProgressBar {
    pub fn new(total: u64, message: &str) -> Self {
        eprintln!("  {}", message);
        Self {
            total,
            current: 0,
            message: message.to_string(),
        }
    }

    pub fn inc(&mut self, delta: u64) {
        self.current += delta;
        if self.current <= self.total {
            eprint!(
                "\r  {} [{}/{}]",
                self.message, self.current, self.total
            );
        }
    }

    pub fn finish(&mut self) {
        self.current = self.total;
        eprint!("\r  {} [{}/{}] ✓\n", self.message, self.current, self.total);
    }

    pub fn set_message(&mut self, msg: &str) {
        self.message = msg.to_string();
    }
}

// ── Build engine wrapper ──

/// Unified build engine wrapping the real `BuildExecutor`.
pub struct BuildEngine {
    pub config: BuildConfig,
    pub executor: BuildExecutor,
}

impl BuildEngine {
    /// Create a new engine with the given CLI config.
    /// Initializes the workspace manager and build executor.
    pub fn new(config: BuildConfig) -> Self {
        let workspace = std::sync::Arc::new(WorkspaceManager::new());
        // Auto-detect the current working directory as a project
        if let Ok(cwd) = std::env::current_dir() {
            let project_type = ProjectType::detect_from_path(&cwd);
            let name = cwd
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "project".into());
            let project = crate::workspace_manager::Project::new(
                "default",
                name,
                &cwd,
                project_type,
            );
            let _ = workspace.register_project(project);
            let _ = workspace.set_active_project("default");
        }
        let executor = BuildExecutor::new(workspace);
        Self { config, executor }
    }

    /// Execute a build for the active project using the real BuildExecutor.
    pub async fn execute(&self, request: &BuildRequest) -> anyhow::Result<BuildResult> {
        self.executor.build_active_project(request).await
    }

    /// Execute a build for all workspace projects.
    pub async fn execute_all(
        &self,
        request: &BuildRequest,
        parallel: bool,
        max_jobs: usize,
    ) -> anyhow::Result<WorkspaceBuildResult> {
        self.executor.build_all(request, parallel, max_jobs).await
    }

    /// Create a build report from the result.
    pub fn report(&self, message: &str, result: &BuildResult, ci_passed: bool) -> BuildReport {
        BuildReport::from_build_result(message, result, ci_passed)
    }
}

// ── Shared server update helper ──

pub fn shared_server_update_candidate(_is_selfdev_session: bool) -> Option<String> {
    None
}

pub async fn run_selfdev_build(_target: Option<&str>) -> Result<(), String> {
    Err("selfdev build not supported".to_string())
}

pub fn run_selfdev_build_sync(_repo_dir: &std::path::Path) -> Result<(), String> {
    Err("selfdev build not supported".to_string())
}

pub fn selfdev_build_command_for_target(_repo_dir: &std::path::Path, _target: &SelfDevBuildTarget) -> SelfDevBuildCommand {
    SelfDevBuildCommand {
        program: "cargo".to_string(),
        args: vec!["build".to_string()],
        display: "cargo build".to_string(),
    }
}
