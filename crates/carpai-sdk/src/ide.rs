//! IDE integration adapters

use crate::error::{CarpAiError, Result};
use crate::types::*;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// IDE types supported by the SDK
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum IdeType {
    VSCode,
    JetBrains,
    Neovim,
    Vim,
    Emacs,
    SublimeText,
    Web,
    Custom(String),
}

impl IdeType {
    pub fn as_str(&self) -> &str {
        match self {
            Self::VSCode => "vscode",
            Self::JetBrains => "jetbrains",
            Self::Neovim => "neovim",
            Self::Vim => "vim",
            Self::Emacs => "emacs",
            Self::SublimeText => "sublime_text",
            Self::Web => "web",
            Self::Custom(name) => name,
        }
    }

    /// Detect the current IDE from environment
    pub fn detect() -> Option<Self> {
        // Check for VS Code
        if std::env::var("VSCODE_PID").is_ok()
            || std::env::var("TERM_PROGRAM").map_or(false, |t| t.contains("vscode"))
        {
            return Some(Self::VSCode);
        }

        // Check for JetBrains IDEs
        if std::env::var("JETBRAINS_CLIENT_TOKEN").is_ok() {
            return Some(Self::JetBrains);
        }

        // Check for Neovim/Vim
        if std::env::var("NVIM").is_ok() || std::env::var("VIMRUNTIME").is_ok() {
            return if std::env::var("NVIM").is_ok() {
                Some(Self::Neovim)
            } else {
                Some(Self::Vim)
            };
        }

        None
    }
}

/// IDE-specific configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdeConfig {
    /// The type of IDE
    pub ide_type: IdeType,

    /// IDE version (if available)
    pub version: Option<String>,

    /// Supported features for this IDE
    #[serde(default)]
    pub capabilities: IdeCapabilities,

    /// IDE-specific settings
    #[serde(default)]
    pub settings: serde_json::Value,
}

/// Capabilities of an IDE
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IdeCapabilities {
    /// Supports inline completion
    #[serde(default = "default_true")]
    pub inline_completion: bool,

    /// Supports chat panel
    #[serde(default = "default_true")]
    pub chat_panel: bool,

    /// Supports code actions
    #[serde(default)]
    pub code_actions: bool,

    /// Supports multi-file editing
    #[serde(default)]
    pub multi_file_edit: bool,

    /// Supports terminal integration
    #[serde(default)]
    pub terminal: bool,

    /// Supports custom UI rendering
    #[serde(default)]
    pub custom_ui: bool,
}

fn default_true() -> bool { true }

/// Trait for IDE adapters
#[async_trait]
pub trait IdeAdapter: Send + Sync {
    /// Get the IDE type
    fn ide_type(&self) -> IdeType;

    /// Get IDE capabilities
    fn capabilities(&self) -> &IdeCapabilities;

    /// Initialize the adapter (called when IDE plugin loads)
    async fn initialize(&self, config: IdeConfig) -> Result<()>;

    /// Show a notification to the user
    async fn show_notification(&self, message: &str, level: NotificationLevel) -> Result<()>;

    /// Show a quick pick / selection dialog
    async fn show_quick_pick(
        &self,
        items: Vec<QuickPickItem>,
    ) -> Result<Option<QuickPickItem>>;

    /// Show an input box
    async fn show_input_box(&self, prompt: &str, placeholder: Option<&str>) -> Result<Option<String>>;

    /// Get the currently open file
    async fn get_active_file(&self) -> Option<ActiveFileInfo>;

    /// Get selected text/range
    async fn get_selection(&self) -> Option<TextSelection>;

    /// Apply text edits to files
    async fn apply_edits(&self, edits: Vec<TextEdit>) -> Result<()>;

    /// Execute a command in the terminal
    async fn execute_command(&self, command: &str) -> Result<String>;

    /// Show progress indicator
    async fn show_progress(&self, title: &str, message: Option<&str>) -> Result<ProgressHandle>;
}

/// Notification level
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NotificationLevel {
    Info,
    Warning,
    Error,
}

/// Quick pick item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuickPickItem {
    pub label: String,
    pub description: Option<String>,
    pub detail: Option<String>,
    pub value: String,
}

/// Active file information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveFileInfo {
    pub path: String,
    pub language: Option<String>,
    pub content: String,
    pub cursor_line: u32,
    pub cursor_column: u32,
}

/// Text selection information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextSelection {
    pub start_line: u32,
    pub start_column: u32,
    pub end_line: u32,
    pub end_column: u32,
    pub text: String,
}

/// Text edit operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextEdit {
    pub file_path: String,
    pub start_line: u32,
    pub start_column: u32,
    pub end_line: u32,
    pub end_column: u32,
    pub new_text: String,
}

/// Progress handle for showing progress
pub trait ProgressHandle: Send + Sync {
    /// Update progress message
    fn update_message(&self, message: &str);

    /// Report progress percentage
    fn report_progress(&self, percent: f64);

    /// Complete the progress (success)
    fn complete(self: Box<Self>, message: Option<&str>);

    /// Fail the progress (error)
    fn fail(self: Box<Self>, error: &str);
}

/// Generic/No-op IDE adapter for testing or headless mode
pub struct GenericIdeAdapter {
    ide_type: IdeType,
    capabilities: IdeCapabilities,
}

impl GenericIdeAdapter {
    pub fn new(ide_type: IdeType) -> Self {
        Self {
            ide_type,
            capabilities: IdeCapabilities::default(),
        }
    }

    pub fn with_capabilities(ide_type: IdeType, capabilities: IdeCapabilities) -> Self {
        Self {
            ide_type,
            capabilities,
        }
    }
}

#[async_trait]
impl IdeAdapter for GenericIdeAdapter {
    fn ide_type(&self) -> IdeType {
        self.ide_type.clone()
    }

    fn capabilities(&self) -> &IdeCapabilities {
        &self.capabilities
    }

    async fn initialize(&self, _config: IdeConfig) -> Result<()> {
        Ok(())
    }

    async fn show_notification(&self, _message: &str, _level: NotificationLevel) -> Result<()> {
        Ok(())
    }

    async fn show_quick_pick(&self, items: Vec<QuickPickItem>) -> Result<Option<QuickPickItem>> {
        Ok(items.into_iter().next())
    }

    async fn show_input_box(&self, _prompt: &str, _placeholder: Option<&str>) -> Result<Option<String>> {
        Ok(None)
    }

    async fn get_active_file(&self) -> Option<ActiveFileInfo> {
        None
    }

    async fn get_selection(&self) -> Option<TextSelection> {
        None
    }

    async fn apply_edits(&self, _edits: Vec<TextEdit>) -> Result<()> {
        Ok(())
    }

    async fn execute_command(&self, _command: &str) -> Result<String> {
        Err(CarpAiError::FeatureNotAvailable {
            feature: "terminal".to_string(),
            requirement: Some("Generic adapter does not support terminal commands".to_string()),
        })
    }

    async fn show_progress(&self, title: &str, message: Option<&str>) -> Result<Box<dyn ProgressHandle>> {
        tracing::info!(title = %title, ?message, "Showing progress");
        Ok(Box::new(NoOpProgressHandle))
    }
}

/// No-op progress handle
struct NoOpProgressHandle;

impl ProgressHandle for NoOpProgressHandle {
    fn update_message(&self, _message: &str) {}
    fn report_progress(&self, _percent: f64) {}
    fn complete(self: Box<Self>, _message: Option<&str>) {}
    fn fail(self: Box<Self>, _error: &str) {}
}
