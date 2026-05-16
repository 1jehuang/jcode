// ════════════════════════════════════════════════════════════════
// MCP 连接管理器 — 自动重连 + 状态追踪
// ════════════════════════════════════════════════════════════════

use crate::types::ConnectionState;
use std::time::Duration;
use tokio::sync::RwLock;

/// 重连策略
#[derive(Debug, Clone)]
pub struct ReconnectPolicy {
    pub max_attempts: u32,
    pub initial_backoff_ms: u64,
    pub max_backoff_ms: u64,
    /// 抖动因子 (0.0 - 1.0), 用于避免惊群效应
    pub jitter_factor: f64,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            max_attempts: crate::DEFAULT_MAX_RECONNECT_ATTEMPTS,
            initial_backoff_ms: crate::DEFAULT_INITIAL_BACKOFF_MS,
            max_backoff_ms: crate::DEFAULT_MAX_BACKOFF_MS,
            jitter_factor: 0.2, // 20% random jitter
        }
    }
}

impl ReconnectPolicy {
    /// 根据尝试次数计算退避延迟
    ///
    /// 公式: min(initial * 2^(attempt-1) + jitter, max)
    ///
    /// Returns None if max attempts exceeded
    pub fn backoff_delay(&self, attempt: u32) -> Option<Duration> {
        if attempt > self.max_attempts || self.max_attempts == 0 {
            return None;
        }

        let base_ms = self.initial_backoff_ms
            .saturating_mul(2u64.saturating_pow(attempt.saturating_sub(1)));

        let capped = base_ms.min(self.max_backoff_ms);

        // Add random jitter (± jitter_factor)
        let jitter_range = (capped as f64 * self.jitter_factor) as i64;
        let final_ms = if jitter_range > 0 {
            use rand::Rng;
            let mut rng = rand::rng();
            let j: i64 = rng.random_range(-jitter_range..=jitter_range);
            (capped as i64 + j).max(0) as u64
        } else {
            capped
        };

        Some(Duration::from_millis(final_ms))
    }
}

/// 连接管理器
pub struct ConnectionManager {
    state: RwLock<ConnectionState>,
    reconnect_policy: ReconnectPolicy,
    current_attempt: RwLock<u32>,
    connect_count: RwLock<u64>,
    disconnect_count: RwLock<u64>,
    last_connected_at: RwLock<Option<std::time::Instant>>,
}

impl Default for ConnectionManager {
    fn default() -> Self {
        Self::new(ReconnectPolicy::default())
    }
}

impl ConnectionManager {
    pub fn new(policy: ReconnectPolicy) -> Self {
        Self {
            state: RwLock::new(ConnectionState::Pending),
            reconnect_policy: policy,
            current_attempt: RwLock::new(0),
            connect_count: RwLock::new(0),
            disconnect_count: RwLock::new(0),
            last_connected_at: RwLock::new(None),
        }
    }

    pub async fn state(&self) -> ConnectionState {
        self.state.read().await.clone()
    }

    pub async fn set_state(&self, new_state: ConnectionState) {
        match (&*self.state.read().await, &new_state) {
            (_, ConnectionState::Connected { .. }) => {
                *self.connect_count.write().await += 1;
                *self.last_connected_at.write().await = Some(std::time::Instant::now());
                *self.current_attempt.write().await = 0; // Reset on success
            }
            (ConnectionState::Connected { .. }, _) => {
                *self.disconnect_count.write().await += 1;
            }
            _ => {}
        }
        
        *self.state.write().await = new_state;
    }

    pub fn reconnect_policy(&self) -> ReconnectPolicy {
        self.reconnect_policy.clone()
    }

    pub async fn current_attempt(&self) -> u32 {
        *self.current_attempt.read().await
    }

    pub async fn increment_attempt(&self) {
        *self.current_attempt.write().await += 1;
    }

    /// 获取连接统计
    pub async fn stats(&self) -> ConnectionStats {
        ConnectionStats {
            connect_count: *self.connect_count.read().await,
            disconnect_count: *self.disconnect_count.read().await,
            current_attempt: *self.current_attempt.read().await,
            uptime_secs: self.last_connected_at.read().await
                .map(|t| t.elapsed().as_secs()),
            state: self.state.read().await.as_str().to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConnectionStats {
    pub connect_count: u64,
    pub disconnect_count: u64,
    pub current_attempt: u32,
    pub uptime_secs: Option<u64>,
    pub state: String,
}
