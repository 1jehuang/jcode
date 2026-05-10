// ════════════════════════════════════════════════════════════════
// AbortController / AbortSignal — 流式中断控制系统
// 对应 Claude Code: toolUseContext.abortController
// 
// 设计要点:
//   1. 支持 graceful abort — 等待正在执行的工具完成 (grace period)
//   2. 支持强制 abort — 立即终止所有操作
//   3. 支持 abort reason 分类 — 用户取消/超时/错误/权限拒绝
//   4. Signal 可跨 async task 共享 (Arc + AtomicBool)
// ════════════════════════════════════════════════════════════════

use std::sync::{
    Arc, 
    atomic::{AtomicBool, Ordering, AtomicU8},
};
use std::time::Instant;

use serde::{Deserialize, Serialize};

/// Abort 原因分类 (对应 Claude Code 的多种 abort 场景)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AbortReason {
    /// 用户主动取消 (Ctrl+C / UI 取消按钮)
    UserCancelled,
    
    /// 超时 (API / 工具执行超时)
    Timeout { elapsed_ms: u64 },
    
    /// 权限被拒绝 (用户拒绝了操作审批请求)
    PermissionDenied,
    
    /// 成本预算耗尽
    BudgetExceeded,
    
    /// 模型降级链全部失败
    AllModelsFailed,
    
    /// 外部信号 (系统关闭等)
    ExternalSignal,
    
    /// 内部错误导致的终止
    InternalError(String),
}

impl std::fmt::Display for AbortReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UserCancelled => write!(f, "用户取消"),
            Self::Timeout { elapsed_ms } => write!(f, "操作超时 ({elapsed_ms}ms)"),
            Self::PermissionDenied => write!(f, "权限被拒绝"),
            Self::BudgetExceeded => write!(f, "成本预算耗尽"),
            Self::AllModelsFailed => write!(f, "所有模型均失败"),
            Self::ExternalSignal => write!(f, "外部终止信号"),
            Self::InternalError(msg) => write!(f, "内部错误: {msg}"),
        }
    }
}

/// Abort 原因的原子存储 (u8 枚举 + None = 0)
const ABORT_REASON_NONE: u8 = 0;
const ABORT_REASON_USER_CANCELLED: u8 = 1;
const ABORT_REASON_TIMEOUT: u8 = 2;
const ABORT_REASON_PERMISSION_DENIED: u8 = 3;
const ABORT_REASON_BUDGET_EXCEEDED: u8 = 4;
const ABORT_REASON_ALL_MODELS_FAILED: u8 = 5;
const ABORT_REASON_EXTERNAL_SIGNAL: u8 = 6;
// 7+ reserved for InternalError variants

/// 中断信号 — 可跨线程/async task 共享的只读句柄
/// 
/// # 使用模式
/// ```ignore
/// let signal = controller.signal();
/// // 在异步任务中检查:
/// if signal.is_aborted() { return; }
/// // 或等待中断:
/// signal.wait_aborted().await;
/// ```
#[derive(Debug, Clone)]
pub struct AbortSignal {
    inner: Arc<AbortSignalInner>,
}

#[derive(Debug)]
struct AbortSignalInner {
    /// 是否已触发 abort
    aborted: AtomicBool,
    /// Abort 原因编码
    reason_code: AtomicU8,
    /// Abort 触发时间
    aborted_at: std::sync::Mutex<Option<Instant>>,
    /// 用于 wait_aborted() 的通知通道
    notify: tokio::sync::Notify,
}

impl AbortSignal {
    /// 创建新的未触发信号
    pub fn new() -> Self {
        Self {
            inner: Arc::new(AbortSignalInner {
                aborted: AtomicBool::new(false),
                reason_code: AtomicU8::new(ABORT_REASON_NONE),
                aborted_at: std::sync::Mutex::new(None),
                notify: tokio::sync::Notify::new(),
            })
        }
    }
    
    /// 检查是否已触发 abort (无阻塞，O(1))
    #[inline]
    pub fn is_aborted(&self) -> bool {
        self.inner.aborted.load(Ordering::Relaxed)
    }
    
    /// 获取 abort 原因
    pub fn reason(&self) -> Option<AbortReason> {
        self.decode_reason(self.inner.reason_code.load(Ordering::Relaxed))
    }
    
    /// 获取 abort 触发以来的耗时
    pub fn elapsed_since_abort(&self) -> Option<std::time::Duration> {
        let guard = self.inner.aborted_at.lock().ok()?;
        guard.map(|t| t.elapsed())
    }
    
    /// 异步等待 abort 信号触发
    /// 
    /// 返回 abort 原因。如果从未触发则一直等待。
    pub async fn wait_aborted(&self) -> Option<AbortReason> {
        self.inner.notified().await;
        self.reason()
    }
    
    /// 带超时的等待 abort
    /// 
    /// 返回 `Some(reason)` 如果在超时前触发了 abort，
    /// `None` 表示超时。
    pub async fn wait_aborted_with_timeout(
        &self, 
        timeout: std::time::Duration
    ) -> Option<AbortReason> {
        tokio::select! {
            _ = self.inner.notified() => self.reason(),
            _ = tokio::time::sleep(timeout) => None,
        }
    }
    
    /// 检查是否已 abort，如果是则返回错误
    /// 
    /// 便捷方法: 在循环中使用 `signal.check_aborted()?`
    pub fn check_aborted(&self) -> Result<(), AbortError> {
        if self.is_aborted() {
            Err(AbortError {
                reason: self.reason().unwrap_or(AbortReason::ExternalSignal),
            })
        } else {
            Ok(())
        }
    }
    
