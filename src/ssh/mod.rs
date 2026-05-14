//! # SSH Remote Connection - 远程连接模块
//!
//! 提供完整的SSH远程连接和文件传输能力，包括：
//! - **会话管理** (SshSession) - 连接生命周期管理
//! - **命令执行** - 同步/流式命令执行
//! - **文件传输** - SCP上传/下载
//! - **CLI接口** (SshCommand) - 命令行封装
//!
//! ## 功能特性
//!
//! ✅ **安全连接** - 支持密钥文件认证
//! ✅ **超时控制** - 可配置连接和执行超时
//! ✅ **流式输出** - 实时显示远程命令输出
//! ✅ **文件传输** - 基于SCP的安全传输
//! ✅ **状态追踪** - 连接状态和运行时间监控
//!
//! ## 使用示例
//!
//! ```rust,no_run
//! use carpai::ssh::{SshSession, SshConfig};
//! use std::time::Duration;
//!
//! // 配置连接
//! let config = SshConfig {
//!     host: "example.com".to_string(),
//!     port: 2222,
//!     user: "deploy".to_string(),
//!     identity_file: Some(std::path::PathBuf::from("~/.ssh/id_rsa")),
//!     connect_timeout: Duration::from_secs(30),
//! };
//!
//! // 创建并连接
//! let mut session = SshSession::new(config);
//! session.connect().expect("Failed to connect");
//!
//! // 执行命令
//! let output = session.execute("ls -la").expect("Execution failed");
//! println!("Output:\n{}", output.stdout);
//!
//! // 流式执行（实时输出）
//! session.execute_streaming("tail -f /var/log/app.log", |line| {
//!     println!("{}", line);
//! }).ok();
//!
//! // 文件上传
//! session.upload(
//!     &std::path::PathBuf::from("local-file.txt"),
//!     &std::path::PathBuf::from("/remote/path/file.txt")
//! ).expect("Upload failed");
//!
//! // 断开连接
//! session.disconnect().ok();
//! ```
//!
//! ## 安全建议
//!
//! 1. **使用密钥认证** - 避免密码明文传输
//! 2. **限制端口范围** - 仅开放必要端口
//! 3. **设置合理超时** - 防止无限等待
//! 4. **定期轮换密钥** - 保持安全性

pub mod session;
pub mod command;

#[cfg(test)]
mod tests;

pub use session::{SshSession, SshConfig, SshOutput};
pub use command::SshCommand;