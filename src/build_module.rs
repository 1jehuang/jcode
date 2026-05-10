//! Multi-language build/compilation module for jcode.
//!
//! Provides a unified build system that supports multiple programming languages
//! and integrates with the workspace manager for multi-project builds.
//!
//! Supported languages:
//! - **Rust**: cargo build / cargo build --workspace
//! - **Node.js/TypeScript**: npm run build / tsc --build
//! - **React/Vue/Angular**: npm run build / ng build
//! - **Python**: python -m build / poetry build
//! - **Go**: go build ./...
//! - **C/C++**: cmake --build / make
//! - **Java/Kotlin**: mvn package / gradle build
//! - **C#/.NET**: dotnet build
//! - **Ruby**: bundle exec rake build

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::tool::{Tool, ToolContext, ToolOutput};
use crate::workspace_manager::{ProjectType, WorkspaceManager};

// =============================================================================
// Build result types
// =============================================================================

/// Outcome of a single build invocation.
#[derive(Debug, Clone)]
pub struct BuildResult {
    /// Whether the build succeeded
    pub success: bool,
    /// Exit code of the build process (0 = success)
    pub exit_code: Option<i32>,
    /// Captured stdout from the build process
    pub stdout: String,
    /// Captured stderr from the build process
    pub stderr: String,
    /// Combined output (stdout + stderr interleaved for display)
    pub output: String,
    /// Duration of the build
    pub duration: Duration,
    /// Number of warnings detected
    pub warning_count: usize,
    /// Number of errors detected
    pub error_count: usize,
    /// Language/project type that was built
    pub project_type: ProjectType,
    /// Path where the build was executed
    pub build_dir: PathBuf,
    /// Output artifact paths (if detectable)
    pub artifacts: Vec<PathBuf>,
}

impl BuildResult {
    /// Create a failure result from an error message.
    pub fn error(message: impl Into<String>, project_type: ProjectType, dir: &Path) -> Self {
        let msg = message.into();
        Self {
            success: false,
            exit_code: None,
            stdout: String::new(),
            stderr: msg.clone(),
            output: msg,
            duration: Duration::ZERO,
            warning_count: 0,
            error_count: 1,
            project_type,
            build_dir: dir.to_path_buf(),
            artifacts: Vec::new(),
        }
    }

    /// Format a summary line for display.
    pub fn summary_line(&self) -> String {
        if self.success {
            format!(
                "Build OK in {:.1}s ({} warnings)",
                self.duration.as_secs_f32(),
                self.warning_count
            )
        } else {
            format!(
                "Build FAILED in {:.1}s ({} errors, {} warnings) [exit code: {:?}]",
                self.duration.as_secs_f32(),
                self.error_count,
                self.warning_count,
                self.exit_code
            )
        }
    }
}

/// Result of a multi-project (workspace-level) build.
#[derive(Debug, Clone)]
pub struct WorkspaceBuildResult {
    /// Per-project results keyed by project ID
    pub projects: HashMap<String, BuildResult>,
    /// Overall success (all projects succeeded)
    pub all_succeeded: bool,
    /// Total duration across all projects
    pub total_duration: Duration,
    /// Total number of parallel jobs used
    pub parallel_jobs: usize,
}

// =============================================================================
// Build configuration
// =============================================================================

/// Parameters for a build request.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct BuildRequest {
    /// Override the default build command
    #[serde(default)]
    pub command: Option<String>,
    /// Additional arguments to pass to the build command
    #[serde(default)]
    pub args: Vec<String>,
    /// Target project ID in a multi-project workspace (empty = active project)
    #[serde(default)]
    pub project_id: Option<String>,
    /// Whether to run in release mode
    #[serde(default)]
    pub release: bool,
    /// Enable verbose/detailed output
    #[serde(default)]
    pub verbose: bool,
    /// Specific target to build (e.g., binary name, package)
    #[serde(default)]
    pub target: Option<String>,
    /// Additional environment variables for this build
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Whether to clean before building
    #[serde(default)]
    pub clean: bool,
    /// Maximum number of parallel jobs (for supported build systems)
    #[serde(default)]
    pub jobs: Option<usize>,
    /// Enable or disable incremental compilation
    #[serde(default)]
    pub incremental: Option<bool>,
}

