// ════════════════════════════════════════════════════════════════
// 模型降级管理系统
// 对应 Claude Code: query.ts L894-951 FallbackTriggeredError 处理
//
// 核心能力:
//   1. 多级降级链 — 主模型 -> 备选1 -> 备选2 -> ... -> 最终兜底
//   2. 智能降级触发 — 仅对特定错误触发 (过载/超时/限流)
//   3. 降级状态追踪 — 记录每次降级的原因和目标
//   4. 自动恢复 — 主模型恢复后切回
// ════════════════════════════════════════════════════════════════

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use super::{ErrorCategory, TerminalState};
use crate::types::LoopEvent;

/// 触发模型降级的错误条件
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FallbackTrigger {
    /// 模型过载 (529 / overloaded)
    ModelOverloaded,
    /// API 超时
    Timeout,
    /// 速率限制
    RateLimited,
    /// 服务端内部错误
    ServerError(u16),
    /// 连接失败
    ConnectionFailed,
}

impl std::fmt::Display for FallbackTrigger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ModelOverloaded => write!(f, "模型过载"),
            Self::Timeout => write!(f, "API 超时"),
            Self::RateLimited => write!(f, "速率限制"),
            Self::ServerError(code) => write!(f, "服务端错误 {code}"),
            Self::ConnectionFailed => write!(f, "连接失败"),
        }
    }
}

impl From<&ErrorCategory> for Option<FallbackTrigger> {
    fn from(category: &ErrorCategory) -> Self {
        match category {
            ErrorCategory::ModelErrorOverloaded => Some(FallbackTrigger::ModelOverloaded),
            ErrorCategory::NetworkTimeout => Some(FallbackTrigger::Timeout),
            ErrorCategory::RateLimited { .. } => Some(FallbackTrigger::RateLimited),
            ErrorCategory::ServerError(code) => Some(FallbackTrigger::ServerError(*code)),
            ErrorCategory::Unknown => Some(FallbackTrigger::ConnectionFailed),
            _ => None,  // auth/permission/budget 等不应触发降级
        }
    }
}

/// 单次降级记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FallbackRecord {
    pub from_model: String,
    pub to_model: String,
    pub trigger: FallbackTrigger,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub attempt_number: u32,
}

/// 降级链配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FallbackChain {
    /// 有序的备选模型列表
    pub models: Vec<String>,
    
    /// 哪些触发条件允许降级
    pub allowed_triggers: Vec<FallbackTrigger>,
    
    /// 同一请求最大降级次数
    pub max_depth: usize,
    
    /// 冷却时间 — 降级后多久可以尝试恢复主模型 (秒)
    pub cooldown_secs: u64,
}

impl FallbackChain {
    pub fn new(models: Vec<String>) -> Self {
        Self {
            models,
            allowed_triggers: vec![
                FallbackTrigger::ModelOverloaded,
                FallbackTrigger::Timeout,
                FallbackTrigger::RateLimited,
                FallbackTrigger::ServerError(503),
                FallbackTrigger::ConnectionFailed,
            ],
            max_depth: MAX_FALLBACK_DEPTH,
            cooldown_secs: 300,  // 5 分钟冷却
        }
    }
    
    /// 获取指定深度的备选模型
    pub fn get_fallback(&self, depth: usize) -> Option<&str> {
        if depth < self.models.len() {
            self.models.get(depth).map(|s| s.as_str())
        } else {
            None
        }
    }
    
    /// 判断是否应该触发降级
    pub fn should_trigger(&self, trigger: &FallbackTrigger) -> bool {
        self.allowed_triggers.contains(trigger)
    }
}

/// 模型降级管理器
pub struct ModelFallbackManager {
    /// 降级链配置
    chain: FallbackChain,
    
    /// 当前降级深度 (0 = 使用主模型)
    current_depth: usize,
    
    /// 当前使用的模型 ID
    current_model: String,
    
    /// 主模型 ID
    primary_model: String,
    
    /// 降级历史记录
    history: Arc<RwLock<VecDeque<FallbackRecord>>>,
    
    /// 上次降级时间 (用于冷却检测)
    last_fallback_time: Arc<RwLock<Option<Instant>>>,
    
    /// 是否启用自动恢复到主模型
    enable_auto_recovery: bool,
    
    /// 事件回调
    on_event: Option<Box<dyn Fn(LoopEvent) + Send + Sync>>,
}

impl ModelFallbackManager {
    pub fn new(primary_model: String, fallback_chain: FallbackChain) -> Self {
        let current_model = primary_model.clone();
        Self {
            chain: fallback_chain,
            current_depth: 0,
            current_model,
            primary_model,
            history: Arc::new(RwLock::new(VecDeque::with_capacity(20))),
            last_fallback_time: Arc::new(RwLock::new(None)),
            enable_auto_recovery: true,
            on_event: None,
        }
    }
    
