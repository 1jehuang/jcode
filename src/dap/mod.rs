//! DAP (Debug Adapter Protocol) 客户端模块
//! 
//! 实现 Debug Adapter Protocol 客户端，支持与 IDE/编辑器进行调试通信
//! 
//! ## 核心功能
//! - 调试会话管理
//! - 断点设置与管理
//! - 堆栈跟踪获取
//! - 变量查看
//! - 步进调试（step in/out/next）
//! - 暂停/继续执行

pub mod protocol;
pub mod session;
pub mod adapter;
pub mod launch_integration;

pub use protocol::*;
pub use session::*;
pub use adapter::*;

#[cfg(test)]
mod tests;

pub use self::adapter::DebugAdapter;