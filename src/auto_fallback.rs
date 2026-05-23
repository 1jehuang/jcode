//! Auto Local→Cloud Fallback — 本地算力不足时自动切云端
//!
//! 架构:
//!   ProviderRequest → HealthCheck(local)
//!     → Alive → CpuEngine::chat() (本地推理)
//!     → Dead  → DeepseekProvider::chat() (云端)
//!     → 冷却后自动恢复 Local
//!
//! 复用现有组件:
//!   - crates/jcode-cpu-inference CpuEngine (本地)
//!   - crates/jcode-llm DeepseekProvider (云端)
//!   - jcode-provider-core/failover.rs (降级策略)
//!   - jcode-agent-advanced/model_fallback.rs (降级链)

use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// 推理目标
#[derive(Debug, Clone, PartialEq)]
pub enum InferenceTarget {
    Local { model: String },
    Cloud { provider: String, model: String },
}

/// Fallback 路由器状态
#[derive(Debug, Clone)]
pub struct FallbackStatus {
    pub target: InferenceTarget,
    pub switched_at: Instant,
    pub fail_count: u32,
    pub cooldown_until: Option<Instant>,
}

/// 自动 Fallback 路由器
pub struct AutoFallbackRouter {
    local_models: Vec<String>,
    cloud_model: String,
    status: Arc<RwLock<FallbackStatus>>,
    health_check_interval: Duration,
    max_failures_before_fallback: u32,
    cooldown_secs: u64,
}

impl AutoFallbackRouter {
    pub fn new(local_models: Vec<String>, cloud_model: &str) -> Self {
        let target = if !local_models.is_empty() {
            InferenceTarget::Local { model: local_models[0].clone() }
        } else {
            InferenceTarget::Cloud { provider: "deepseek".to_string(), model: cloud_model.to_string() }
        };

        Self {
            local_models,
            cloud_model: cloud_model.to_string(),
            status: Arc::new(RwLock::new(FallbackStatus {
                target,
                switched_at: Instant::now(),
                fail_count: 0,
                cooldown_until: None,
            })),
            health_check_interval: Duration::from_secs(30),
            max_failures_before_fallback: 3,
            cooldown_secs: 120,
        }
    }

    /// 获取当前推理目标 (自动判断 local/cloud)
    /// 每次推理前调用，自动检测 local 状态并切换
    pub async fn resolve_target(&self) -> InferenceTarget {
        let status = self.status.read().await;

        // 检查冷却状态
        if let Some(cooldown) = status.cooldown_until {
            if Instant::now() < cooldown {
                return InferenceTarget::Cloud {
                    provider: "deepseek".to_string(),
                    model: self.cloud_model.clone(),
                };
            }
        }

        match &status.target {
            InferenceTarget::Local { model } => {
                // 本地模式 — 检查健康
                if status.fail_count >= self.max_failures_before_fallback {
                    // 本地失败过多 → 切云端
                    drop(status);
                    self.switch_to_cloud().await;
                    InferenceTarget::Cloud {
                        provider: "deepseek".to_string(),
                        model: self.cloud_model.clone(),
                    }
                } else {
                    InferenceTarget::Local { model: model.clone() }
                }
            }
            InferenceTarget::Cloud { .. } => {
                // 云端模式 — 检查本地是否已恢复
                if self.check_local_health_quick().await {
                    drop(status);
                    self.switch_back_to_local().await;
                    let s = self.status.read().await;
                    match &s.target {
                        InferenceTarget::Local { model } => InferenceTarget::Local { model: model.clone() },
                        _ => InferenceTarget::Cloud { provider: "deepseek".to_string(), model: self.cloud_model.clone() },
                    }
                } else {
                    InferenceTarget::Cloud {
                        provider: "deepseek".to_string(),
                        model: self.cloud_model.clone(),
                    }
                }
            }
        }
    }

