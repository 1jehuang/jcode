//! Multi-project workspace management for jcode.
//!
//! This module provides workspace-aware project management, allowing jcode to
//! work with multiple projects simultaneously within a unified workspace context.
//! It supports:
//! - Project registration and discovery
//! - Active project switching
//! - Cross-project dependency tracking
//! - Workspace-scoped configuration
//! - Project-specific build environments

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Represents a single project within a workspace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    /// Unique project identifier (e.g., "my-app", "backend-api")
    pub id: String,
    /// Human-readable project name
    pub name: String,
    /// Absolute path to the project root directory
    pub root_path: PathBuf,
    /// Project type/language classification
    pub project_type: ProjectType,
    /// Optional description of the project
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Dependencies on other projects in this workspace (by project ID)
    #[serde(default)]
    pub dependencies: Vec<String>,
    /// Project-specific environment variables
    #[serde(default)]
    pub env_vars: HashMap<String, String>,
    /// Whether this project is currently active/focused
    #[serde(default)]
    pub active: bool,
    /// Build system configuration for this project
    #[serde(default)]
    pub build_config: Option<ProjectBuildConfig>,
    /// Last activity timestamp (ISO 8601)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_active: Option<String>,
    /// Git remote URL if available
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_remote: Option<String>,
    /// Custom tags for organization
    #[serde(default)]
    pub tags: Vec<String>,
}

impl Project {
    /// Create a new project with the given parameters.
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        root_path: impl AsRef<Path>,
        project_type: ProjectType,
    ) -> Self {
        let root = root_path.as_ref().to_path_buf();
        Self {
            id: id.into(),
            name: name.into(),
            root_path: root,
            project_type,
            description: None,
            dependencies: Vec::new(),
            env_vars: HashMap::new(),
            active: false,
            build_config: None,
            last_active: None,
            git_remote: None,
            tags: Vec::new(),
        }
    }

    /// Check if the project root path exists on disk.
    pub fn exists(&self) -> bool {
        self.root_path.exists()
    }

    /// Get the project's Cargo.toml path if it's a Rust project.
    pub fn cargo_toml_path(&self) -> Option<PathBuf> {
        matches!(self.project_type, ProjectType::Rust | ProjectType::RustWorkspace)
            .then(|| self.root_path.join("Cargo.toml"))
            .filter(|p| p.exists())
    }

    /// Get the project's package.json path if it's a Node.js/TypeScript project.
    pub fn package_json_path(&self) -> Option<PathBuf> {
        matches!(
            self.project_type,
            ProjectType::NodeJs | ProjectType::TypeScript | ProjectType::React
                | ProjectType::Vue | ProjectType::Angular
        )
        .then(|| self.root_path.join("package.json"))
        .filter(|p| p.exists())
    }

    /// Get the project's CMakeLists.txt path if it's a C/C++ project with CMake.
    pub fn cmake_path(&self) -> Option<PathBuf> {
        matches!(self.project_type, ProjectType::C | ProjectType::Cpp)
            .then(|| self.root_path.join("CMakeLists.txt"))
            .filter(|p| p.exists())
    }

    /// Get the project's go.mod path if it's a Go project.
    pub fn go_mod_path(&self) -> Option<PathBuf> {
        matches!(self.project_type, ProjectType::Go)
            .then(|| self.root_path.join("go.mod"))
            .filter(|p| p.exists())
    }

    /// Update the last active timestamp to now.
    pub fn touch(&mut self) {
        self.last_active = Some(chrono::Utc::now().to_rfc3339());
    }
}

