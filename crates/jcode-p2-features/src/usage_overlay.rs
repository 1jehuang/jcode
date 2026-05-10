// ════════════════════════════════════════════════════════════════
// Token/费用 实时覆盖层 — Usage Overlay
//
// 实时显示:
//   - Token 消耗 (input/output/total)
//   - 当前会话费用估算
//   - 速率限制状态 (rate limit)
//   - 历史使用趋势
//   - 预算警告
// ════════════════════════════════════════════════════════════════

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    /// 输入 token 数
    pub input_tokens: u64,
    /// 输出 token 数
    pub output_tokens: u64,
    /// 缓存读取命中 (节省的 input tokens)
    pub cache_read_tokens: u64,
    /// 缓存写入
    pub cache_write_tokens: u64,
}

impl TokenUsage {
    pub fn total(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }
    
    /// 估算成本 (按 GPT-4o 定价)
    pub fn estimated_cost_usd(&self) -> f64 {
        // Input: $2.50 / 1M tokens, Output: $10 / 1M tokens
        let input_cost = self.input_tokens as f64 * 2.50 / 1_000_000.0;
        let output_cost = self.output_tokens as f64 * 10.0 / 1_000_000.0;
        let cache_read_cost = self.cache_read_tokens as f64 * 0.30 / 1_000_000.0;
        
        input_cost + output_cost + cache_read_cost
    }

    /// 格式化为可读字符串
    pub fn display(&self) -> String {
        let mut parts = vec![format!("{} in / {} out", self.input_tokens, self.output_tokens)];
        if self.cache_read_tokens > 0 {
            parts.push(format!("cache hit: {}", self.cache_read_tokens));
        }
        if self.total() > 0 {
            parts.push(format!("total: {}", self.total()));
            parts.push(format!("${:.4}", self.estimated_cost_usd()));
        }
        parts.join(" | ")
    }
}

/// 使用统计 (聚合)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageStats {
    /// 总请求数
    pub total_requests: u64,
    /// 总 Token 消耗
    pub total_tokens: TokenUsage,
    /// 总费用
    pub total_cost_usd: f64,
    /// 会话开始时间
    pub session_start: chrono::DateTime<chrono::Utc>,
    /// 最后更新时间
    pub last_update: chrono::DateTime<chrono::Utc>,
    /// 各模型的分布
    pub by_model: std::collections::HashMap<String, ModelUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelUsage {
    pub model_name: String,
    pub requests: u64,
    pub tokens: TokenUsage,
    pub cost_usd: f64,
}

/// 速率限制状态
#[derive(Debug, Clone)]
pub struct RateLimitStatus {
    pub remaining_requests: u32,
    pub reset_at: Option<chrono::DateTime<chrono::Utc>>,
    pub is_limited: bool,
}

/// 预算配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetConfig {
    /// 最大预算 ($), 0 = 无限制
    pub max_budget_usd: f64,
    /// 最大 Token 数, 0 = 无限制
    pub max_tokens: u64,
    /// 警告阈值 (达到此百分比时发出警告)
    pub warn_threshold_pct: f64,
    /// 超过预算时的行为
    pub over_budget_action: OverBudgetAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OverBudgetAction {
    WarnOnly,       // 仅警告，不阻止
    BlockNewRequests, // 阻止新请求
    SwitchToCheaperModel, // 切换到更便宜的模型
}

impl Default for BudgetConfig {
    fn default() -> Self {
        Self {
            max_budget_usd: 10.0,
            max_tokens: 5_000_000,
            warn_threshold_pct: 80.0,
            over_budget_action: OverBudgetAction::WarnOnly,
        }
    }
}

/// 使用量覆盖层主结构
pub struct UsageOverlay {
    stats: Arc<RwLock<UsageStats>>,
    budget: Arc<RwLock<BudgetConfig>>,
    rate_limit: Arc<RwLock<RateLimitStatus>>,
    history: Arc<RwLock<Vec<TokenUsage>>>, // 每 request 的 token 记录
}

impl Default for UsageOverlay {
    fn default() -> Self { Self::new() }
}

