use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: Option<String>,
    pub license: Option<String>,
    pub entry_point: String,
    pub dependencies: Vec<String>,
    pub capabilities: Vec<PluginCapability>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginCapability {
    Commands,
    Tools,
    Hooks,
    Themes,
    Languages,
    MCP,
    Custom(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledPlugin {
    pub manifest: PluginManifest,
    pub install_path: PathBuf,
    pub enabled: bool,
    pub installed_at: chrono::DateTime<chrono::Utc>,
}