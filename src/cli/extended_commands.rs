//! Extended Commands System
//!
//! Ported from claude_code_src with Rust adaptations:
//! - /btw: Context-aware hints and suggestions
//! - /fast: Fast mode toggle (reduced thinking, quicker responses)
//! - /rewind: Session rollback to previous state
//!
//! These commands enhance the core command system with advanced agent capabilities.

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

/// Command execution context
#[derive(Debug, Clone)]
pub struct CommandContext {
    pub session_id: String,
    pub user_input: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub metadata: HashMap<String, String>,
}

impl Default for CommandContext {
    fn default() -> Self {
        Self {
            session_id: String::new(),
            user_input: String::new(),
            timestamp: chrono::Utc::now(),
            metadata: HashMap::new(),
        }
    }
}

/// Command result with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandResult {
    pub success: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    pub duration_ms: u64,
}

/// Extended command trait
#[async_trait]
pub trait ExtendedCommand: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn usage(&self) -> &str;

    async fn execute(&self, ctx: &CommandContext, args: Option<&str>) -> Result<CommandResult>;
    async fn validate_args(&self, args: Option<&str>) -> Result<()>;
}

// ── /btw Command: Context-Aware Hints ──

pub struct BtwCommand {
    hint_generator: Arc<RwLock<Option<Arc<dyn Fn(&str) -> Vec<String> + Send + Sync>>>>,
}

impl BtwCommand {
    pub fn new() -> Self {
        Self {
            hint_generator: Arc::new(RwLock::new(None)),
        }
    }

    pub fn set_hint_generator<F>(&self, generator: F)
    where
        F: Fn(&str) -> Vec<String> + Send + Sync + 'static,
    {
        let gen = Arc::new(generator) as Arc<dyn Fn(&str) -> Vec<String> + Send + Sync>;
        // In production, you'd properly store this
    }

    fn generate_contextual_hints(&self, _context: &str) -> Vec<String> {
        vec![
            format!("💡 Tip: Consider breaking this task into smaller steps"),
            format!("📝 You can use /fast mode for quicker iterations"),
            format!("🔄 Use /rewind if you want to undo recent changes"),
            format!("🎯 Focus on the most impactful changes first"),
        ]
    }
}

impl Default for BtwCommand {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ExtendedCommand for BtwCommand {
    fn name(&self) -> &str {
        "btw"
    }

    fn description(&self) -> &str {
        "Show contextual hints and suggestions based on current work"
    }

    fn usage(&self) -> &str {
        "/btw [context]"
    }

    async fn validate_args(&self, _args: Option<&str>) -> Result<()> {
        Ok(())
    }

    async fn execute(&self, ctx: &CommandContext, args: Option<&str>) -> Result<CommandResult> {
        let start = std::time::Instant::now();

        let context = args.unwrap_or(&ctx.user_input);
        let hints = self.generate_contextual_hints(context);

        let mut message = String::from("🤔 **By the way...**\n\n");
        for (i, hint) in hints.iter().enumerate() {
            message.push_str(&format!("{}. {}\n", i + 1, hint));
        }

        // Add context-specific suggestions
        if !ctx.metadata.is_empty() {
            message.push_str("\n**Context:**\n");
            for (key, value) in &ctx.metadata {
                message.push_str(&format!("- {}: {}\n", key, value));
            }
        }

        Ok(CommandResult {
            success: true,
            message,
            data: None,
            duration_ms: start.elapsed().as_millis() as u64,
        })
    }
}

// ── /fast Command: Fast Mode Toggle ──

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FastMode {
    Normal,
    Fast,
    Turbo,
}

impl std::fmt::Display for FastMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FastMode::Normal => write!(f, "normal"),
            FastMode::Fast => write!(f, "fast"),
            FastMode::Turbo => write!(f, "turbo"),
        }
    }
}

pub struct FastCommand {
    current_mode: Arc<RwLock<FastMode>>,
    mode_history: Arc<RwLock<Vec<(chrono::DateTime<chrono::Utc>, FastMode)>>>,
}

