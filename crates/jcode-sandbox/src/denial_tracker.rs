// ════════════════════════════════════════════════════════════════
// 拒绝追踪与自动降级 — 移植自 Claude Code denialTracking.ts
//
// 核心机制:
//   1. 连续拒绝计数 — 用户连续拒绝 N 次 → 自动降级到更严格模式
//   2. 总拒绝上限    — 累计拒绝超过 M 次 → 永久回退到手动确认
//   3. YOLO 失败追踪 — AI 分类器连续错误 → 关闭 Auto 模式
//   4. 允许重置      — 用户主动允许后清零计数器
// ════════════════════════════════════════════════════════════════

use crate::types::{PermissionMode, YoloClassificationResult};
use serde::{Deserialize, Serialize};

/// 追踪状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DenialTracker {
    /// 当前会话的连续拒绝次数
    pub consecutive_denials: u32,

    /// 历史累计总拒绝次数 (跨会话持久化)
    pub total_denials: u32,

    /// YOLO AI 分类器的连续错误次数
    pub yolo_consecutive_errors: u32,

    /// 上一次操作的时间戳 (用于超时重置)
    pub last_action_epoch_secs: u64,

    /// 是否已触发永久降级 (需要用户手动解除)
    pub permanent_degradation: bool,
}

/// 降级动作
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DegradationAction {
    /// 无需降级
    None,
    /// 从 Auto 降到 Default
    AutoToDefault { reason: String },
    /// 从 Default 降到 Plan
    DefaultToPlan { reason: String },
    /// 永久禁用 YOLO (AI 分类器不可靠)
    DisableYolo { reason: String },
    /// 需要用户干预才能恢复
    RequireManualReset { reason: String },
}

impl Default for DenialTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl DenialTracker {
    pub fn new() -> Self {
        Self {
            consecutive_denials: 0,
            total_denials: 0,
            yolo_consecutive_errors: 0,
            last_action_epoch_secs: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            permanent_degradation: false,
        }
    }

    // ─── 记录操作结果 ─────────────────────────────────

    /// 记录一次用户拒绝
    pub fn record_denial(&mut self) -> DegradationAction {
        self.consecutive_denials += 1;
        self.total_denials += 1;
        self.touch();

        tracing::warn!(
            consecutive = self.consecutive_denials,
            total = self.total_denials,
            "User denial recorded"
        );

        // 检查是否达到永久降级阈值
        if self.total_denials >= crate::MAX_TOTAL_DENIALS {
            self.permanent_degradation = true;
            return DegradationAction::RequireManualReset {
                reason: format!(
                    "累计拒绝 {} 次 (上限 {}), 已永久降级为手动确认模式. \
                     可通过 'accept' 命令或设置面板手动重置.",
                    self.total_denials,
                    crate::MAX_TOTAL_DENIALS
                ),
            };
        }

        // 检查是否达到连续拒绝阈值
        if self.consecutive_denials >= crate::DEFAULT_DENIAL_THRESHOLD {
            let reason = format!(
                "连续拒绝 {} 次, 自动从当前模式降级为更严格的确认模式",
                self.consecutive_denials
            );
            // 不在这里直接改模式, 只是建议降级
            if self.consecutive_denials == crate::DEFAULT_DENIAL_THRESHOLD {
                return DegradationAction::AutoToDefault { reason };
            } else if self.consecutive_denials == crate::DEFAULT_DENIAL_THRESHOLD * 2 {
                return DegradationAction::DefaultToPlan { reason };
            }
        }

        DegradationAction::None
    }

    /// 记录一次用户允许 (重置连续计数)
    pub fn record_allow(&mut self) {
        if self.consecutive_denials > 0 {
            tracing::info!(
                reset_from = self.consecutive_denials,
                "Consecutive denials reset by user allow"
            );
        }
        self.consecutive_denials = 0;
        self.touch();
    }

    /// 记录一次 YOLO 错误
    pub fn record_yolo_error(&mut self) -> DegradationAction {
        self.yolo_consecutive_errors += 1;
        self.touch();

        if self.yolo_consecutive_errors >= crate::YOLO_AUTO_FALLBACK_AFTER {
            DegradationAction::DisableYolo {
                reason: format!(
                    "YOLO 分类器连续错误 {} 次, 已自动关闭 AI 自动分类",
                    self.yolo_consecutive_errors
                ),
            }
        } else {
            DegradationAction::None
        }
    }