    /// 报告推理失败 (触发 fallback 计数器)
    pub async fn report_failure(&self, error: &str) {
        let mut status = self.status.write().await;
        status.fail_count += 1;

        if status.fail_count >= self.max_failures_before_fallback {
            status.target = InferenceTarget::Cloud {
                provider: "deepseek".to_string(),
                model: self.cloud_model.clone(),
            };
            status.switched_at = Instant::now();
            tracing::warn!(
                "[AutoFallback] Local model failed {} times, switching to cloud. Last error: {}",
                status.fail_count, error
            );
        }
    }

    /// 报告推理成功 (重置失败计数器)
    pub async fn report_success(&self) {
        let mut status = self.status.write().await;
        status.fail_count = 0;
    }

    /// 切换到云端
    async fn switch_to_cloud(&self) {
        let mut status = self.status.write().await;
        status.target = InferenceTarget::Cloud {
            provider: "deepseek".to_string(),
            model: self.cloud_model.clone(),
        };
        status.switched_at = Instant::now();
        status.cooldown_until = Some(Instant::now() + Duration::from_secs(self.cooldown_secs));
        tracing::info!("[AutoFallback] Switched to cloud (Deepseek {})", self.cloud_model);
    }

    /// 切换回本地
    async fn switch_back_to_local(&self) {
        if let Some(model) = self.local_models.first() {
            let mut status = self.status.write().await;
            status.target = InferenceTarget::Local { model: model.clone() };
            status.fail_count = 0;
            status.cooldown_until = None;
            status.switched_at = Instant::now();
            tracing::info!("[AutoFallback] Switched back to local model: {}", model);
        }
    }

    /// 快速检查本地模型是否存活 (HTTP ping)
    async fn check_local_health_quick(&self) -> bool {
        // 检查本地 llama.cpp 进程端口
        for port in 18000..18100u16 {
            let url = format!("http://127.0.0.1:{}/health", port);
            match reqwest::get(&url).await {
                Ok(resp) if resp.status().is_success() => return true,
                _ => continue,
            }
        }
        false
    }

    /// 获取当前 Fallback 状态 (日志/展示用)
    pub async fn status_summary(&self) -> String {
        let s = self.status.read().await;
        let target_str = match &s.target {
            InferenceTarget::Local { model } => format!("local/{}", model),
            InferenceTarget::Cloud { provider, model } => format!("cloud/{}/{}", provider, model),
        };
        let cooldown = s.cooldown_until
            .map(|c| {
                let remaining = c.saturating_duration_since(Instant::now()).as_secs();
                format!("{}s", remaining)
            })
            .unwrap_or_else(|| "none".to_string());

        format!(
            "Target: {} | Failures: {}/{} | Cooldown: {} | Uptime: {:?}",
            target_str, s.fail_count, self.max_failures_before_fallback, cooldown,
            Instant::now().saturating_duration_since(s.switched_at)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_initial_target_is_local() {
        let router = AutoFallbackRouter::new(
            vec!["qwen3-72b-int4".to_string()],
            "deepseek-chat"
        );
        let target = router.resolve_target().await;
        assert!(matches!(target, InferenceTarget::Local { .. }));
    }

    #[tokio::test]
    async fn test_fallback_to_cloud_after_failures() {
        let router = AutoFallbackRouter::new(
            vec!["qwen3-72b-int4".to_string()],
            "deepseek-chat"
        );
        router.report_failure("timeout").await;
        router.report_failure("OOM").await;
        router.report_failure("crash").await;

        let target = router.resolve_target().await;
        assert!(matches!(target, InferenceTarget::Cloud { .. }));
    }

    #[test]
    fn test_no_local_models_starts_in_cloud() {
        let router = AutoFallbackRouter::new(vec![], "deepseek-chat");
        let status = router.status.blocking_read();
        assert!(matches!(status.target, InferenceTarget::Cloud { .. }));
    }
}