/// Language-specific build profile.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct BuildProfile {
    /// Display name for this profile
    pub name: String,
    /// Custom build command override
    pub command: Option<String>,
    /// Default arguments
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variables
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Output directory
    pub output_dir: Option<String>,
}

// =============================================================================
// Build executor — core engine
// =============================================================================

/// The main build engine that executes builds across languages and projects.
pub struct BuildExecutor {
    workspace: Arc<WorkspaceManager>,
}

impl BuildExecutor {
    /// Create a new build executor bound to a workspace.
    pub fn new(workspace: Arc<WorkspaceManager>) -> Self {
        Self { workspace }
    }

    // === Single Project Builds ===

    /// Execute a build for a specific project type in the given directory.
    pub async fn build_project(
        &self,
        project_type: &ProjectType,
        work_dir: &Path,
        request: &BuildRequest,
    ) -> Result<BuildResult> {
        let start = Instant::now();

        // Resolve the actual command to run
        let cmd_str = self.resolve_command(project_type, request).await?;
        let (program, args) = self.parse_command_line(&cmd_str, project_type, request)?;

        tracing::info!(
            project_type = ?project_type,
            dir = %work_dir.display(),
            cmd = %cmd_str,
            "Executing build"
        );

        // Execute the command
        let mut cmd = tokio::process::Command::new(&program);
        cmd.args(&args)
            .current_dir(work_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Merge environment: process env + request env + workspace env
        let resolved_env = self.workspace.resolved_env().await;
        for (k, v) in std::env::vars() {
            cmd.env(&k, &v);
        }
        for (k, v) in &request.env {
            cmd.env(k, v);
        }
        for (k, v) in &resolved_env {
            cmd.env(k, v);
        }

        // Handle clean-first
        if request.clean {
            if let Some(clean_cmd) = self.resolve_clean_command(project_type, work_dir) {
                let (clean_prog, clean_args) =
                    self.parse_command_parts(&clean_cmd).unwrap_or_else(|| ("echo".into(), vec!["no clean".into()]));
                let _ = tokio::process::Command::new(clean_prog)
                    .args(&clean_args)
                    .current_dir(work_dir)
                    .output()
                    .await;
            }
        }

        let output = cmd.output().await.with_context(|| {
            format!("Failed to execute build command: {} {}", program, args.join(" "))
        })?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        // Interleave output for combined display
        let combined = self.interleave_output(&stdout, &stderr);

        let success = output.status.success();
        let exit_code = output.status.code();
        let duration = start.elapsed();

        // Parse diagnostics (errors/warnings)
        let (error_count, warning_count) = self.parse_diagnostics(project_type, &combined);

        // Detect output artifacts
        let artifacts = self.detect_artifacts(project_type, work_dir).unwrap_or_default();

        Ok(BuildResult {
            success,
            exit_code,
            stdout,
            stderr,
            output: combined,
            duration,
            warning_count,
            error_count,
            project_type: project_type.clone(),
            build_dir: work_dir.to_path_buf(),
            artifacts,
        })
    }

    /// Build the currently active project in the workspace.
    pub async fn build_active_project(&self, request: &BuildRequest) -> Result<BuildResult> {
        let project = self
            .workspace
            .get_active_project()
            .await
            .context("No active project")?;

        self.build_project(&project.project_type, &project.root_path, request)
            .await
    }

    // === Multi-Project (Workspace) Builds ===

    /// Build all projects in the workspace.
    pub async fn build_all(
        &self,
        request: &BuildRequest,
        parallel: bool,
        max_jobs: usize,
    ) -> Result<WorkspaceBuildResult> {
        let projects = self.workspace.list_projects().await;
        let start = Instant::now();
        let mut results = HashMap::new();

        if parallel && max_jobs > 1 {
            // Parallel execution with semaphore
            use futures::stream::{self, StreamExt};
            let semaphore = Arc::new(tokio::sync::Semaphore::new(max_jobs));
            let mut stream = stream::iter(projects.into_iter().map(|proj| {
                let sem = semaphore.clone();
                let exec = &*self;
                let req = request.clone();
                async move {
                    let _permit = sem.acquire().await.unwrap();
                    let id = proj.id.clone();
                    let result = exec
                        .build_project(&proj.project_type, &proj.root_path, &req)
                        .await
                        .unwrap_or_else(|e| BuildResult::error(e.to_string(), proj.project_type, &proj.root_path));
                    (id, result)
                }
            }))
            .buffer_unordered(max_jobs);

            while let Some((id, result)) = stream.next().await {
                results.insert(id, result);
            }
        } else {
            // Sequential execution
            for proj in projects {
                let id = proj.id.clone();
                let result = self
                    .build_project(&proj.project_type, &proj.root_path, request)
                    .await
                    .unwrap_or_else(|e| {
                        BuildResult::error(e.to_string(), proj.project_type, &proj.root_path)
                    });
                results.insert(id, result);
            }
        }

        let all_ok = results.values().all(|r| r.success);
        let total_dur = start.elapsed();

        Ok(WorkspaceBuildResult {
            projects: results,
            all_succeeded: all_ok,
            total_duration: total_dur,
            parallel_jobs: if parallel { max_jobs } else { 1 },
        })
    }

    // === Command Resolution ===

    /// Determine the actual build command based on project type and overrides.
    async fn resolve_command(&self, project_type: &ProjectType, request: &BuildRequest) -> Result<String> {
        // 1. Explicit override in request
        if let Some(ref cmd) = request.command {
            return Ok(cmd.clone());
        }

        // 2. Check project's custom build config
        if let Some(proj) = self.workspace.get_active_project().await {
            if let Some(ref bc) = proj.build_config {
                if let Some(ref cmd) = bc.build_command {
                    return Ok(cmd.clone());
                }
            }
        }

        // 3. Fall back to project type default
        let mut cmd = project_type.default_build_command().to_string();

        // Apply common flags from request
        if request.release {
            match project_type {
                ProjectType::Rust | ProjectType::RustWorkspace => cmd += " --release",
                ProjectType::NodeJs | ProjectType::TypeScript | ProjectType::React | ProjectType::Vue | ProjectType::Angular => {}
                ProjectType::Go => cmd += " -ldflags '-s'",
                _ => {}
            }
        }

        if request.verbose {
            match project_type {
                ProjectType::Rust | ProjectType::RustWorkspace => cmd += " -v",
                _ => {}
            }
        }

        Ok(cmd)
    }

    /// Parse a command string into (program, args).
    fn parse_command_line(
        &self,
        cmd: &str,
        project_type: &ProjectType,
        request: &BuildRequest,
    ) -> Result<(String, Vec<String>)> {
        let mut parts: Vec<String> = cmd.split_whitespace().map(String::from).collect();

        if parts.is_empty() {
            bail!("Empty build command");
        }

        let program = parts.remove(0);

        // Append additional args from request
        parts.extend(request.args.iter().cloned());

        // Add target if specified
        if let Some(ref target) = request.target {
            match project_type {
                ProjectType::Rust | ProjectType::RustWorkspace => {
                    parts.push("--bin".into());
                    parts.push(target.clone());
                }
                ProjectType::Go => {
                    // go build ./... doesn't take target like this; skip
                }
                _ => {
                    parts.push(target.clone());
                }
            }
        }

        // Add parallel job flags
        if let Some(jobs) = request.jobs {
            if jobs > 1 {
                match project_type {
                    ProjectType::Rust | ProjectType::RustWorkspace => {
                        parts.push(format!("-j{}", jobs));
                    }
                    ProjectType::C | ProjectType::Cpp => {
                        parts.push(format!("-j{}", jobs));
                    }
                    _ => {}
                }
            }
        }

        // Incremental flag handling
        if let Some(incr) = request.incremental {
            match project_type {
                ProjectType::Rust | ProjectType::RustWorkspace => {
                    if !incr {
                        parts.push("--profile=dev-panic".into()); // Force full rebuild
                    }
                }
                _ => {}
            }
        }

        Ok((program, parts))
    }

    /// Split a simple command string into (program, args).
    fn parse_command_parts(&self, cmd: &str) -> Option<(String, Vec<String>)> {
        let mut parts: Vec<String> = cmd.split_whitespace().map(String::from).collect();
        if parts.is_empty() {
            return None;
        }
        let prog = parts.remove(0);
        Some((prog, parts))
    }

    /// Get the clean command for a project type.
    fn resolve_clean_command(&self, project_type: &ProjectType, _work_dir: &Path) -> Option<String> {
        Some(match project_type {
            ProjectType::Rust | ProjectType::RustWorkspace => "cargo clean".into(),
            ProjectType::NodeJs | ProjectType::TypeScript | ProjectType::React | ProjectType::Vue | ProjectType::Angular => {
                "npm run clean 2>/dev/null || rm -rf dist node_modules/.cache".into()
            }
            ProjectType::C | ProjectType::Cpp => "rm -rf build CMakeFiles".into(),
            ProjectType::Go => "go clean".into(),
            ProjectType::Java => "mvn clean".into(),
            ProjectType::Kotlin => "./gradlew clean".into(),
            ProjectType::Python => "rm -rf dist build *.egg-info".into(),
            ProjectType::CSharp => "dotnet clean".into(),
            _ => return None,
        })
    }

    // === Output Processing ===

    /// Interleave stdout and stderr for unified display.
    fn interleave_output(&self, stdout: &str, stderr: &str) -> String {
        // Simple approach: show stderr first (usually has errors), then stdout
        // A more sophisticated implementation would interleave by timestamp
        let mut out = String::new();
        if !stderr.trim().is_empty() {
            out.push_str("[stderr] ");
            out.push_str(stderr);
            out.push('\n');
        }
        if !stdout.trim().is_empty() {
            out.push_str(stdout);
            out.push('\n');
        }
        if out.is_empty() {
            out = "(no output)".into();
        }
        out
    }

    /// Parse compiler diagnostics (errors and warnings) from output.
    fn parse_diagnostics(&self, project_type: &ProjectType, output: &str) -> (usize, usize) {
        let lower = output.to_ascii_lowercase();
        let mut errors = 0usize;
        let mut warnings = 0usize;

        // Common patterns across compilers
        let error_patterns: &[&str] = match project_type {
            ProjectType::Rust | ProjectType::RustWorkspace => {
                &["error[E", "error: ", "could not compile", "cannot find"]
            }
            ProjectType::NodeJs | ProjectType::TypeScript | ProjectType::React | ProjectType::Vue | ProjectType::Angular => {
                &["error ts", "error:", "Cannot find module", "SyntaxError"]
            }
            ProjectType::C | ProjectType::Cpp => {
                &["error: ", "fatal error", "undefined reference", "linker error"]
            }
            ProjectType::Go => &[" error", " undefined:", " cannot find"],
            ProjectType::Python => &["error", "error:", "syntaxerror"],
            ProjectType::Java | ProjectType::Kotlin => &["error:", "ERROR", "[error]"],
            _ => &["error", "error:"],
        };

        let warning_patterns: &[&str] = match project_type {
            ProjectType::Rust | ProjectType::RustWorkspace => &["warning:", "warning["],
            ProjectType::NodeJs | ProjectType::TypeScript => &["warning ts", "warning("],
            ProjectType::C | ProjectType::Cpp => &["warning: "],
            ProjectType::Go => &[" warning"],
            ProjectType::Java | ProjectType::Kotlin => &["warning:", "WARNING", "[warn]"],
            _ => &["warning", "warning:"],
        };

        for pat in error_patterns {
            errors += lower.matches(pat).count();
        }
        for pat in warning_patterns {
            warnings += lower.matches(pat).count();
        }

        // Cap at reasonable numbers to avoid overcounting
        (errors.min(1000), warnings.min(10000))
    }

    /// Try to detect output artifacts after a successful build.
    fn detect_artifacts(
        &self,
        project_type: &ProjectType,
        work_dir: &Path,
    ) -> Result<Vec<PathBuf>> {
        let mut artifacts = Vec::new();

        let candidates = match project_type {
            ProjectType::Rust | ProjectType::RustWorkspace => vec![
                "target/release",
                "target/debug",
            ],
            ProjectType::NodeJs | ProjectType::TypeScript | ProjectType::React | ProjectType::Vue | ProjectType::Angular => {
                vec!["dist", "build", "out"]
            }
            ProjectType::Python => vec!["dist", "build"],
            ProjectType::Go => vec![], // Go outputs to cwd or specified -o
            ProjectType::C | ProjectType::Cpp => vec!["build", "bin", "Release", "Debug"],
            ProjectType::Java | ProjectType::Kotlin => vec!["target", "build", "out"],
            ProjectType::CSharp => vec!["bin", "release", "debug"],
            _ => return Ok(artifacts),
        };

        for candidate in candidates {
            let p = work_dir.join(candidate);
            if p.exists() {
                artifacts.push(p);
            }
        }

        Ok(artifacts)
    }
}

// =============================================================================
// Tool integration — exposes BuildExecutor as a callable tool for the agent
// =============================================================================

/// The `build` tool — allows the AI agent to invoke builds on any language.
pub struct BuildTool {
    executor: Arc<BuildExecutor>,
}

impl BuildTool {
    pub fn new(workspace: Arc<WorkspaceManager>) -> Self {
        Self {
            executor: Arc::new(BuildExecutor::new(workspace)),
        }
    }
}

#[async_trait]
impl Tool for BuildTool {
    fn name(&self) -> &str {
        "build"
    }

