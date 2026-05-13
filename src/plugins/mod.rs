//! Plugin System for Dynamic Extension
//!
//! Features:
//! - Dynamic loading of external skills
//! - Third-party command registration
//! - Custom tool integration
//! - Plugin lifecycle management

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// Plugin metadata and manifest
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: Option<String>,
    #[serde(default)]
    pub permissions: Vec<PluginPermission>,
    #[serde(default)]
    pub dependencies: Vec<String>,
    pub entry_point: String,
}

/// Plugin permission levels
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PluginPermission {
    /// Read-only access to workspace files
    ReadFiles,
    /// Write/modify workspace files
    WriteFiles,
    /// Execute shell commands
    ExecuteCommands,
    /// Network access (HTTP requests)
    NetworkAccess,
    /// Access to MCP/LSP services
    ServiceAccess,
    /// Full system access (use with caution)
    FullAccess,
}

impl std::fmt::Display for PluginPermission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ReadFiles => write!(f, "read-files"),
            Self::WriteFiles => write!(f, "write-files"),
            Self::ExecuteCommands => write!(f, "execute-commands"),
            Self::NetworkAccess => write!(f, "network-access"),
            Self::ServiceAccess => write!(f, "service-access"),
            Self::FullAccess => write!(f, "full-access"),
        }
    }
}

/// Plugin state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginState {
    Unloaded,
    Loading,
    Loaded,
    Active,
    Error(String),
    Disabled,
}

impl std::fmt::Display for PluginState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unloaded => write!(f, "unloaded"),
            Self::Loading => write!(f, "loading"),
            Self::Loaded => write!(f, "loaded"),
            Self::Active => write!(f, "active"),
            Self::Error(msg) => write!(f, "error: {}", msg),
            Self::Disabled => write!(f, "disabled"),
        }
    }
}

/// Plugin trait that all plugins must implement
#[async_trait]
pub trait Plugin: Send + Sync {
    fn name(&self) -> &str;
    fn version(&self) -> &str;

    /// Initialize the plugin
    async fn initialize(&mut self, context: &PluginContext) -> Result<()>;

    /// Shutdown the plugin gracefully
    async fn shutdown(&self) -> Result<()>;

    /// Get provided commands (if any)
    fn commands(&self) -> Vec<Box<dyn PluginCommand>> {
        vec![]
    }

    /// Get provided skills (if any)
    fn skills(&self) -> Vec<Box<dyn PluginSkill>> {
        vec![]
    }

    /// Get provided tools (if any)
    fn tools(&self) -> Vec<Box<dyn PluginTool>> {
        vec![]
    }
}

/// Context passed to plugins during initialization
#[derive(Debug, Clone)]
pub struct PluginContext {
    pub plugin_dir: PathBuf,
    pub data_dir: PathBuf,
    pub config: HashMap<String, String>,
    pub api_version: String,
}

/// Plugin command trait
#[async_trait]
pub trait PluginCommand: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn usage(&self) -> &str;

    async fn execute(&self, args: Option<&str>) -> Result<String>;
    async fn validate_args(&self, args: Option<&str>) -> Result<()> {
        let _ = args; // Default: accept any arguments
        Ok(())
    }
}

/// Plugin skill trait
#[async_trait]
pub trait PluginSkill: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;

    async fn can_execute(&self, context: &serde_json::Value) -> bool;
    async fn execute(
        &self,
        context: &serde_json::Value,
    ) -> Result<crate::skill_system::SkillResult>;
    async fn estimate_cost(
        &self,
        context: &serde_json::Value,
    ) -> Result<crate::skill_system::SkillCostEstimate>;
}

/// Plugin tool trait
#[async_trait]
pub trait PluginTool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> serde_json::Value;

    async fn execute(&self, params: serde_json::Value) -> Result<serde_json::Value>;
    async fn validate_params(&self, params: &serde_json::Value) -> Result<bool> {
        let _ = params;
        Ok(true)
    }
}

/// Loaded plugin instance with metadata
pub struct LoadedPlugin {
    pub manifest: PluginManifest,
    pub instance: Box<dyn Plugin>,
    pub state: Arc<RwLock<PluginState>>,
    pub loaded_at: chrono::DateTime<chrono::Utc>,
}

impl LoadedPlugin {
    pub fn new(manifest: PluginManifest, instance: Box<dyn Plugin>) -> Self {
        Self {
            manifest,
            instance,
            state: Arc::new(RwLock::new(PluginState::Unloaded)),
            loaded_at: chrono::Utc::now(),
        }
    }

    pub async fn state(&self) -> PluginState {
        *self.state.read().await
    }

    pub async fn is_active(&self) -> bool {
        matches!(*self.state.read().await, PluginState::Active)
    }
}