impl UsageOverlay {
    pub fn new() -> Self {
        Self {
            stats: Arc::new(RwLock::new(UsageStats {
                session_start: chrono::Utc::now(),
                last_update: chrono::Utc::now(),
                ..Default::default()
            })),
            budget: Arc::new(RwLock::new(BudgetConfig::default())),
            rate_limit: Arc::new(RwLock::new(RateLimitStatus {
                remaining_requests: 1000,
                reset_at: None,
                is_limited: false,
            })),
            history: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// 记录一次 API 调用的 Token 使用
    pub async fn record_usage(&self, usage: TokenUsage, model: &str) {
        let cost = usage.estimated_cost_usd();
        
        {
            let mut stats = self.stats.write().await;
            stats.total_requests += 1;
            stats.total_tokens.input_tokens += usage.input_tokens;
            stats.total_tokens.output_tokens += usage.output_tokens;
            stats.total_tokens.cache_read_tokens += usage.cache_read_tokens;
            stats.total_cost_usd += cost;
            stats.last_update = chrono::Utc::now();

            stats.by_model
                .entry(model.to_string())
                .or_insert_with(|| ModelUsage {
                    model_name: model.to_string(),
                    requests: 0,
                    tokens: TokenUsage { ..Default::default() },
                    cost_usd: 0.0,
                })
                .requests += 1;
            
            if let Some(m) = stats.by_model.get_mut(model) {
                m.tokens.input_tokens += usage.input_tokens;
                m.tokens.output_tokens += usage.output_tokens;
                m.cost_usd += cost;
            }
        }

        self.history.write().await.push(usage);
    }

    /// 更新速率限制状态
    pub async fn update_rate_limit(&self, remaining: u32, reset_at: Option<chrono::DateTime<chrono::Utc>>) {
        let mut rl = self.rate_limit.write().await;
        rl.remaining_requests = remaining;
        rl.reset_at = reset_at;
        rl.is_limited = remaining < 10; // 低于 10 则视为受限
    }

    /// 设置预算配置
    pub async fn set_budget(&self, config: BudgetConfig) {
        *self.budget.write().await = config;
    }

    /// 检查是否超出预算
    pub async fn check_budget(&self) -> BudgetCheckResult {
        let stats = self.stats.read().await;
        let budget = self.budget.read().await;

        if budget.max_budget_usd > 0.0 {
            let pct = (stats.total_cost_usd / budget.max_budget_usd) * 100.0;
            if pct >= 100.0 {
                return BudgetCheckResult { exceeded: true, warning: false, percentage: pct };
            } else if pct >= budget.warn_threshold_pct {
                return BudgetCheckResult { exceeded: false, warning: true, percentage: pct };
            }
        }

        if budget.max_tokens > 0 && stats.total_tokens.total() >= budget.max_tokens {
            return BudgetCheckResult { exceeded: true, warning: false, percentage: 100.0 };
        }

        BudgetCheckResult { exceeded: false, warning: false, percentage: 0.0 }
    }

    /// 获取当前使用摘要 (用于 UI 显示)
    pub async fn get_display_summary(&self) -> String {
        let stats = self.stats.read().await;
        let budget_check = self.check_budget().await;
        let rate_limit = self.rate_limit.read().await;
        let elapsed = (chrono::Utc::now() - stats.session_start).num_seconds();

        format!(
            "💰 ${:.4} | 🪙 {} | ⏱ {}s | 📊 reqs={} | {}{}",
            stats.total_cost_usd,
            stats.total_tokens.display(),
            elapsed,
            stats.total_requests,
            if rate_limit.is_limited { format!("⚠️ Rate limited ({}) ", rate_limit.remaining_requests) } else { String::new() },
            match budget_check.exceeded { true => "🚫 OVER BUDGET", false => "" }
        )
    }

    /// 获取完整统计
    pub async fn get_stats(&self) -> UsageStats {
        self.stats.read().await.clone()
    }

    /// 重置统计
    pub async fn reset_stats(&self) {
        *self.stats.write().await = UsageStats {
            session_start: chrono::Utc::now(),
            last_update: chrono::Utc::now(),
            ..Default::default()
        };
        self.history.write().await.clear();
    }
}

#[derive(Debug, Clone)]
pub struct BudgetCheckResult {
    pub exceeded: bool,
    pub warning: bool,
    pub percentage: f64,
}