    fn description(&self) -> &str {
        r#"Build/compile the current project or specific target.

Supports multi-language compilation:
- Rust: cargo build [--workspace]
- Node.js/TypeScript: npm run build / tsc
- React/Vue/Angular: npm run build / ng build
- Python: python -m build
- Go: go build ./...
- C/C++: cmake --build / make
- Java: mvn package / gradle build
- C#: dotnet build
- Ruby: bundle exec rake build

In multi-project workspace mode, can build individual projects or all projects.
Detects and reports errors/warnings with structured output."#
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Override the default build command (e.g., 'cargo build --release', 'npm run build:prod')"
                },
                "args": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Additional arguments to pass to the build command"
                },
                "project_id": {
                    "type": "string",
                    "description": "Target project ID in workspace mode (omit for active project)"
                },
                "target": {
                    "type": "string",
                    "description": "Specific target to build (e.g., binary name, package)"
                },
                "release": {
                    "type": "boolean",
                    "description": "Build in release/optimize mode",
                    "default": false
                },
                "clean": {
                    "type": "boolean",
                    "description": "Clean build artifacts before building",
                    "default": false
                },
                "verbose": {
                    "type": "boolean",
                    "description": "Enable verbose output",
                    "default": false
                },
                "jobs": {
                    "type": "integer",
                    "description": "Number of parallel build jobs (for supported build systems)"
                },
                "all_projects": {
                    "type": "boolean",
                    "description": "Build all projects in the workspace",
                    "default": false
                },
                "parallel": {
                    "type": "boolean",
                    "description": "Build projects in parallel (only with all_projects)",
                    "default": false
                }
            },
            "required": []
        })
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        // Extract fields before consuming input
        let all_projects = input.get("all_projects").and_then(|v| v.as_bool()).unwrap_or(false);
        let parallel = input.get("parallel").and_then(|v| v.as_bool()).unwrap_or(false);
        let max_jobs = input.get("max_jobs")
            .and_then(|v| v.as_u64())
            .map(|j| j as usize)
            .unwrap_or(4);

        let request: BuildRequest = serde_json::from_value(input)?;

        // If a project_id is specified, try to switch context
        if let Some(ref pid) = request.project_id {
            // Set as active temporarily for the build
            if let Err(e) = self.executor.workspace.set_active_project(pid).await {
                return Ok(jcode_tool_types::ToolOutput {
                    output: format!(
                        "Warning: Could not set active project '{}': {}. Building current project.\n",
                        pid, e
                    ),
                    title: None,
                    metadata: None,
                    images: Vec::new(),
                });
            }
        }

        // Check if this is an all-projects build
        if all_projects {
            // Workspace-level build
            let result = self.executor.build_all(&request, parallel, max_jobs).await?;

            // Format workspace build report
            let mut lines = vec![
                format!("=== Workspace Build Summary ({}) ===\n",
                    if result.all_succeeded { "SUCCESS" } else { "FAILED" }),
                format!("Total duration: {:.2}s\n", result.total_duration.as_secs_f32()),
                format!("Projects built: {}\n", result.projects.len()),
                String::from("\n"),
            ];

            for (pid, br) in &result.projects {
                let status = if br.success { "OK" } else { "FAILED" };
                lines.push(format!(
                    "  [{}] {} - {:.1}s ({} err, {} warn)\n",
                    status, pid, br.duration.as_secs_f32(), br.error_count, br.warning_count
                ));
                if !br.stdout.is_empty() || !br.stderr.is_empty() {
                    lines.push(format!("--- {} output ---\n{}\n", pid, &br.output[..br.output.len().min(4000)]));
                }
            }

            let text = lines.join("");
            return Ok(jcode_tool_types::ToolOutput {
                output: text,
                title: None,
                metadata: None,
                images: Vec::new(),
            });
        }

        // Single project build
        let result = self.executor.build_active_project(&request).await?;

        let mut output = String::new();
        output.push_str(&result.summary_line());
        output.push('\n');

        // Include full output if not too large (truncate very long outputs)
        if !result.output.is_empty() {
            let max_display = if result.success { 2000 } else { 8000 };
            if result.output.len() > max_display {
                output.push_str("--- Build Output (truncated) ---\n");
                output.push_str(&result.output[..max_display]);
                output.push_str(&format!("\n... [truncated, total {} bytes]\n", result.output.len()));
            } else {
                output.push_str("--- Build Output ---\n");
                output.push_str(&result.output);
            }
            output.push('\n');
        }

        // Show artifacts on success
        if result.success && !result.artifacts.is_empty() {
            output.push_str("--- Artifacts ---\n");
            for art in &result.artifacts {
                output.push_str(&format!("  {}\n", art.display()));
            }
        }

        Ok(jcode_tool_types::ToolOutput {
            output: output,
            title: None,
            metadata: None,
            images: Vec::new(),
        })
    }
}