/// Classification of project types/languages.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ProjectType {
    /// Rust project (single crate)
    Rust,
    /// Rust workspace (multi-crate)
    RustWorkspace,
    /// Node.js/JavaScript project
    NodeJs,
    /// TypeScript project
    TypeScript,
    /// React frontend application
    React,
    /// Vue.js frontend application
    Vue,
    /// Angular frontend application
    Angular,
    /// Python project
    Python,
    /// Go project
    Go,
    /// C project
    C,
    /// C++ project
    Cpp,
    /// Java project (Maven/Gradle)
    Java,
    /// Kotlin project
    Kotlin,
    /// Ruby/Rails project
    Ruby,
    /// C# / .NET project
    CSharp,
    /// Generic/unclassified project
    #[default]
    Generic,
}

impl ProjectType {
    /// All known project type variants as string slices for display/config.
    pub fn all() -> &'static [&'static str] {
        &[
            "rust",
            "rust_workspace",
            "nodejs",
            "typescript",
            "react",
            "vue",
            "angular",
            "python",
            "go",
            "c",
            "cpp",
            "java",
            "kotlin",
            "ruby",
            "csharp",
            "generic",
        ]
    }

    /// Parse from string, returning None for unknown values.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "rust" => Some(Self::Rust),
            "rustworkspace" | "rust-workspace" | "rust_workspace" => Some(Self::RustWorkspace),
            "nodejs" | "node" | "javascript" | "js" => Some(Self::NodeJs),
            "typescript" | "ts" => Some(Self::TypeScript),
            "react" | "reactjs" => Some(Self::React),
            "vue" | "vuejs" => Some(Self::Vue),
            "angular" | "angularjs" | "ng" => Some(Self::Angular),
            "python" | "py" => Some(Self::Python),
            "go" | "golang" => Some(Self::Go),
            "c" => Some(Self::C),
            "cpp" | "c++" | "cplusplus" => Some(Self::Cpp),
            "java" => Some(Self::Java),
            "kotlin" | "kt" => Some(Self::Kotlin),
            "ruby" | "rails" => Some(Self::Ruby),
            "csharp" | "c#" | ".net" | "dotnet" => Some(Self::CSharp),
            _ => Some(Self::Generic),
        }
    }

    /// Detect project type from filesystem heuristics.
    pub fn detect_from_path(path: &Path) -> Self {
        if !path.is_dir() {
            return Self::Generic;
        }

        // Check for Rust workspace (Cargo.toml with [workspace])
        if path.join("Cargo.toml").exists() {
            if let Ok(content) = std::fs::read_to_string(path.join("Cargo.toml"))
                && (content.contains("[workspace]") || content.contains("[workspace.members]")) {
                    return Self::RustWorkspace;
                }
            return Self::Rust;
        }

        // Check for Go module
        if path.join("go.mod").exists() {
            return Self::Go;
        }

        // Check for Python
        if path.join("pyproject.toml").exists()
            || path.join("setup.py").exists()
            || path.join("requirements.txt").exists()
        {
            return Self::Python;
        }

        // Check for Node.js/TypeScript/React/Vue/Angular
        if path.join("package.json").exists() {
            if let Ok(content) = std::fs::read_to_string(path.join("package.json")) {
                let lower = content.to_ascii_lowercase();
                if lower.contains("\"react\"") || lower.contains("@vitejs/plugin-react") {
                    return Self::React;
                }
                if lower.contains("\"vue\"") || lower.contains("@vitejs/plugin-vue") {
                    return Self::Vue;
                }
                if lower.contains("\"@angular")
                    || lower.contains("\"angular-core\"")
                    || lower.contains("\"@angular/core\"")
                {
                    return Self::Angular;
                }
                if lower.contains("\"typescript\"") || path.join("tsconfig.json").exists() {
                    return Self::TypeScript;
                }
            }
            return Self::NodeJs;
        }

        // Check for Java/Kotlin
        if path.join("pom.xml").exists() || path.join("build.gradle").exists() || path.join("build.gradle.kts").exists() {
            if path.join("build.gradle.kts").exists() {
                return Self::Kotlin;
            }
            return Self::Java;
        }

        // Check for CMake C/C++
        if path.join("CMakeLists.txt").exists() {
            if path.join("*.cpp").exists() || glob_matches(path, "**/*.cpp") {
                return Self::Cpp;
            }
            return Self::C;
        }

        // Check for C#/.NET
        if path.join("*.csproj").exists() || glob_matches(path, "**/*.csproj") {
            return Self::CSharp;
        }

        // Check for Ruby
        if path.join("Gemfile").exists() {
            return Self::Ruby;
        }

        Self::Generic
    }

    /// Get the default build command for this project type.
    pub fn default_build_command(&self) -> &'static str {
        match self {
            Self::Rust => "cargo build",
            Self::RustWorkspace => "cargo build --workspace",
            Self::NodeJs => "npm run build",
            Self::TypeScript => "tsc --build",
            Self::React => "npm run build",
            Self::Vue => "npm run build",
            Self::Angular => "ng build",
            Self::Python => "python -m build",
            Self::Go => "go build ./...",
            Self::C => "make",
            Self::Cpp => "cmake --build build",
            Self::Java => "mvn package",
            Self::Kotlin => "./gradlew build",
            Self::Ruby => "bundle exec rake build",
            Self::CSharp => "dotnet build",
            Self::Generic => "echo 'No default build command'",
        }
    }

    /// Get the default test command for this project type.
    pub fn default_test_command(&self) -> &'static str {
        match self {
            Self::Rust | Self::RustWorkspace => "cargo test",
            Self::NodeJs | Self::TypeScript | Self::React | Self::Vue | Self::Angular => {
                "npm test"
            }
            Self::Python => "pytest",
            Self::Go => "go test ./...",
            Self::C | Self::Cpp => "ctest --test-dir build",
            Self::Java => "mvn test",
            Self::Kotlin => "./gradlew test",
            Self::Ruby => "bundle exec rspec",
            Self::CSharp => "dotnet test",
            Self::Generic => "echo 'No default test command'",
        }
    }

    /// Get the default run command for this project type.
    pub fn default_run_command(&self) -> &'static str {
        match self {
            Self::Rust => "cargo run",
            Self::RustWorkspace => "cargo run --bin <binary>",
            Self::NodeJs => "npm start",
            Self::TypeScript => "ts-node index.ts",
            Self::React => "npm start",
            Self::Vue => "npm run dev",
            Self::Angular => "ng serve",
            Self::Python => "python main.py",
            Self::Go => "run .",
            Self::C => "./<binary>",
            Self::Cpp => "./<binary>",
            Self::Java => "java -jar target/*.jar",
            Self::Kotlin => "./gradlew run",
            Self::Ruby => "ruby main.rb",
            Self::CSharp => "dotnet run",
            Self::Generic => "echo 'No default run command'",
        }
    }
}

