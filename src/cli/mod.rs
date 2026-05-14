pub mod args;
pub mod auth_test;
pub mod commands;
pub mod debug;
pub mod dispatch;
pub mod extended_commands;
pub mod hot_exec;
pub mod login;
pub mod output;
pub mod provider_init;
pub mod selfdev;
pub mod startup;
pub mod terminal;
pub mod tui_launch;

// Claude Code CLI 兼容层 (Phase 1 - P0核心命令)
pub mod claude_compat;
pub mod print_mode;
pub mod session_resume;
pub mod pipe_handler;
pub mod slash_commands;
pub mod cli_flags;
pub mod management_commands;

// Phase 2 - P1高频命令
pub mod p1_commands;