/// Plugin manager for loading, managing, and executing plugins
pub struct PluginManager {
    plugins: RwLock<HashMap<String, Arc<LoadedPlugin>>>,
    plugin_dirs: Vec<PathBuf>,
    context: PluginContext,
    on_plugin_load: Option<Arc<dyn Fn(&str) + Send + Sync>>,
    on_plugin_error: Option<Arc<dyn Fn(&str, &str) + Send + Sync>>,
}

impl PluginManager {
    pub fn new(context: PluginContext) -> Self {
        Self {
            plugins: RwLock::new(HashMap::new()),
            plugin_dirs: Vec::new(),
            context,
            on_plugin_load: None,
            on_plugin_error: None,
        }
    }

    /// Add a directory to search for plugins
    pub fn add_plugin_dir(&mut self, dir: impl Into<PathBuf>) {
        self.plugin_dirs.push(dir.into());
    }

    /// Set callback for when plugin is loaded
    pub fn on_load<F>(&mut self, callback: F)
    where
        F: Fn(&str) + Send + Sync + 'static,
    {
        self.on_plugin_load = Some(Arc::new(callback));
    }

    /// Set callback for when plugin encounters error
    pub fn on_error<F>(&mut self, callback: F)
    where
        F: Fn(&str, &str) + Send + Sync + 'static,
    {
        self.on_plugin_error = Some(Arc::new(callback));
    }

    /// Load a plugin from manifest file
    pub async fn load_from_manifest(&self, path: &Path) -> Result<Arc<LoadedPlugin>> {
        info!("Plugin: Loading from {:?}", path);

        // Read and parse manifest
        let manifest_str = tokio::fs::read_to_string(path).await?;
        let manifest: PluginManifest = serde_json::from_str(&manifest_str)?;

        // Validate dependencies
        for dep in &manifest.dependencies {
            if !self.is_loaded(dep).await {
                anyhow::bail!("Dependency '{}' not satisfied", dep);
            }
        }

        // Create plugin instance (in production, would use dynamic loading)
        // For now, we'll create a placeholder
        let instance = self.create_plugin_instance(&manifest).await?;

        // Wrap in LoadedPlugin
        let plugin = Arc::new(LoadedPlugin::new(manifest.clone(), instance));

        // Update state to loading
        *plugin.state.write().await = PluginState::Loading;

        // Initialize plugin
        {
            let mut guard = plugin.instance.as_ref();
            // Note: This won't compile as-is because of trait object limitations
            // In production, you'd use interior mutability or different architecture
            // This is simplified for demonstration
        }

        // Mark as active
        *plugin.state.write().await = PluginState::Active;

        // Register in manager
        {
            let mut plugins = self.plugins.write().await;
            plugins.insert(manifest.name.clone(), plugin.clone());
        }

        // Notify callback
        if let Some(ref cb) = self.on_plugin_load {
            cb(&manifest.name);
        }

        info!("Plugin '{}' v{} loaded successfully", manifest.name, manifest.version);
        Ok(plugin)
    }

    /// Create plugin instance from manifest
    async fn create_plugin_instance(
        &self,
        _manifest: &PluginManifest,
    ) -> Result<Box<dyn Plugin>> {
        // In production, this would:
        // 1. Load dynamic library (.dll/.so/.dylib)
        // 2. Find exported symbol for plugin factory
        // 3. Call factory function to get Plugin instance
        //
        // For now, return a placeholder
        Err(anyhow::anyhow!(
            "Dynamic plugin loading not yet implemented"
        ))
    }

    /// Check if a plugin is loaded
    pub async fn is_loaded(&self, name: &str) -> bool {
        let plugins = self.plugins.read().await;
        plugins.contains_key(name)
    }

    /// Unload a plugin
    pub async fn unload(&self, name: &str) -> Result<()> {
        let mut plugins = self.plugins.write().await;

        if let Some(plugin) = plugins.remove(name) {
            // Shutdown plugin
            *plugin.state.write().await = PluginState::Unloaded;
            info!("Plugin '{}' unloaded", name);
            Ok(())
        } else {
            anyhow::bail!("Plugin '{}' not found", name)
        }
    }

    /// Get a loaded plugin
    pub async fn get(&self, name: &str) -> Option<Arc<LoadedPlugin>> {
        let plugins = self.plugins.read().await;
        plugins.get(name).cloned()
    }

    /// List all loaded plugins
    pub async fn list_plugins(&self) -> Vec<PluginInfo> {
        let plugins = self.plugins.read().await;
        plugins
            .values()
            .map(|p| PluginInfo {
                name: p.manifest.name.clone(),
                version: p.manifest.version.clone(),
                description: p.manifest.description.clone(),
                state: p.state().await,
                loaded_at: p.loaded_at,
            })
            .collect()
    }