/// Simple glob matcher for common patterns (no regex dependency).
fn glob_matches(base: &Path, pattern: &str) -> bool {
    use std::fs;
    if let Ok(entries) = fs::read_dir(base) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if pattern.contains("*.cpp") && name.ends_with(".cpp") {
                return true;
            }
            if pattern.contains("*.csproj") && name.ends_with(".csproj") {
                return true;
            }
        }
    }
    false
}

/// Build system configuration for a specific project.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectBuildConfig {
    /// Custom build command (overrides project type default)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build_command: Option<String>,
    /// Custom test command
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test_command: Option<String>,
    /// Custom run command
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_command: Option<String>,
    /// Additional build arguments
    #[serde(default)]
    pub build_args: Vec<String>,
    /// Environment variables specifically for builds
    #[serde(default)]
    pub build_env: HashMap<String, String>,
    /// Output directory relative to project root
    #[serde(default)]
    pub output_dir: Option<String>,
    /// Whether to enable incremental compilation
    #[serde(default = "default_true")]
    pub incremental: bool,
}

fn default_true() -> bool {
    true
}

impl Default for ProjectBuildConfig {
    fn default() -> Self {
        Self {
            build_command: None,
            test_command: None,
            run_command: None,
            build_args: Vec::new(),
            build_env: HashMap::new(),
            output_dir: None,
            incremental: true,
        }
    }
}

