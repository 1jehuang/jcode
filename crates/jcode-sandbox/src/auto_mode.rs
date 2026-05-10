// ════════════════════════════════════════════════════════════════
// AutoMode 状态机 — 移植自 Claude Code autoModeState.ts
//
// Auto 模式的完整状态转换:
//
//   ┌──────────┐  用户允许    ┌──────────┐
//   │ Inactive │ ──────────→ │ Active   │
//   │ (关闭)   │ ←────────── │ (激活中) │
//   └──────────┘  用户拒绝    └────┬─────┘
//                              │ 超时/错误
//                              ▼
//                         ┌──────────┐
//                         │ Cooldown  │
//                         │ (冷却期)  │
//                         └──────────┘
//
// 状态转换规则:
// - Inactive → Active: 用户显式开启 auto mode
// - Active → Inactive: 用户手动关闭, 或连续 N 次拒绝后自动关闭
// - Active → Cooldown: YOLO 错误/超时/不确定结果 → 进入冷却等待用户确认
// - Cooldown → Active: 冷却期内收到一次用户确认
// - Cooldown → Inactive: 冷却期超时无确认, 回退到 Default
// ════════════════════════════════════════════════════════════════

use crate::types::PermissionMode;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// AutoMode 的内部状态
// Note: 不实现 Serialize/Deserialize，因为包含 std::time::Instant（运行时状态）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutoModeState {
    /// 自动模式未启用
    Inactive,
    /// 自动模式激活中 — YOLO 分类器正在做决策
    Active {
        /// 激活时间
        since: Instant,
        /// 连续成功决策数 (用于信任累积)
        consecutive_successes: u32,
        /// 总决策次数
        total_decisions: u32,
    },
    /// 冷却期 — 因错误/不确定性进入暂停
    Cooldown {
        /// 进入冷却的原因
        reason: String,
        /// 冷却开始时间
        since: Instant,
        /// 冷却持续时间
        duration: Duration,
    },
}

/// 触发状态转换的事件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AutoModeEvent {
    /// 用户请求开启 Auto mode
    UserActivate,
    /// 用户请求关闭 Auto mode
    UserDeactivate,
    /// YOLO 成功分类并允许
    YoloAllow { confidence: f64 },
    /// YOLO 成功分类但需要确认
    YoloAsk { confidence: f64 },
    /// YOLO 阻止操作
    YoloBlock { confidence: f64, reason: String },
    /// YOLO 分类失败 (LLM 错误)
    YoloError { error: String },
    /// 用户在 Auto 模式下手动确认了操作
    UserConfirmWhileActive,
    /// 用户在 Auto 模式下拒绝了操作
    UserRejectWhileActive,
    /// 冷却期超时
    CooldownExpired,
}

/// 状态机转换结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransitionResult {
    /// 状态已变更
    Changed { from: AutoModeState, to: AutoModeState, action: Option<String> },
    /// 无变化 (事件被忽略或当前状态不处理)
    NoChange,
}

/// AutoMode 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoModeConfig {
    /// 冷却期默认时长 (秒)
    pub cooldown_secs: u64,

    /// 连续拒绝多少次后自动退出 Auto
    pub auto_exit_on_rejects: u32,

    /// 连续成功多少次后提高信任 (可降低温度)
    pub trust_threshold: u32,

    /// 最大活跃时长 (0 = 不限制)
    pub max_active_secs: u64,
}

impl Default for AutoModeConfig {
    fn default() -> Self {
        Self {
            cooldown_secs: 60,
            auto_exit_on_rejects: 3,
            trust_threshold: 10,
            max_active_secs: 0, // 不限制
        }
    }
}

pub struct AutoModeStateMachine {
    state: AutoModeState,
    config: AutoModeConfig,
}

impl Default for AutoModeStateMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl AutoModeStateMachine {
    pub fn new() -> Self {
        Self {
            state: AutoModeState::Inactive,
            config: AutoModeConfig::default(),
        }
    }

    pub fn with_config(config: AutoModeConfig) -> Self {
        Self { config, ..Self::new() }
    }