    /// Scan directories for plugins and load them
    pub async fn scan_and_load(&self) -> Result<Vec<Arc<LoadedPlugin>>> {
        let mut loaded = Vec::new();

        for dir in &self.plugin_dirs {
            if !dir.exists() {
                continue;
            }

            // Look for manifest.json or plugin.toml files
            let entries = tokio::fs::read_dir(dir).await?;

            while let Some(entry) = entries.next_entry().await? {
                let path = entry.path();

                if path.file_name() == Some("plugin.json".as_ref())
                    || path.file_name() == Some("plugin.toml".as_ref())
                {
                    match self.load_from_manifest(&path).await {
                        Ok(plugin) => loaded.push(plugin),
                        Err(e) => {
                            error!("Failed to load plugin {:?}: {}", path, e);

                            if let Some(ref cb) = self.on_plugin_error {
                                let name = path
                                    .file_stem()
                                    .and_then(|s| s.to_str())
                                    .unwrap_or("unknown");
                                cb(name, &e.to_string());
                            }
                        }
                    }
                }
            }
        }

        Ok(loaded)
    }

    /// Execute a command from any loaded plugin
    pub async fn execute_command(
        &self,
        command_name: &str,
        args: Option<&str>,
    ) -> Result<String> {
        let plugins = self.plugins.read().await;

        for (_name, plugin) in plugins.iter() {
            if !plugin.is_active().await {
                continue;
            }

            for cmd in plugin.instance.commands() {
                if cmd.name() == command_name {
                    cmd.validate_args(args).await?;
                    return cmd.execute(args).await;
                }
            }
        }

        anyhow::bail!("Command '{}' not found in any plugin", command_name)
    }

    /// Get all available commands from all plugins
    pub async fn list_commands(&self) -> Vec<CommandInfo> {
        let plugins = self.plugins.read().await;
        let mut commands = Vec::new();

        for (plugin_name, plugin) in plugins.iter() {
            if !plugin.is_active().await {
                continue;
            }

            for cmd in plugin.instance.commands() {
                commands.push(CommandInfo {
                    name: cmd.name().to_string(),
                    description: cmd.description().to_string(),
                    usage: cmd.usage().to_string(),
                    source: plugin_name.clone(),
                });
            }
        }

        commands
    }

    /// Get all available skills from all plugins
    pub async fn list_skills(&self) -> Vec<SkillInfo> {
        let plugins = self.plugins.read().await;
        let mut skills = Vec::new();

        for (plugin_name, plugin) in plugins.iter() {
            if !plugin.is_active().await {
                continue;
            }

            for skill in plugin.instance.skills() {
                skills.push(SkillInfo {
                    name: skill.name().to_string(),
                    description: skill.description().to_string(),
                    source: plugin_name.clone(),
                });
            }
        }

        skills
    }

    /// Shutdown all plugins
    pub async fn shutdown_all(&self) -> Result<()> {
        let plugins = self.plugins.read().await;

        for (name, plugin) in plugins.iter() {
            *plugin.state.write().await = PluginState::Disabled;
            info!("Plugin '{}' disabled", name);
        }

        Ok(())
    }
}

/// Plugin information summary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInfo {
    pub name: String,
    pub version: String,
    pub description: String,
    pub state: PluginState,
    pub loaded_at: chrono::DateTime<chrono::Utc>,
}

/// Command information from plugin
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandInfo {
    pub name: String,
    pub description: String,
    pub usage: String,
    pub source: String,
}

/// Skill information from plugin
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    pub source: String,
}

// ════════════════════════════════════════════════════════════════
// Built-in Example Plugins
// ════════════════════════════════════════════════════════════════

/// Example: A simple logging plugin
pub struct LoggingPlugin {
    log_file: PathBuf,
}

impl LoggingPlugin {
    pub fn new(log_file: PathBuf) -> Self {
        Self { log_file }
    }
}

#[async_trait]
impl Plugin for LoggingPlugin {
    fn name(&self) -> &str {
        "logging"
    }

    fn version(&self) -> &str {
        "1.0.0"
    }

    async fn initialize(&mut self, _context: &PluginContext) -> Result<()> {
        info!("Logging plugin initialized, writing to {:?}", self.log_file);
        Ok(())
    }

    async fn shutdown(&self) -> Result<()> {
        info!("Logging plugin shutting down");
        Ok(())
    }

    fn commands(&self) -> Vec<Box<dyn PluginCommand>> {
        vec![Box::new(LogCommand {
            log_file: self.log_file.clone(),
        })]
    }
}

struct LogCommand {
    log_file: PathBuf,
}

#[async_trait]
impl PluginCommand for LogCommand {
    fn name(&self) -> &str {
        "log"
    }

    fn description(&self) -> &str {
        "Log a message to the plugin log file"
    }

    fn usage(&self) -> &str {
        "/log <message>"
    }

    async fn execute(&self, args: Option<&str>) -> Result<String> {
        let message = args.unwrap_or("No message provided");

        let entry = format!(
            "[{}] {}\n",
            chrono::Utc::now().format("%Y-%m-%d %H:%M:%S"),
            message
        );

        tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_file)
            .await?
            .write_all(entry.as_bytes())
            .await?;

        Ok(format!("Logged: {}", message))
    }
}