/// The full workspace containing all registered projects.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceConfig {
    /// Workspace name
    pub name: String,
    /// Absolute path to the workspace root
    pub workspace_root: PathBuf,
    /// All registered projects keyed by ID
    pub projects: HashMap<String, Project>,
    /// Currently active project ID (if any)
    #[serde(default)]
    pub active_project_id: Option<String>,
    /// Global workspace environment variables
    #[serde(default)]
    pub global_env: HashMap<String, String>,
    /// Workspace-level settings
    #[serde(default)]
    pub settings: WorkspaceSettings,
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        Self {
            name: "default-workspace".into(),
            workspace_root: PathBuf::from("."),
            projects: HashMap::new(),
            active_project_id: None,
            global_env: HashMap::new(),
            settings: WorkspaceSettings::default(),
        }
    }
}

/// Workspace-level settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceSettings {
    /// Maximum number of projects allowed (0 = unlimited)
    #[serde(default = "default_max_projects")]
    pub max_projects: usize,
    /// Auto-detect new projects in workspace subdirectories
    #[serde(default = "default_true")]
    pub auto_discover: bool,
    /// Show cross-project dependency warnings
    #[serde(default = "default_true")]
    pub dependency_warnings: bool,
    /// Enable parallel builds across projects
    #[serde(default)]
    pub parallel_builds: bool,
    /// Max parallel build jobs
    #[serde(default = "default_parallel_jobs")]
    pub max_parallel_jobs: usize,
}

impl Default for WorkspaceSettings {
    fn default() -> Self {
        Self {
            max_projects: default_max_projects(),
            auto_discover: true,
            dependency_warnings: true,
            parallel_builds: false,
            max_parallel_jobs: default_parallel_jobs(),
        }
    }
}

fn default_max_projects() -> usize {
    20
}
fn default_parallel_jobs() -> usize {
    4
}

/// The main workspace manager — thread-safe, async-friendly container for multi-project state.
pub struct WorkspaceManager {
    config: RwLock<WorkspaceConfig>,
    workspace_file: Option<PathBuf>,
}

impl WorkspaceManager {
    /// Create a new empty workspace manager (in-memory only).
    pub fn new() -> Self {
        Self {
            config: RwLock::new(WorkspaceConfig::default()),
            workspace_file: None,
        }
    }

