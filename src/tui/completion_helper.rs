//! 补全助手模块
//! 
//! 提供代码补全相关的辅助功能

/// 补全预取状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompletionPrefetchState {
    Idle,
    Prefetching,
    Ready,
    Failed,
}

/// 补全助手结构体
pub struct CompletionHelper;

impl CompletionHelper {
    /// 创建新的补全助手
    pub fn new() -> Self {
        Self
    }
}