    /// 设置事件回调
    pub fn on_event<F>(&mut self, callback: F) 
    where
        F: Fn(LoopEvent) + Send + Sync + 'static,
    {
        self.on_event = Some(Box::new(callback));
    }
    
    /// 获取当前应使用的模型
    pub fn current_model(&self) -> &str {
        &self.current_model
    }
    
    /// 获取当前降级深度
    pub fn depth(&self) -> usize {
        self.current_depth
    }
    
    /// 是否正在使用降级后的模型
    pub fn is_fallback_active(&self) -> bool {
        self.current_depth > 0
    }
    
    /// 尝试触发降级
    /// 
    /// 返回:
    /// - `Ok(Some(model))` — 成功降级到指定模型
    /// - `Ok(None)` — 不需要降级 (主模型正常)
    /// - `Err(state)` — 所有备选模型都已耗尽，返回终止状态
    pub async fn try_fallback(
        &mut self, 
        trigger: FallbackTrigger, 
        error_category: &ErrorCategory
    ) -> Result<Option<String>, TerminalState> {
        
        // 检查是否允许该触发条件
        if !self.chain.should_trigger(&trigger) {
            tracing::debug!(
                trigger = %trigger,
                "Fallback not allowed for this trigger type"
            );
            return Ok(None);
        }
        
        // 检查是否已达到最大降级深度
        if self.current_depth >= self.chain.max_depth {
            tracing::error!(
                depth = self.current_depth,
                max = self.chain.max_depth,
                "All fallback models exhausted"
            );
            
            // 发送事件
            self.emit_event(LoopEvent::Warning {
                level: super::WarningLevel::Error,
                message: format!("所有 {} 个备用模型均已失败", self.chain.max_depth),
            });
            
            return Err(TerminalState::Error {
                message: format!("所有模型均不可用 ({}次降级后)", self.chain.max_depth),
                recoverable: false,
            });
        }
        
        // 获取下一个备选模型
        let next_depth = self.current_depth + 1;
        let fallback_model = match self.chain.get_fallback(next_depth) {
            Some(m) => m.to_string(),
            None => {
                return Err(TerminalState::Error {
                    message: "降级链中没有更多可用模型".to_string(),
                    recoverable: false,
                });
            }
        };
        
        // 执行降级
        let previous_model = self.current_model.clone();
        self.current_model = fallback_model.clone();
        self.current_depth = next_depth;
        
        // 更新时间戳
        *self.last_fallback_time.write().await = Some(Instant::now());
        
        // 记录历史
        let record = FallbackRecord {
            from_model: previous_model.clone(),
            to_model: fallback_model.clone(),
            trigger: trigger.clone(),
            timestamp: chrono::Utc::now(),
            attempt_number: next_depth as u32,
        };
        self.history.write().await.push_back(record);
        
        // 发送事件
        self.emit_event(LoopEvent::ModelFallbackTriggered {
            from: previous_model,
            to: fallback_model.clone(),
            reason: trigger.clone(),
        });
        
        tracing::warn!(
            from = %previous_model,
            to = %fallback_model,
            depth = next_depth,
            trigger = %trigger,
            "Model fallback triggered"
        );
        
        Ok(Some(fallback_model))
    }
    
    /// 尝试恢复到主模型 (如果冷却期已过)
    pub async fn try_recover(&mut self) -> Option<String> {
        if !self.enable_auto_recovery || self.current_depth == 0 {
            return None;
        }
        
        // 检查冷却期
        let should_recover = {
            let last = self.last_fallback_time.read().await;
            match *last {
                Some(t) => t.elapsed() > Duration::from_secs(self.chain.cooldown_secs),
                None => true,
            }
        };
        
        if should_recover {
            let previous_model = self.current_model.clone();
            self.current_model = self.primary_model.clone();
            self.current_depth = 0;
            
            tracing::info!(
                from = %previous_model,
                to = %self.primary_model,
                "Recovered to primary model"
            );
            
            Some(self.primary_model.clone())
        } else {
            None
        }
    }
    
    /// 重置降级状态 (新会话开始时调用)
    pub async fn reset(&mut self) {
        self.current_depth = 0;
        self.current_model = self.primary_model.clone();
        *self.last_fallback_time.write().await = None;
    }
    
    /// 获取降级历史
    pub async fn history(&self) -> Vec<FallbackRecord> {
        self.history.read().await.iter().cloned().collect()
    }
    
    fn emit_event(&self, event: LoopEvent) {
        if let Some(ref cb) = self.on_event {
            cb(event);
        }
    }
}