    /// Create or load workspace from a configuration file.
    pub async fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let config = if path.exists() {
            jcode_storage::read_json(&path)?
        } else {
            WorkspaceConfig {
                workspace_root: path.parent().unwrap_or(Path::new(".")).to_path_buf(),
                ..Default::default()
            }
        };
        Ok(Self {
            config: RwLock::new(config),
            workspace_file: Some(path),
        })
    }

    /// Save current workspace configuration to disk (if backed by file).
    pub async fn save(&self) -> Result<()> {
        let Some(ref path) = self.workspace_file else {
            return Ok(()); // In-memory only, no-op
        };
        let cfg = self.config.read().await;
        jcode_storage::write_json(path, &*cfg)?;
        Ok(())
    }

    // === Project Registration ===

    /// Register a new project into the workspace.
    pub async fn register_project(&self, mut project: Project) -> Result<String> {
        let mut cfg = self.config.write().await;

        if cfg.projects.len() >= cfg.settings.max_projects && cfg.settings.max_projects > 0 {
            anyhow::bail!(
                "Workspace has reached maximum project limit ({})",
                cfg.settings.max_projects
            );
        }

        if cfg.projects.contains_key(&project.id) {
            anyhow::bail!("Project '{}' already exists", project.id);
        }

        if !project.exists() {
            tracing::warn!("Registered project '{}' path does not exist: {:?}", project.id, project.root_path);
        }

        project.touch();
        let pid = project.id.clone();
        cfg.projects.insert(pid.clone(), project);

        // Auto-activate if no active project
        if cfg.active_project_id.is_none() {
            cfg.active_project_id = Some(pid.clone());
            if let Some(p) = cfg.projects.get_mut(&pid) {
                p.active = true;
            }
        }

        drop(cfg);
        self.save().await?;
        Ok(pid)
    }

    /// Remove a project from the workspace by ID.
    pub async fn remove_project(&self, project_id: &str) -> Result<bool> {
        let mut cfg = self.config.write().await;

        if cfg.projects.remove(project_id).is_none() {
            return Ok(false);
        }

        // If we removed the active project, pick another
        if cfg.active_project_id.as_deref() == Some(project_id) {
            let new_active_id = cfg.projects.keys().next().cloned();
            cfg.active_project_id = new_active_id.clone();
            for (_, p) in cfg.projects.iter_mut() {
                p.active = new_active_id.as_deref() == Some(p.id.as_str());
            }
        }

        // Clean up dependencies referencing removed project
        for p in cfg.projects.values_mut() {
            p.dependencies.retain(|dep| dep != project_id);
        }

        drop(cfg);
        self.save().await?;
        Ok(true)
    }

    // === Active Project ===

    /// Switch the active project to the given ID.
    pub async fn set_active_project(&self, project_id: &str) -> Result<()> {
        let mut cfg = self.config.write().await;

        if !cfg.projects.contains_key(project_id) {
            anyhow::bail!("Project '{}' not found in workspace", project_id);
        }

        // Deactivate previous
        let prev_id = cfg.active_project_id.clone();
        if let Some(ref pid) = prev_id
            && let Some(p) = cfg.projects.get_mut(pid) {
                p.active = false;
            }

        cfg.active_project_id = Some(project_id.to_string());
        if let Some(p) = cfg.projects.get_mut(project_id) {
            p.active = true;
            p.touch();
        }

        drop(cfg);
        self.save().await?;
        Ok(())
    }

    /// Get the currently active project.
    pub async fn get_active_project(&self) -> Option<Project> {
        let cfg = self.config.read().await;
        cfg.active_project_id
            .as_ref()
            .and_then(|id| cfg.projects.get(id))
            .cloned()
    }

    /// Get the active project's root path, or None if no project is active.
    pub async fn active_project_path(&self) -> Option<PathBuf> {
        self.get_active_project().await.map(|p| p.root_path)
    }

    /// Get the working directory for tool execution:
    /// prefers active project root, falls back to workspace root.
    pub async fn resolve_working_dir(&self, fallback: Option<&Path>) -> PathBuf {
        self.active_project_path()
            .await
            .or_else(|| fallback.map(|p| p.to_path_buf()))
            .unwrap_or_else(|| {
                // Try to read from config lock; fallback to cwd
                tokio::task::block_in_place(|| {
                    std::env::current_dir().unwrap_or_else(|_| ".".into())
                })
            })
    }

    // === Queries ===

    /// List all registered project IDs.
    pub async fn list_project_ids(&self) -> Vec<String> {
        let cfg = self.config.read().await;
        cfg.projects.keys().cloned().collect()
    }

    /// List all projects sorted by last active time.
    pub async fn list_projects(&self) -> Vec<Project> {
        let cfg = self.config.read().await;
        let mut projects: Vec<_> = cfg.projects.values().cloned().collect();
        projects.sort_by(|a, b| {
            b.last_active
                .as_deref()
                .cmp(&a.last_active.as_deref())
        });
        projects
    }

    /// Get a specific project by ID.
    pub async fn get_project(&self, id: &str) -> Option<Project> {
        let cfg = self.config.read().await;
        cfg.projects.get(id).cloned()
    }

    /// Find which project owns the given absolute path.
    pub async fn find_project_for_path(&self, path: &Path) -> Option<Project> {
        let cfg = self.config.read().await;
        cfg.projects
            .values()
            .find(|p| path.starts_with(&p.root_path))
            .cloned()
    }

    /// Count of registered projects.
    pub async fn project_count(&self) -> usize {
        let cfg = self.config.read().await;
        cfg.projects.len()
    }

    // === Discovery ===

    /// Auto-discover projects in subdirectories of the given base path.
    pub async fn discover_projects(&self, base: &Path) -> Result<Vec<Project>> {
        let mut discovered = Vec::new();

        let entries = std::fs::read_dir(base).with_context(|| format!("Cannot read directory {:?}", base))?;

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            // Skip hidden directories
            if path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|s| s.starts_with('.'))
            {
                continue;
            }

            let proj_type = ProjectType::detect_from_path(&path);
            if matches!(proj_type, ProjectType::Generic) {
                continue; // Skip unrecognized directories
            }

            let dir_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();

            let id = slugify(&dir_name);
            let project = Project::new(
                &id,
                &dir_name,
                &path,
                proj_type,
            );

            discovered.push(project);
        }

        Ok(discovered)
    }

    /// Run auto-discovery and register any newly found projects.
    pub async fn auto_discover_and_register(&self) -> Result<usize> {
        let cfg = self.config.read().await;
        let base = cfg.workspace_root.clone();
        let existing_ids: HashSet<String> = cfg.projects.keys().cloned().collect();
        let should_discover = cfg.settings.auto_discover;
        drop(cfg);

        if !should_discover {
            return Ok(0);
        }

        let discovered = self.discover_projects(&base).await?;
        let mut count = 0usize;

        for mut proj in discovered {
            if !existing_ids.contains(&proj.id) {
                proj.description = Some("Auto-discovered".into());
                if self.register_project(proj).await.is_ok() {
                    count += 1;
                } else {
                    // Already registered between check and insert — ignore
                }
            }
        }

        Ok(count)
    }

    // === Environment ===

    /// Merge global env + active project env into a single map.
    pub async fn resolved_env(&self) -> HashMap<String, String> {
        let cfg = self.config.read().await;
        let mut env = cfg.global_env.clone();

        if let Some(ref aid) = cfg.active_project_id
            && let Some(proj) = cfg.projects.get(aid) {
                env.extend(proj.env_vars.clone());
                // Add build env too
                if let Some(ref bc) = proj.build_config {
                    env.extend(bc.build_env.clone());
                }
            }

        env
    }

    // === Summary / Status ===

    /// Generate a human-readable summary of the workspace state.
    pub async fn summary(&self) -> WorkspaceSummary {
        let cfg = self.config.read().await;
        WorkspaceSummary {
            name: cfg.name.clone(),
            workspace_root: cfg.workspace_root.clone(),
            total_projects: cfg.projects.len(),
            active_project_id: cfg.active_project_id.clone(),
            active_project_name: cfg
                .active_project_id
                .as_ref()
                .and_then(|id| cfg.projects.get(id))
                .map(|p| p.name.clone()),
            project_types: cfg
                .projects
                .values()
                .map(|p| p.project_type.clone())
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect(),
        }
    }
}