    /// 记录一次 YOLO 成功 (重置错误计数)
    pub fn record_yolo_success(&mut self) {
        self.yolo_consecutive_errors = 0;
    }

    // ─── 查询状态 ─────────────────────────────────────

    /// 获取推荐的权限模式 (基于追踪状态)
    pub fn recommended_mode(
        &self,
        current_mode: PermissionMode,
    ) -> PermissionMode {
        if self.permanent_degradation {
            return PermissionMode::Default; // 最安全模式
        }

        match current_mode {
            PermissionMode::Auto => {
                // 如果连续拒绝过多, 降级
                if self.consecutive_denials >= crate::DEFAULT_DENIAL_THRESHOLD * 2 {
                    PermissionMode::Default
                } else if self.consecutive_denials >= crate::DEFAULT_DENIAL_THRESHOLD {
                    PermissionMode::Default
                } else {
                    current_mode
                }
            }
            _ => current_mode,
        }
    }

    /// 检查是否应该禁用 YOLO
    pub fn should_disable_yolo(&self) -> bool {
        self.yolo_consecutive_errors >= crate::YOLO_AUTO_FALLBACK_AFTER
    }

    /// 手动重置所有计数器 (用户显式操作)
    pub fn manual_reset(&mut self) {
        self.consecutive_denials = 0;
        self.total_denials = 0; // 可选: 也重置总数
        self.yolo_consecutive_errors = 0;
        self.permanent_degradation = false;
        self.touch();
        tracing::info!("Denial tracker manually reset");
    }

    /// 序列化用于持久化
    pub fn to_persistent_state(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// 从持久化恢复
    pub fn from_persistent_state(state: &str) -> Option<Self> {
        serde_json::from_str(state).ok()
    }

    fn touch(&mut self) {
        self.last_action_epoch_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_consecutive_denials_trigger() {
        let mut t = DenialTracker::new();

        for i in 0..crate::DEFAULT_DENIAL_THRESHOLD {
            assert_eq!(t.record_denial(), DegradationAction::None, "denial #{} should not trigger", i + 1);
        }

        // 第 DEFAULT_DENIAL_THRESHOLD + 1 次应触发
        match t.record_denial() {
            DegradationAction::AutoToDefault { .. } => {}
            other => panic!("Expected AutoToDefault, got {:?}", other),
        }
    }

    #[test]
    fn test_allow_resets_counter() {
        let mut t = DenialTracker::new();

        t.record_denial();
        t.record_denial();
        assert_eq!(t.consecutive_denials, 2);

        t.record_allow();
        assert_eq!(t.consecutive_denials, 0);

        // 再次拒绝应从头开始计数
        t.record_denial();
        assert_eq!(t.consecutive_denials, 1);
    }

    #[test]
    fn test_permanent_degradation() {
        let mut t = DenialTracker::new();

        for _ in 0..crate::MAX_TOTAL_DENIALS {
            t.record_denial();
        }

        match t.record_denial() {
            DegradationAction::RequireManualReset { .. } => {}
            other => panic!("Expected RequireManualReset, got {:?}", other),
        }
        assert!(t.permanent_degradation);
    }

    #[test]
    fn test_yolo_error_tracking() {
        let mut t = DenialTracker::new();

        for i in 0..crate::YOLO_AUTO_FALLBACK_AFTER {
            assert_eq!(
                t.record_yolo_error(),
                DegradationAction::None,
                "yolo error #{} should not trigger",
                i + 1
            );
        }

        match t.record_yolo_error() {
            DegradationAction::DisableYolo { .. } => {}
            other => panic!("Expected DisableYolo, got {:?}", other),
        }
    }

    #[test]
    fn test_manual_reset() {
        let mut t = DenialTracker::new();
        for _ in 0..10 {
            t.record_denial();
        }
        t.permanent_degradation = true;

        t.manual_reset();

        assert_eq!(t.consecutive_denials, 0);
        assert!(!t.permanent_degradation);
    }

    #[test]
    fn test_serialization_roundtrip() {
        let mut original = DenialTracker::new();
        original.record_denial();
        original.record_denial();
        original.record_yolo_error();

        let state = original.to_persistent_state();
        let restored = DenialTracker::from_persistent_state(&state).expect("should deserialize");

        assert_eq!(restored.consecutive_denials, 2);
        assert_eq!(restored.yolo_consecutive_errors, 1);
    }
}
