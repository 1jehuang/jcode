//! Claude Code CLI 命令移植层
//!
//! 本模块实现了从Claude Code移植的核心CLI命令，使CarpAI在CLI功能上追平Claude Code。
//!
//! ## 已实现的命令 (Phase 1 - P0核心)
//!
//! ### CLI Flags (命令行选项)
//! - `-p, --print` : Print模式 (非交互式，执行后退出)
//! - `-c, --continue` : 继续上次会话
//! - `-r, --resume <session>` : 按名称/ID恢复会话
//! - `--add-dir <path>` : 添加额外工作目录
//! - `--model <name>` : 指定AI模型
//! - `--debug [category]` : 调试模式
//! - `--allowedTools <patterns>` : 工具白名单
//! - `--dangerously-skip-permissions` : 跳过权限提示
//! - `--append-system-prompt <text>` : 追加系统提示
//! - `--verbose` : 详细输出
//! - `--quiet` : 静默模式
//! - `--json` : JSON输出格式
//!
//! ### Slash Commands (斜杠命令)
//! - `/help` : 显示帮助信息
//! - `/clear` : 清空对话历史
//! - `/compact [instructions]` : 压缩对话上下文
//! - `/cost` : 显示Token使用统计
//! - `/doctor` : 健康检查诊断
//! - `/model` : 切换AI模型
//! - `/config` : 打开配置界面
//! - `/version` : 显示版本信息
//! - `/status` : 显示系统状态
//! - `/context` : 上下文使用情况
//!
//! ### 管理命令
//! - `carpai update` : 更新到最新版本
//! - `carpai auth login/logout/status` : 认证管理
//! - `carpai agents` : 子代理管理
//! - `carpai mcp` : MCP服务器配置

pub mod print_mode;
pub mod session_resume;
pub mod pipe_handler;
pub mod slash_commands;
pub mod cli_flags;
pub mod management_commands;

// Re-exports for convenience
pub use print_mode::run_print_mode;
pub use session_resume::{run_continue_session, run_resume_session};
pub use pipe_handler::handle_pipe_input;
pub use slash_commands::{
    handle_help_command,
    handle_clear_command,
    handle_compact_command,
    handle_cost_command,
    handle_doctor_command,
    handle_model_command,
    handle_config_command,
    handle_version_command,
    handle_status_command,
    handle_context_command,
};
pub use cli_flags::{
    parse_cli_flags,
    CliFlags,
};
pub use management_commands::{
    run_update_command,
    run_agents_command,
    run_mcp_command,
};

/// Claude Code CLI 兼容性层入口
/// 
/// 提供统一的接口来处理所有从Claude Code移植的命令
pub struct ClaudeCodeCompat {
    flags: CliFlags,
}

impl ClaudeCodeCompat {
    pub fn new() -> Self {
        Self {
            flags: CliFlags::default(),
        }
    }
    
    /// 从命令行参数初始化
    pub fn from_args(args: &[String]) -> Self {
        Self {
            flags: parse_cli_flags(args),
        }
    }
    
    /// 检查是否应该使用print模式
    pub fn should_use_print_mode(&self) -> bool {
        self.flags.print_mode
    }
    
    /// 检查是否应该继续上次会话
    pub fn should_continue_session(&self) -> bool {
        self.flags.continue_session
    }
    
    /// 获取要恢复的会话名称
    pub fn get_resume_session(&self) -> Option<&str> {
        self.flags.resume_session.as_deref()
    }
}