use std::collections::HashSet;

/// Lightweight snapshot of workspace status for display/logging.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceSummary {
    pub name: String,
    pub workspace_root: PathBuf,
    pub total_projects: usize,
    pub active_project_id: Option<String>,
    pub active_project_name: Option<String>,
    pub project_types: Vec<ProjectType>,
}

impl std::fmt::Display for WorkspaceSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Workspace '{}'", self.name)?;
        write!(f, " [{} project(s)]", self.total_projects)?;
        if let Some(ref name) = self.active_project_name {
            write!(f, ", active: '{}'", name)?;
        }
        Ok(())
    }
}

/// Convert a string to a URL-safe slug.
fn slugify(input: &str) -> String {
    input
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else if c.is_whitespace() {
                '-'
            } else {
                '\0'
            }
        })
        .filter(|&c| c != '\0')
        .collect::<String>()
        .trim_matches('-')
        .replace("--+", "-")
}

/// Shared global workspace manager instance (lazy-initialized).
static GLOBAL_WORKSPACE: std::sync::OnceLock<Arc<WorkspaceManager>> =
    std::sync::OnceLock::new();

/// Initialize or get the global workspace manager.
pub fn init_global_workspace(manager: WorkspaceManager) -> Arc<WorkspaceManager> {
    GLOBAL_WORKSPACE
        .get_or_init(|| Arc::new(manager))
        .clone()
}