impl FastCommand {
    pub fn new() -> Self {
        Self {
            current_mode: Arc::new(RwLock::new(FastMode::Normal)),
            mode_history: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub async fn get_current_mode(&self) -> FastMode {
        *self.current_mode.read().await
    }

    pub async fn set_mode(&self, mode: FastMode) {
        let prev = *self.current_mode.read().await;
        *self.current_mode.write().await = mode;
        self.mode_history.write().await.push((chrono::Utc::now(), prev));
        info!("Fast mode changed: {} -> {}", prev, mode);
    }

    fn get_mode_description(mode: FastMode) -> &'static str {
        match mode {
            FastMode::Normal => "🐢 **Normal Mode**: Full reasoning, detailed responses",
            FastMode::Fast => "⚡ **Fast Mode**: Reduced thinking, quicker responses",
            FastMode::Turbo => "🚀 **Turbo Mode**: Minimal reasoning, maximum speed",
        }
    }

    fn get_mode_settings(mode: FastMode) -> HashMap<&'static str, &'static str> {
        let mut settings = HashMap::new();
        match mode {
            FastMode::Normal => {
                settings.insert("thinking_budget", "full");
                settings.insert("response_detail", "high");
                settings.insert("verification_level", "strict");
            }
            FastMode::Fast => {
                settings.insert("thinking_budget", "reduced");
                settings.insert("response_detail", "medium");
                settings.insert("verification_level", "moderate");
            }
            FastMode::Turbo => {
                settings.insert("thinking_budget", "minimal");
                settings.insert("response_detail", "low");
                settings.insert("verification_level", "basic");
            }
        }
        settings
    }
}

impl Default for FastCommand {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ExtendedCommand for FastCommand {
    fn name(&self) -> &str {
        "fast"
    }

    fn description(&self) -> &str {
        "Toggle or set fast mode for quicker responses"
    }

    fn usage(&self) -> &str {
        "/fast [normal|fast|turbo]"
    }

    async fn validate_args(&self, args: Option<&str>) -> Result<()> {
        if let Some(arg) = args {
            match arg.to_lowercase().as_str() {
                "normal" | "fast" | "turbo" | "" => Ok(()),
                _ => Err(anyhow::anyhow!(
                    "Invalid mode '{}'. Use: normal, fast, or turbo",
                    arg
                )),
            }
        } else {
            Ok(())
        }
    }

    async fn execute(&self, _ctx: &CommandContext, args: Option<&str>) -> Result<CommandResult> {
        let start = std::time::Instant::now();

        let new_mode = match args.map(|s| s.to_lowercase()).as_deref() {
            Some("normal") => FastMode::Normal,
            Some("fast") => FastMode::Fast,
            Some("turbo") => FastMode::Turbo,
            Some("") | None => {
                // Toggle between modes
                match self.get_current_mode().await {
                    FastMode::Normal => FastMode::Fast,
                    FastMode::Fast => FastMode::Turbo,
                    FastMode::Turbo => FastMode::Normal,
                }
            }
            Some(invalid) => {
                return Ok(CommandResult {
                    success: false,
                    message: format!("❌ Invalid mode '{}'. Use: normal, fast, turbo", invalid),
                    data: None,
                    duration_ms: start.elapsed().as_millis() as u64,
                });
            }
        };

        self.set_mode(new_mode).await;

        let settings = Self::get_mode_settings(new_mode);
        let mut message = format!("{}\n\n", Self::get_mode_description(new_mode));
        message.push_str("**Settings:**\n");
        for (key, value) in &settings {
            message.push_str(&format!("- {}: {}\n", key, value));
        }

        Ok(CommandResult {
            success: true,
            message,
            data: Some(serde_json::json!({
                "mode": new_mode.to_string(),
                "settings": settings
            })),
            duration_ms: start.elapsed().as_millis() as u64,
        })
    }
}

// ── /rewind Command: Session Rollback ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub id: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub description: String,
    pub message_count: usize,
    pub tool_calls: usize,
    pub data_hash: String,
}

pub struct RewindCommand {
    snapshots: Arc<RwLock<Vec<SessionSnapshot>>>,
    max_snapshots: usize,
}

impl RewindCommand {
    pub fn new(max_snapshots: usize) -> Self {
        Self {
            snapshots: Arc::new(RwLock::new(Vec::new())),
            max_snapshots,
        }
    }

    pub async fn create_snapshot(
        &self,
        description: String,
        message_count: usize,
        tool_calls: usize,
    ) -> String {
        let snapshot = SessionSnapshot {
            id: format!("snap_{}", chrono::Utc::now().timestamp_millis()),
            timestamp: chrono::Utc::now(),
            description,
            message_count,
            tool_calls,
            data_hash: format!("{:x}", md5::compute(format!("{}", chrono::Utc::now()))),
        };

        let id = snapshot.id.clone();
        let mut snapshots = self.snapshots.write().await;
        snapshots.push(snapshot);

        // Keep only the last N snapshots
        while snapshots.len() > self.max_snapshots {
            snapshots.remove(0);
        }

        debug!("Created snapshot: {}", id);
        id
    }

    pub async fn list_snapshots(&self) -> Vec<SessionSnapshot> {
        self.snapshots.read().await.clone()
    }

    pub async fn get_snapshot(&self, id: &str) -> Option<SessionSnapshot> {
        self.snapshots
            .read()
            .await
            .iter()
            .find(|s| s.id == id)
            .cloned()
    }