    /// 处理事件并返回状态转换结果
    pub fn handle_event(&mut self, event: AutoModeEvent) -> TransitionResult {
        let old_state = self.state.clone();

        match (&self.state, event) {
            // ── Inactive 状态 ──
            (AutoModeState::Inactive, AutoModeEvent::UserActivate) => {
                self.state = AutoModeState::Active {
                    since: Instant::now(),
                    consecutive_successes: 0,
                    total_decisions: 0,
                };
            }

            // ── Active 状态 ──
            (
                AutoModeState::Active {
                    since,
                    consecutive_successes,
                    total_decisions,
                },
                event,
            ) => {
                let mut new_successes = *consecutive_successes;
                let new_total = total_decisions.saturating_add(1);
                let mut new_state = None;

                match event {
                    AutoModeEvent::UserDeactivate => {
                        self.state = AutoModeState::Inactive;
                        return self.transition_result(old_state);
                    }

                    AutoModeEvent::YoloAllow { .. } => {
                        new_successes = new_successes.saturating_add(1);
                        tracing::debug!(
                            successes = new_successes,
                            "YOLO allowed"
                        );
                    }

                    AutoModeEvent::YoloAsk { confidence } => {
                        if confidence < 0.5 {
                            // 低置信度 → 冷却
                            new_state = Some(AutoModeState::Cooldown {
                                reason: format!(
                                    "YOLO 低置信度 ({:.1}%), 需要人工确认",
                                    confidence * 100.0
                                ),
                                since: Instant::now(),
                                duration: Duration::from_secs(self.config.cooldown_secs),
                            });
                        }
                        // 高置信度的 ask 可以继续 (信任模式)
                        new_successes = new_successes.saturating_add(1);
                    }

                    AutoModeEvent::YoloBlock { reason, confidence } => {
                        new_state = Some(AutoModeState::Cooldown {
                            reason: format!("YOLO 阻止 (置信度 {:.0}%): {}", confidence * 100.0, reason),
                            since: Instant::now(),
                            duration: Duration::from_secs(self.config.cooldown_secs),
                        });
                    }

                    AutoModeEvent::YoloError { error } => {
                        new_state = Some(AutoModeState::Cooldown {
                            reason: format!("YOLO 错误: {}", error),
                            since: Instant::now(),
                            duration: Duration::from_secs(self.config.cooldown_secs),
                        });
                    }

                    AutoModeEvent::UserRejectWhileActive => {
                        if new_successes > 0 && (new_total - new_successes) >= self.config.auto_exit_on_rejects {
                            // 连续拒绝过多 → 退出 Auto
                            self.state = AutoModeState::Inactive;
                            return TransitionResult::Changed {
                                from: old_state,
                                to: AutoModeState::Inactive,
                                action: Some(
                                    format!(
                                        "因连续 {} 次拒绝, 自动退出 Auto 模式",
                                        self.config.auto_exit_on_rejects
                                    )
                                ),
                            };
                        }
                        new_successes = 0; // 重置连续成功计数
                    }

                    AutoModeEvent::UserConfirmWhileActive => {
                        new_successes = new_successes.saturating_add(1);
                    }

                    _ => {}
                }

                // 检查最大活跃时长
                if new_state.is_none() {
                    if self.config.max_active_secs > 0
                        && since.elapsed().as_secs() > self.config.max_active_secs
                    {
                        new_state = Some(AutoModeState::Cooldown {
                            reason: "Auto 模式达到最大活跃时长".into(),
                            since: Instant::now(),
                            duration: Duration::from_secs(self.config.cooldown_secs / 2), // 较短冷却
                        });
                    }
                }

                if let Some(ns) = new_state {
                    self.state = ns;
                    return self.transition_result(old_state);
                }

                // 仅更新内部计数器 (不改变状态)
                if let AutoModeState::Active { consecutive_successes, total_decisions, .. } =
                    &mut self.state
                {
                    *consecutive_successes = new_successes;
                    *total_decisions = new_total;
                }
                return TransitionResult::NoChange;
            }

            // ── Cooldown 状态 ──
            (AutoModeState::Cooldown { since, duration, .. }, event) => {
                match event {
                    AutoModeEvent::UserDeactivate => {
                        self.state = AutoModeState::Inactive;
                        return self.transition_result(old_state);
                    }
                    AutoModeEvent::UserConfirmWhileActive | AutoModeEvent::UserActivate => {
                        // 用户确认 → 重新进入 Active
                        self.state = AutoModeState::Active {
                            since: Instant::now(),
                            consecutive_successes: 0,
                            total_decisions: 0,
                        };
                        return self.transition_result(old_state);
                    }
                    AutoModeEvent::CooldownExpired => {
                        if since.elapsed() >= *duration {
                            self.state = AutoModeState::Inactive;
                            return self.transition_result(old_state);
                        }
                    }
                    _ => {} // Cooldown 期间忽略其他事件
                }
            }

            // 其他无效的状态+事件组合 → 忽略
            _ => {}
        }

        TransitionResult::NoChange
    }

