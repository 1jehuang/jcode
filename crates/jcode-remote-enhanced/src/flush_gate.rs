//! FlushGate - 历史刷写门控
//!
//! 移植自 Claude Code `remoteBridgeCore.ts`:
//! ```typescript
//! const flushGate = new FlushGate<Message>()
//! // 写入期间队列消息，flush 完成后按序发送
//! ```
//!
//! 解决的问题: 初始历史消息与实时写入消息之间的竞态条件
//!
//! # 状态机
//! ```text
/// Open ──[start_flush]──→ Flushing(queue, future)
///   ↑                        │
///   └──────[flush_done]────────┘
//! ```
//!
//! # 使用模式
//! 1. 正常状态 (Open): `enqueue()` 直接返回消息, 立即可用
//! 2. 初始加载期间 (Flushing): `enqueue()` 将消息排队等待
//! 3. flush 完成: 排队消息按 FIFO 顺序释放

use std::collections::VecDeque;
use tokio::sync::{oneshot, watch};

/// FlushGate 门控
///
/// 泛型参数 T 为被门控的消息类型
pub struct FlushGate<T> {
    /// 当前状态
    state: FlushGateState<T>,
}

/// 内部状态枚举
enum FlushGateState<T> {
    /// 开放模式 - 消息直接通过
    Open,

    /// 冲刷中 - 新消息入队等待
    Flushing {
        /// 排队的消息
        queue: VecDeque<T>,
        /// flush 完成的通知接收端
        done_receiver: Option<watch::Receiver<bool>>,
    },
}

impl<T> Default for FlushGate<T> {
    fn default() -> Self {
        Self {
            state: FlushGateState::Open,
        }
    }
}

impl<T> FlushGate<T> {
    /// 创建新的 FlushGate
    pub fn new() -> Self {
        Self::default()
    }

    /// 开始一个 flush 操作
    ///
    /// 调用后, 所有新消息将被排队直到 `flush_complete()` 被调用
    pub fn start_flush(&mut self) {
        match &self.state {
            FlushGateState::Open => {
                self.state = FlushGateState::Flushing {
                    queue: VecDeque::new(),
                    done_receiver: None,
                };
            }
            FlushGateState::Flushing { .. } => {
                tracing::warn!("FlushGate: start_flush called while already flushing");
            }
        }
    }

    /// 将消息入队或直接返回
    ///
    /// Returns:
    /// - `Some(message)`: 如果在开放模式下, 消息立即可用
    /// - `None`: 如果在冲刷模式下, 消息已入队等待
    pub fn enqueue(&mut self, message: T) -> Option<T> {
        match &mut self.state {
            FlushGateState::Open => Some(message),
            FlushGateState::Flushing { queue, .. } => {
                queue.push_back(message);
                None
            }
        }
    }

    /// 标记 flush 完成, 返回所有排队的消息
    ///
    /// 所有在冲刷期间排队的消息将按 FIFO 顺序返回
    pub fn flush_complete(&mut self) -> Vec<T> {
        let mut result = Vec::new();
        
        if let FlushGateState::Flushing { queue, .. } = 
            std::mem::replace(&mut self.state, FlushGateState::Open)
        {
            result = queue.into_iter().collect();
        }
        
        result
    }

    /// 当前是否处于冲刷状态
    pub fn is_flushing(&self) -> bool {
        matches!(&self.state, FlushGateState::Flushing { .. })
    }

    /// 获取当前排队消息数量
    pub fn queued_count(&self) -> usize {
        match &self.state {
            FlushGateState::Open => 0,
            FlushGateState::Flushing { queue, .. } => queue.len(),
        }
    }
}
