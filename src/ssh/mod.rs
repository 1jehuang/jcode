//! # Enhanced SSH Remote Connection System
//!
//! 深度移植Claude Code的SSH远程能力，提供企业级SSH连接管理：
//!
//! ## 核心功能模块
//!
//! ### 1. 会话管理 (Session Management)
//! - 连接池 (Connection Pool) - 复用SSH连接提升性能
//! - 多会话支持 (Multi-session) - 同时管理多个远程主机
//! - 心跳检测 (Heartbeat) - 自动检测连接状态
//! - 自动重连 (Auto-reconnect) - 断线后自动恢复
//!
//! ### 2. 高级命令执行 (Advanced Execution)
//! - 同步执行 (Sync) - 阻塞等待结果
//! - 异步执行 (Async) - 后台任务队列
//! - 流式输出 (Streaming) - 实时数据传输
//! - 交互式终端 (Interactive PTY) - 支持sudo等交互命令
//! - 并行执行 (Parallel) - 多主机同时执行
//!
//! ### 3. 文件传输增强 (Enhanced File Transfer)
//! - SCP上传/下载 (基础传输)
//! - Rsync同步 (增量同步)
//! - 目录递归操作 (Recursive operations)
//! - 传输进度追踪 (Progress tracking)
//! - 断点续传 (Resume support)
//!
//! ### 4. 端口转发 (Port Forwarding)
//! - 本地端口转发 (Local forwarding) - L:local:port -> remote:port
//! - 远程端口转发 (Remote forwarding) - R:remote:port -> local:port
//! - 动态端口转发 (Dynamic/SOCKS) - SOCKS5代理
//!
//! ### 5. 隧道与代理 (Tunneling & Proxy)
//! - SSH隧道建立 (Tunnel creation)
//! - 跳板机支持 (Jump host/Bastion)
//! - HTTP/SOCKS5代理 (Proxy chaining)
//! - VPN模式 (Tun device)
//!
//! ### 6. 配置管理 (Configuration)
//! - ~/.ssh/config 解析 (Config file parsing)
//! - Host别名 (Host aliases)
//! - Identity文件管理 (Key management)
//! - Known hosts验证 (Host key verification)
//!
//! ### 7. 安全特性 (Security)
//! - 密钥认证 (Public key auth)
//! - SSH Agent集成 (Agent forwarding)
//! - 审计日志 (Audit logging)
//! - 命令白名单 (Command whitelist)
//!
//! ## 架构设计
//!
//! ```
//! SshManager (全局管理器)
//! +-- SshConfigParser (~/.ssh/config解析器)
//! +-- ConnectionPool (连接池)
//! |   +-- Vec<SshSession> (活跃连接)
//! |       +-- CommandExecutor (命令执行器)
//! |       |   +-- SyncExecutor
//! |       |   +-- AsyncExecutor
//! |       |   +-- StreamingExecutor
//! |       +-- FileTransfer (文件传输)
//! |       |   +-- ScpTransfer
//! |       |   +-- RsyncTransfer
//! |       +-- PortForwarder (端口转发)
//! |           +-- LocalForward
//! |           +-- RemoteForward
//! |           +-- DynamicForward (SOCKS5)
//! +-- SessionRegistry (会话注册表)
//! |   +-- HashMap<session_id, SshSession>
//! +-- AuditLogger (审计日志)
//!     +-- Vec<SshEvent> (操作记录)
//! ```

pub mod session;
pub mod command;
pub mod config;
pub mod tunnel;
pub mod transfer;
pub mod pool;
pub mod audit;
pub mod resilience;

// New advanced modules (Phase 1-2 completion)
pub mod sftp;              // SFTP protocol support
pub mod agent;             // SSH Agent forwarding
pub mod host_keys;         // Known Hosts management
pub mod pty;               // Full PTY terminal support
pub mod enhanced_scp;      // Enhanced SCP with options
pub mod mfa;               // Multi-factor authentication

// Enhanced integration layer
pub mod enhanced;           // High-level manager and utilities

// Re-export main types for convenience
pub use session::{SshSession, SshConfig, SshOutput};
pub use command::{SshCommand, CommandExecutor};
pub use config::{SshHostConfig, ConfigParser};
pub use tunnel::{PortForwarder, ForwardType};
pub use resilience::{
    ResilientSshSession, SmartRetryHandler, CircuitBreaker, RetryPolicy,
    ReconnectStrategy, ErrorClassification, ResilientConnectionPool
};

// Re-export new advanced features
pub use sftp::{SftpClient, SftpTransferResult, SftpError, SftpFileInfo, SftpSessionManager};
pub use agent::{
    SshAgentManager, AgentIdentity, AgentHealthStatus, AgentError, ensure_ssh_agent_running
};
pub use host_keys::{
    KnownHostsManager, KnownHostEntry, VerificationResult, HashAlgorithm, 
    KnownHostsError, KnownHostsStats
};
pub use pty::{PtySession, PtyConfig, PtyState, TerminalSize, PtyError, PtySessionManager};
pub use enhanced_scp::{EnhancedScp, EnhancedTransferResult, SymlinkBehavior, ScpError};
pub use mfa::{
    MfaManager, MfaConfig, AuthMethodType, MfaSession, AuthChallenge, AuthResult,
    TotpAuthenticator, TotpConfig, TotpError, U2fAuthenticator, U2fConfig, U2fError,
    RateLimitConfig
};