    /// 获取当前状态引用
    pub fn state(&self) -> &AutoModeState {
        &self.state
    }

    /// 是否处于活跃状态 (Auto 模式可用)
    pub fn is_active(&self) -> bool {
        matches!(self.state, AutoModeState::Active { .. })
    }

    /// 是否处于冷却期
    pub fn is_in_cooldown(&self) -> bool {
        matches!(self.state, AutoModeState::Cooldown { .. })
    }

    /// 获取当前权限模式的建议值
    pub fn suggested_permission_mode(&self) -> PermissionMode {
        match &self.state {
            AutoModeState::Active { .. } => PermissionMode::Auto,
            _ => PermissionMode::Default,
        }
    }

    /// 手动重置为 Inactive
    pub fn reset(&mut self) {
        self.state = AutoModeState::Inactive;
    }

    fn transition_result(&self, from: AutoModeState) -> TransitionResult {
        TransitionResult::Changed {
            from,
            to: self.state.clone(),
            action: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_activate_from_inactive() {
        let mut sm = AutoModeStateMachine::new();
        assert!(!sm.is_active());

        let result = sm.handle_event(AutoModeEvent::UserActivate);

        matches!(result, TransitionResult::Changed { .. });
        assert!(sm.is_active());
    }

    #[test]
    fn test_deactivate_from_active() {
        let mut sm = AutoModeStateMachine::new();
        sm.handle_event(AutoModeEvent::UserActivate);
        assert!(sm.is_active());

        sm.handle_event(AutoModeEvent::UserDeactivate);
        assert!(!sm.is_active());
    }

    #[test]
    fn test_yolo_block_triggers_cooldown() {
        let mut sm = AutoModeStateMachine::new();
        sm.handle_event(AutoModeEvent::UserActivate);

        let result = sm.handle_event(AutoModeEvent::YoloBlock {
            confidence: 0.95,
            reason: "危险操作".into(),
        });

        matches!(result, TransitionResult::Changed { .. });
        assert!(sm.is_in_cooldown());
    }

    #[test]
    fn test_yolo_error_triggers_cooldown() {
        let mut sm = AutoModeStateMachine::new();
        sm.handle_event(AutoModeEvent::UserActivate);

        let result = sm.handle_event(AutoModeEvent::YoloError {
            error: "API timeout".into(),
        });

        assert!(matches!(result, TransitionResult::Changed { .. }));
        assert!(sm.is_in_cooldown());
    }

    #[test]
    fn test_consecutive_rejects_exit_auto() {
        let mut sm = AutoModeStateMachine::new();
        sm.handle_event(AutoModeEvent::UserActivate);

        // 前两次拒绝不应退出 (阈值=3)
        for _ in 0..2 {
            sm.handle_event(AutoModeEvent::UserRejectWhileActive);
        }
        assert!(sm.is_active(), "前两次拒绝不应退出 Auto");

        // 第三次拒绝应退出
        let result = sm.handle_event(AutoModeEvent::UserRejectWhileActive);
        assert!(!sm.is_active());
        matches!(result, TransitionResult::Changed { action: Some(_), .. });
    }

    #[test]
    fn test_confirm_during_cooldown_reactivates() {
        let mut sm = AutoModeStateMachine::new();
        sm.handle_event(AutoModeEvent::UserActivate);
        sm.handle_event(AutoModeEvent::YoloBlock { confidence: 0.9, reason: "test".into() });
        assert!(sm.is_in_cooldown());

        sm.handle_event(AutoModeEvent::UserConfirmWhileActive);
        assert!(sm.is_active());
    }

    #[test]
    fn test_suggested_mode() {
        let sm = AutoModeStateMachine::new();
        assert_eq!(sm.suggested_permission_mode(), PermissionMode::Default);

        let mut sm2 = AutoModeStateMachine::new();
        sm2.handle_event(AutoModeEvent::UserActivate);
        assert_eq!(sm2.suggested_permission_mode(), PermissionMode::Auto);
    }

    #[test]
    fn test_reset() {
        let mut sm = AutoModeStateMachine::new();
        sm.handle_event(AutoModeEvent::UserActivate);
        sm.handle_event(AutoModeEvent::YoloError { error: "x".into() });

        sm.reset();
        assert!(!sm.is_active());
        assert!(!sm.is_in_cooldown());
    }
}