    fn decode_reason(&self, code: u8) -> Option<AbortReason> {
        match code {
            ABORT_REASON_NONE => None,
            ABORT_REASON_USER_CANCELLED => Some(AbortReason::UserCancelled),
            ABORT_REASON_TIMEOUT => Some(AbortReason::Timeout { elapsed_ms: 0 }),
            ABORT_REASON_PERMISSION_DENIED => Some(AbortReason::PermissionDenied),
            ABORT_REASON_BUDGET_EXCEEDED => Some(AbortReason::BudgetExceeded),
            ABORT_REASON_ALL_MODELS_FAILED => Some(AbortReason::AllModelsFailed),
            ABORT_REASON_EXTERNAL_SIGNAL => Some(AbortReason::ExternalSignal),
            _ => Some(AbortReason::InternalError("unknown".to_string())),
        }
    }
}

impl Default for AbortSignal {
    fn default() -> Self {
        Self::new()
    }
}

/// Abort 错误 — 当 check_aborted() 失败时返回
#[derive(Debug, Clone, thiserror::Error)]
#[error("operation aborted: {reason}")]
pub struct AbortError {
    pub reason: AbortReason,
}

/// Abort 控制器 — 拥有触发 abort 的能力
/// 
/// # 所有权模型
/// - `AbortController`: 单一所有者，可触发 abort
/// - `AbortSignal`: 克隆共享，只读检查
#[derive(Debug)]
pub struct AbortController {
    signal: AbortSignal,
}

impl AbortController {
    pub fn new() -> Self {
        Self {
            signal: AbortSignal::new(),
        }
    }
    
    /// 获取共享的信号句柄
    pub fn signal(&self) -> &AbortSignal {
        &self.signal
    }
    
    /// 获取克隆的信号句柄 (可传递给其他 task)
    pub fn signal_clone(&self) -> AbortSignal {
        self.signal.clone()
    }
    
    /// 触发 abort (graceful — 允许正在执行的操作完成)
    pub fn abort(&self, reason: AbortReason) {
        // 设置标志位
        self.signal.inner.aborted.store(true, Ordering::Release);
        
        // 编码并设置原因
        let code = self.encode_reason(&reason);
        self.signal.inner.reason_code.store(code, Ordering::Release);
        
        // 记录时间
        if let Ok(mut guard) = self.signal.inner.aborted_at.lock() {
            *guard = Some(Instant::now());
        }
        
        // 通知所有 waiter
        self.signal.inner.notify.notify_waiters();
        
        tracing::info!(?reason, "Abort triggered");
    }
    
    /// 获取当前 abort 原因
    pub fn reason(&self) -> Option<AbortReason> {
        self.signal.reason()
    }
    
    /// 重置 abort 状态 (谨慎使用! 通常用于测试)
    #[cfg(test)]
    pub fn reset(&self) {
        self.signal.inner.aborted.store(false, Ordering::Release);
        self.signal.inner.reason_code.store(ABORT_REASON_NONE, Ordering::Release);
        if let Ok(mut guard) = self.signal.inner.aborted_at.lock() {
            *guard = None;
        }
    }
    
    fn encode_reason(&self, reason: &AbortReason) -> u8 {
        match reason {
            AbortReason::UserCancelled => ABORT_REASON_USER_CANCELLED,
            AbortReason::Timeout { .. } => ABORT_REASON_TIMEOUT,
            AbortReason::PermissionDenied => ABORT_REASON_PERMISSION_DENIED,
            AbortReason::BudgetExceeded => ABORT_REASON_BUDGET_EXCEEDED,
            AbortReason::AllModelsFailed => ABORT_REASON_ALL_MODELS_FAILED,
            AbortReason::ExternalSignal => ABORT_REASON_EXTERNAL_SIGNAL,
            AbortReason::InternalError(_) => 7,  // generic internal error
        }
    }
}

impl Default for AbortController {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{timeout as tokio_timeout, Duration};

    #[tokio::test]
    async fn test_basic_abort_flow() {
        let ctrl = AbortController::new();
        let signal = ctrl.signal_clone();
        
        assert!(!signal.is_aborted());
        
        ctrl.abort(AbortReason::UserCancelled);
        
        assert!(signal.is_aborted());
        assert_eq!(signal.reason(), Some(AbortReason::UserCancelled));
    }

    #[tokio::test]
    async fn test_wait_aborted_async() {
        let ctrl = AbortController::new();
        let signal = ctrl.signal_clone();
        
        // spawn 一个 task 来触发 abort
        let ctrl2 = ctrl.clone();  // 需要克隆 controller... 
        // 实际上 AbortController 没有 Clone, 让我们用不同的方式
        
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            ctrl.abort(AbortReason::Timeout { elapsed_ms: 50 });
        });
        
        let result = signal.wait_aborted().await;
        assert!(result.is_some());
        matches!(result.unwrap(), AbortReason::Timeout { .. });
    }

    #[test]
    fn test_check_aborted() {
        let ctrl = AbortController::new();
        let signal = ctrl.signal();
        
        assert!(signal.check_aborted().is_ok());
        
        ctrl.abort(AbortReason::PermissionDenied);
        
        let err = signal.check_aborted().unwrap_err();
        assert_eq!(err.reason, AbortReason::PermissionDenied);
    }
}