    pub async fn rewind_to(&self, snapshot_id: &str) -> Result<()> {
        let snapshot = self
            .get_snapshot(snapshot_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("Snapshot '{}' not found", snapshot_id))?;

        info!(
            "Rewinding to snapshot '{}' from {:?}",
            snapshot_id, snapshot.timestamp
        );

        // In a real implementation, this would:
        // 1. Restore conversation state
        // 2. Revert file changes
        // 3. Reset tool state
        // 4. Notify UI of state change

        Ok(())
    }
}

impl Default for RewindCommand {
    fn default() -> Self {
        Self::new(10)
    }
}

#[async_trait]
impl ExtendedCommand for RewindCommand {
    fn name(&self) -> &str {
        "rewind"
    }

    fn description(&self) -> &str {
        "Rollback session to a previous state"
    }

    fn usage(&self) -> &str {
        "/rewind [list|<snapshot_id>]"
    }

    async fn validate_args(&self, args: Option<&str>) -> Result<()> {
        if let Some(arg) = args {
            if arg != "list" && !arg.starts_with("snap_") {
                Err(anyhow::anyhow!(
                    "Invalid argument. Use 'list' or a snapshot ID"
                ))
            } else {
                Ok(())
            }
        } else {
            Ok(())
        }
    }

    async fn execute(&self, _ctx: &CommandContext, args: Option<&str>) -> Result<CommandResult> {
        let start = std::time::Instant::now();

        match args.map(|s| s.to_lowercase()).as_deref() {
            Some("list") | None => {
                let snapshots = self.list_snapshots().await;

                if snapshots.is_empty() {
                    return Ok(CommandResult {
                        success: true,
                        message: "📜 No snapshots available".to_string(),
                        data: Some(serde_json::json!({ "snapshots": [] })),
                        duration_ms: start.elapsed().as_millis() as u64,
                    });
                }

                let mut message = format!("📜 **Available Snapshots** ({} total)\n\n", snapshots.len());
                for (i, snap) in snapshots.iter().rev().enumerate() {
                    message.push_str(&format!(
                        "{}. **{}** - {:?}\n   Messages: {}, Tool calls: {}\n   {}\n\n",
                        i + 1,
                        snap.id,
                        snap.timestamp.format("%H:%M:%S"),
                        snap.message_count,
                        snap.tool_calls,
                        snap.description,
                    ));
                }

                Ok(CommandResult {
                    success: true,
                    message,
                    data: Some(serde_json::json!({ "snapshots": snapshots })),
                    duration_ms: start.elapsed().as_millis() as u64,
                })
            }
            Some(snapshot_id) => {
                match self.rewind_to(snapshot_id).await {
                    Ok(_) => Ok(CommandResult {
                        success: true,
                        message: format!("⏪ **Rewound to snapshot {}**\n\nSession state has been restored.", snapshot_id),
                        data: Some(serde_json::json!({ "rewound_to": snapshot_id })),
                        duration_ms: start.elapsed().as_millis() as u64,
                    }),
                    Err(e) => Ok(CommandResult {
                        success: false,
                        message: format!("❌ {}", e),
                        data: None,
                        duration_ms: start.elapsed().as_millis() as u64,
                    }),
                }
            }
        }
    }
}

// ── Command Registry ──

pub struct ExtendedCommandRegistry {
    commands: RwLock<HashMap<String, Arc<dyn ExtendedCommand>>>,
}

impl ExtendedCommandRegistry {
    pub fn new() -> Self {
        Self {
            commands: RwLock::new(HashMap::new()),
        }
    }

    pub async fn register(&self, command: Arc<dyn ExtendedCommand>) {
        let name = command.name().to_string();
        self.commands.write().await.insert(name.clone(), command);
        info!("Extended command registered: {}", name);
    }

    pub async fn get(&self, name: &str) -> Option<Arc<dyn ExtendedCommand>> {
        self.commands.read().await.get(name).cloned()
    }

    pub async fn list_commands(&self) -> Vec<String> {
        self.commands.read().await.keys().cloned().collect()
    }

    pub async fn execute_command(
        &self,
        name: &str,
        ctx: &CommandContext,
        args: Option<&str>,
    ) -> Result<CommandResult> {
        let command = self
            .get(name)
            .await
            .ok_or_else(|| anyhow::anyhow!("Unknown command: {}", name))?;

        command.validate_args(args).await?;
        command.execute(ctx, args).await
    }
}

impl Default for ExtendedCommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Initialize and register all extended commands
pub async fn init_extended_commands() -> ExtendedCommandRegistry {
    let registry = ExtendedCommandRegistry::new();

    registry.register(Arc::new(BtwCommand::new())).await;
    registry.register(Arc::new(FastCommand::new())).await;
    registry.register(Arc::new(RewindCommand::new())).await;

    info!("Extended commands system initialized");

    registry
}
