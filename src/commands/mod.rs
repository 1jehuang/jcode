//! CarpAI 命令模块化架构
//!
//! 对标 Claude Code 的命令系统，提供100+顶层命令
//!
//! ## 模块分类
//!
//! - **auth**: 认证授权 (login/logout/permissions)
//! - **session**: 会话管理 (session/resume/compact/clear)
//! - **code**: 代码操作 (review/refactor/debug)
//! - **git**: Git工作流 (commit/branch/pr)
//! - **admin**: 管理命令 (usage/config/doctor)
//! - **file**: 文件管理 (files/rename/copy)
//! - **agent**: Agent系统 (agents/skills/plugin/mcp)
//! - **ide**: IDE集成 (ide/hooks/keybindings)
//! - **monitor**: 监控分析 (insights/cost/stats)
//! - **tool**: 工具集成 (help/version/init)
//! - **ui**: UI/UX (theme/vim/plan)
//! - **advanced**: 高级功能 (voice/buddy/bridge)

pub mod auth;
pub mod session;
pub mod code;
pub mod git;
pub mod admin;
pub mod file;
pub mod agent;
pub mod ide;
pub mod monitor;
pub mod tool;
pub mod ui;
pub mod advanced;
pub mod plan_cli;
pub mod core_agent_command;



// Re-export common types
pub use crate::cli::CommandResult;

/// 命令trait - 所有命令必须实现
pub trait Command: Send + Sync {
    /// 命令名称
    fn name(&self) -> &str;

    /// 命令描述
    fn description(&self) -> &str;

    /// 命令别名
    fn aliases(&self) -> &[&str] {
        &[]
    }

    /// 执行命令
    fn execute(&self, args: &[String]) -> impl std::future::Future<Output = anyhow::Result<CommandResult>> + Send;

    /// 是否只读（不修改文件系统）
    fn is_read_only(&self) -> bool {
        false
    }

    /// 是否需要认证
    fn requires_auth(&self) -> bool {
        true
    }
}

/// 命令注册表
pub struct CommandRegistry {
    commands: std::collections::HashMap<String, Box<dyn Command>>,
}

impl CommandRegistry {
    pub fn new() -> Self {
        let mut registry = Self {
            commands: std::collections::HashMap::new(),
        };
        registry.register_all();
        registry
    }

    fn register_all(&mut self) {
        // Auth commands
        self.register(auth::login::LoginCommand);
        self.register(auth::logout::LogoutCommand);
        self.register(auth::permissions::PermissionsCommand);

        // Session commands
        self.register(session::list::SessionListCommand);
        self.register(session::resume::ResumeCommand);
        self.register(session::compact::CompactCommand);

        // Code commands
        self.register(code::review::ReviewCommand);
        self.register(code::security_review::SecurityReviewCommand);

        // Git commands
        self.register(git::commit::CommitCommand);
        self.register(git::commit_push_pr::CommitPushPrCommand);

        // Admin commands
        self.register(admin::usage::UsageCommand);
        self.register(admin::config::ConfigCommand);
        self.register(admin::insights::InsightsCommand);
        self.register(admin::perf::PerfCommand);

        // File commands
        self.register(file::list::FilesCommand);

        // Tool commands
        self.register(tool::help::HelpCommand);
        self.register(tool::version::VersionCommand);

// Extra commands (bulk registered)
use super::extra_commands::*;

        // View git diff
        self.register(DiffCommand);
        // Analyze code context
        self.register(ContextCommand);
        // Context visualization
        self.register(CtxVizCommand);
        // Debug tool calls
        self.register(DebugToolCallCommand);
        // Find bugs
        self.register(BughunterCommand);
        // Performance issues
        self.register(PerfIssueCommand);
        // Trace analysis
        self.register(AntTraceCommand);
        // Quick navigation
        self.register(TeleportCommand2);
        // Tag management
        self.register(TagCommand);
        // Export session
        self.register(ExportCommand);
        // Environment variables
        self.register(EnvCommand);
        // Logout
        self.register(LogoutCommand2);
        // User feedback
        self.register(FeedbackCommand2);
        // Check upgrades
        self.register(UpgradeCommand);
        // Installation wizard
        self.register(InstallCommand);
        // Release notes
        self.register(ReleaseNotesCommand);
        // Remote environment
        self.register(RemoteEnvCommand);
        // Output format
        self.register(OutputStyleCommand);
        // Status bar config
        self.register(StatuslineCommand);
        // Statistics
        self.register(StatsCommand2);
        // Cost tracking
        self.register(CostCommand2);
        // Detailed usage
        self.register(ExtraUsageCommand);
        // Rate limit config
        self.register(RateLimitOptionsCommand);
        // Toggle sandbox
        self.register(SandboxToggleCommand);
        // Model selection
        self.register(ModelCommand2);
        // Desktop integration
        self.register(DesktopCommand);
        // Mobile sync
        self.register(MobileCommand);
        // Chrome integration
        self.register(ChromeCommand);
        // Stickers/emoji
        self.register(StickersCommand);
        // Like feedback
        self.register(GoodClaudeCommand);
        // By the way tips
        self.register(BtwCommand);
        // Advisor mode
        self.register(AdvisorCommand);
        // Review history
        self.register(ThinkbackCommand);
        // Replay history
        self.register(ThinkbackPlayCommand);
        // Hook management
        self.register(HooksCommand2);
        // Keybinding config
        self.register(KeybindingsCommand2);
        // Vim mode
        self.register(VimCommand2);
        // Plan mode
        self.register(PlanCommand2);
        // Effort control
        self.register(EffortCommand2);
        // Fast mode
        self.register(FastCommand2);
        // Iteration count
        self.register(PassesCommand2);
        // Voice mode
        self.register(VoiceCommand2);
        // Pair programming
        self.register(BuddyCommand2);
        // Bridge mode
        self.register(BridgeCommand2);
    }

    fn register(&mut self, cmd: impl Command + 'static) {
        let name = cmd.name().to_string();
        self.commands.insert(name, Box::new(cmd));
    }

    pub fn get(&self, name: &str) -> Option<&dyn Command> {
        self.commands.get(name).map(|c| c.as_ref())
    }

    pub fn list(&self) -> Vec<&dyn Command> {
        self.commands.values().map(|c| c.as_ref()).collect()
    }

    pub fn names(&self) -> Vec<String> {
        self.commands.keys().cloned().collect()
    }
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// 全局命令注册表
static COMMAND_REGISTRY: std::sync::LazyLock<std::sync::Mutex<Option<CommandRegistry>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(None));

/// 获取全局命令注册表
pub fn global_registry() -> std::sync::MutexGuard<'static, Option<CommandRegistry>> {
    COMMAND_REGISTRY.lock().unwrap()
}

/// 初始化全局命令注册表
pub fn init_global_registry() {
    let mut guard = COMMAND_REGISTRY.lock().unwrap();
    *guard = Some(CommandRegistry::new());
}

/// 执行命令
pub async fn execute_command(name: &str, args: &[String]) -> anyhow::Result<CommandResult> {
    let guard = global_registry();
    let registry = guard.as_ref()
        .ok_or_else(|| anyhow::anyhow!("Command registry not initialized"))?;

    let cmd = registry.get(name)
        .ok_or_else(|| anyhow::anyhow!("Unknown command: {}", name))?;

    cmd.execute(args).await
}