// =============================================================================
// Test runner tool
// =============================================================================

/// The `test` tool — allows the AI agent to run tests for any language.
pub struct TestTool {
    executor: Arc<BuildExecutor>,
}

impl TestTool {
    pub fn new(workspace: Arc<WorkspaceManager>) -> Self {
        Self {
            executor: Arc::new(BuildExecutor::new(workspace)),
        }
    }
}

#[async_trait]
impl Tool for TestTool {
    fn name(&self) -> &str {
        "run_tests"
    }

    fn description(&self) -> &str {
        "Run tests for the current project. Auto-detects test framework based on project type (cargo test, npm test, pytest, go test, mvn test, etc.)"
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "filter": {
                    "type": "string",
                    "description": "Filter tests by name/pattern (e.g., 'my_module::*' for Rust, 'utils.*' for pytest)"
                },
                "args": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Additional test runner arguments"
                },
                "verbose": {
                    "type": "boolean",
                    "description": "Verbose test output",
                    "default": false
                },
                "project_id": {
                    "type": "string",
                    "description": "Target project ID in workspace mode"
                }
            },
            "required": []
        })
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        let filter: Option<String> = input.get("filter").and_then(|v| v.as_str()).map(String::from);
        let verbose: bool = input.get("verbose").and_then(|v| v.as_bool()).unwrap_or(false);
        let extra_args: Vec<String> = input
            .get("args")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let project = self.executor.workspace.get_active_project().await
            .context("No active project")?;

        let test_cmd = project.project_type.default_test_command();
        let mut parts: Vec<String> = test_cmd.split_whitespace().map(String::from).collect();

        // Apply filters and flags per language
        match project.project_type {
            ProjectType::Rust | ProjectType::RustWorkspace => {
                if let Some(ref f) = filter { parts.push(f.clone()); }
                if verbose { parts.push("--".into()); parts.push("--nocapture".into()); }
            }
            ProjectType::NodeJs | ProjectType::TypeScript | ProjectType::React | ProjectType::Vue | ProjectType::Angular => {
                if let Some(ref f) = filter {
                    parts.push("--".into());
                    parts.push(f.clone());
                }
                if verbose { parts.push("--verbose".into()); }
            }
            ProjectType::Python => {
                if let Some(ref f) = filter {
                    parts.push("-k".into());
                    parts.push(f.clone());
                }
                if verbose { parts.push("-v".into()); }
            }
            ProjectType::Go => {
                if let Some(ref f) = filter { parts.push("-run".into()); parts.push(f.clone()); }
                if verbose { parts.push("-v".into()); }
            }
            _ => {
                if let Some(ref f) = filter { parts.push(f.clone()); }
            }
        }

        parts.extend(extra_args);

        let program = parts.remove(0);

        tracing::info!(cmd = %test_cmd, "Running tests");

        let output = tokio::process::Command::new(&program)
            .args(&parts)
            .current_dir(&project.root_path)
            .envs(std::env::vars())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .with_context(|| format!("Failed to run test command: {}", test_cmd))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let success = output.status.success();

        let mut text = String::new();
        if success {
            text.push_str("Tests PASSED\n");
        } else {
            text.push_str("Tests FAILED\n");
        }
        text.push_str(&format!("Exit code: {:?}\n", output.status.code()));
        text.push('\n');
        if !stdout.is_empty() {
            text.push_str(&stdout);
            text.push('\n');
        }
        if !stderr.is_empty() {
            text.push_str(stderr.trim());
            text.push('\n');
        }

        Ok(jcode_tool_types::ToolOutput {
            output: text,
            title: None,
            metadata: None,
            images: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_build_result_summary() {
        let result = BuildResult {
            success: true,
            exit_code: Some(0),
            stdout: "Compiling...\nFinished.".into(),
            stderr: String::new(),
            output: "Compiling...\nFinished.".into(),
            duration: Duration::from_millis(1200),
            warning_count: 3,
            error_count: 0,
            project_type: ProjectType::Rust,
            build_dir: PathBuf::from("/tmp"),
            artifacts: vec![PathBuf::from("/tmp/target/release/myapp")],
        };
        let summary = result.summary_line();
        assert!(summary.contains("OK"));
        assert!(summary.contains("1.2"));
        assert!(summary.contains("3"));
    }

    #[test]
    fn test_parse_diagnostics_rust() {
        let executor = {
            let ws = WorkspaceManager::new();
            BuildExecutor::new(Arc::new(ws))
        };
        let output = "error[E0425]: cannot find value x\nwarning: unused variable y";
        let (errs, warns) = executor.parse_diagnostics(&ProjectType::Rust, output);
        assert!(errs >= 1);
        assert!(warns >= 1);
    }

    #[test]
    fn test_interleave_output() {
        let ws = WorkspaceManager::new();
        let executor = BuildExecutor::new(Arc::new(ws));

        let result = executor.interleave_output("hello stdout", "error msg");
        assert!(result.contains("stdout"));
        assert!(result.contains("error"));
    }
}