/// Access the global workspace manager. Returns None if not initialized.
pub fn global_workspace() -> Option<Arc<WorkspaceManager>> {
    GLOBAL_WORKSPACE.get().cloned()
}

// Re-export storage for internal use
mod jcode_storage {
    pub fn read_json<T: serde::de::DeserializeOwned>(path: &std::path::Path) -> anyhow::Result<T> {
        let data = std::fs::read(path)?;
        serde_json::from_slice(&data).map_err(anyhow::Error::from)
    }

    pub fn write_json<T: serde::Serialize>(path: &std::path::Path, value: &T) -> anyhow::Result<()> {
        let data = serde_json::to_string_pretty(value)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, data)?;
        Ok(())
    }

    pub fn ensure_dir(path: &std::path::Path) -> anyhow::Result<()> {
        std::fs::create_dir_all(path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_project_type_detection() {
        // Test that detection doesn't panic on missing dirs
        let pt = ProjectType::detect_from_path(Path::new("/nonexistent/path"));
        assert_eq!(pt, ProjectType::Generic);
    }

    #[test]
    fn test_project_type_parsing() {
        assert_eq!(ProjectType::parse("Rust"), Some(ProjectType::Rust));
        assert_eq!(ProjectType::parse("TypeScript"), Some(ProjectType::TypeScript));
        assert_eq!(ProjectType::parse("react"), Some(ProjectType::React));
        assert_eq!(ProjectType::parse("unknown_foo"), Some(ProjectType::Generic));
    }

    #[test]
    fn test_slugify() {
        assert_eq!(slugify("My Cool App"), "my-cool-app");
        assert_eq!(slugify("Backend API"), "backend-api");
        assert_eq!(slugify("  spaced  "), "spaced");
    }

    #[tokio::test]
    async fn test_register_and_activate_project() {
        let mgr = WorkspaceManager::new();

        let proj = Project::new("test-proj", "Test Project", "/tmp/test", ProjectType::Rust);
        let id = mgr.register_project(proj).await.unwrap();
        assert_eq!(id, "test-proj");

        let active = mgr.get_active_project().await.unwrap();
        assert_eq!(active.id, "test-proj");
        assert!(active.active);

        // Switch to nothingness should fail
        assert!(mgr.set_active_project("nonexistent").await.is_err());

        // Count
        assert_eq!(mgr.project_count().await, 1);
    }

    #[tokio::test]
    async fn test_remove_project() {
        let mgr = WorkspaceManager::new();
        let proj = Project::new("p1", "P1", "/tmp/p1", ProjectType::Python);
        mgr.register_project(proj).await.unwrap();

        assert!(mgr.remove_project("p1").await.unwrap());
        assert!(!mgr.remove_project("p1").await.unwrap()); // Already gone
        assert_eq!(mgr.project_count().await, 0);
    }

    #[tokio::test]
    async fn test_resolve_working_dir() {
        let mgr = WorkspaceManager::new();
        let proj = Project::new("wd-test", "WD Test", "/custom/path", ProjectType::Go);
        mgr.register_project(proj).await.unwrap();

        let wd = mgr.resolve_working_dir(None).await;
        assert_eq!(wd, PathBuf::from("/custom/path"));
    }
}
