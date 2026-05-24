//! Auto Local→Cloud Fallback Router
//!
//! Automatically switches inference target when local compute is insufficient:
//!
//! ```text
//! ProviderRequest → HealthCheck(local)
//!   → Alive → CpuEngine::chat()       (local inference)
//!   → Dead  → DeepseekProvider::chat() (cloud inference)
//!   → Cooldown → Auto-recover local after cooldown period
//! ```
//!
//! ## Reused Components
//!
//! - `crates/jcode-cpu-inference` CpuEngine (local)
//! - `crates/jcode-llm` DeepseekProvider (cloud)
//! - Health check via HTTP ping on llama.cpp ports

use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Inference target
#[derive(Debug, Clone, PartialEq)]
pub enum InferenceTarget {
    Local { model: String },
    Cloud { provider: String, model: String },
}

/// Fallback router status
#[derive(Debug, Clone)]
pub struct FallbackStatus {
    pub target: InferenceTarget,
    pub switched_at: Instant,
    pub fail_count: u32,
    pub cooldown_until: Option<Instant>,
}

/// Automatic fallback router — manages Local ↔ Cloud switching
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
            InferenceTarget::Local {
                model: local_models[0].clone(),
            }
        } else {
            InferenceTarget::Cloud {
                provider: "deepseek".to_string(),
                model: cloud_model.to_string(),
            }
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

    /// Resolve current inference target (auto-detects local/cloud)
    ///
    /// Call before each inference; automatically checks local health and switches.
    pub async fn resolve_target(&self) -> InferenceTarget {
        let status = self.status.read().await;

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
                if status.fail_count >= self.max_failures_before_fallback {
                    drop(status);
                    self.switch_to_cloud().await;
                    InferenceTarget::Cloud {
                        provider: "deepseek".to_string(),
                        model: self.cloud_model.clone(),
                    }
                } else {
                    InferenceTarget::Local {
                        model: model.clone(),
                    }
                }
            }
            InferenceTarget::Cloud { .. } => {
                if self.check_local_health_quick().await {
                    drop(status);
                    self.switch_back_to_local().await;
                    let s = self.status.read().await;
                    match &s.target {
                        InferenceTarget::Local { model } => InferenceTarget::Local {
                            model: model.clone(),
                        },
                        _ => InferenceTarget::Cloud {
                            provider: "deepseek".to_string(),
                            model: self.cloud_model.clone(),
                        },
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

    /// Report inference failure (triggers fallback counter)
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
                status.fail_count,
                error
            );
        }
    }

    /// Report inference success (resets failure counter)
    pub async fn report_success(&self) {
        let mut status = self.status.write().await;
        status.fail_count = 0;
    }

    /// Switch to cloud
    async fn switch_to_cloud(&self) {
        let mut status = self.status.write().await;
        status.target = InferenceTarget::Cloud {
            provider: "deepseek".to_string(),
            model: self.cloud_model.clone(),
        };
        status.switched_at = Instant::now();
        status.cooldown_until =
            Some(Instant::now() + Duration::from_secs(self.cooldown_secs));
        tracing::info!(
            "[AutoFallback] Switched to cloud (Deepseek {})",
            self.cloud_model
        );
    }

    /// Switch back to local
    async fn switch_back_to_local(&self) {
        if let Some(model) = self.local_models.first() {
            let mut status = self.status.write().await;
            status.target = InferenceTarget::Local {
                model: model.clone(),
            };
            status.fail_count = 0;
            status.cooldown_until = None;
            status.switched_at = Instant::now();
            tracing::info!("[AutoFallback] Switched back to local model: {}", model);
        }
    }

    /// Quick health check — HTTP ping local llama.cpp process ports
    async fn check_local_health_quick(&self) -> bool {
        for port in 18000..18100u16 {
            let url = format!("http://127.0.0.1:{}/health", port);
            match reqwest::get(&url).await {
                Ok(resp) if resp.status().is_success() => return true,
                _ => continue,
            }
        }
        false
    }

    /// Get current fallback status (for logging/display)
    pub async fn status_summary(&self) -> String {
        let s = self.status.read().await;
        let target_str = match &s.target {
            InferenceTarget::Local { model } => format!("local/{}", model),
            InferenceTarget::Cloud { provider, model } => {
                format!("cloud/{}/{}", provider, model)
            }
        };
        let cooldown = s
            .cooldown_until
            .map(|c| {
                let remaining = c
                    .saturating_duration_since(Instant::now())
                    .as_secs();
                format!("{}s", remaining)
            })
            .unwrap_or_else(|| "none".to_string());

        format!(
            "Target: {} | Failures: {}/{} | Cooldown: {} | Uptime: {:?}",
            target_str,
            s.fail_count,
            self.max_failures_before_fallback,
            cooldown,
            Instant::now().saturating_duration_since(s.switched_at)
        )
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_initial_target_is_local() {
        let router = AutoFallbackRouter::new(
            vec!["qwen3-72b-int4".to_string()],
            "deepseek-chat",
        );
        let target = router.resolve_target().await;
        assert!(matches!(target, InferenceTarget::Local { .. }));
    }

    #[tokio::test]
    async fn test_fallback_to_cloud_after_failures() {
        let router = AutoFallbackRouter::new(
            vec!["qwen3-72b-int4".to_string()],
            "deepseek-chat",
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

    #[tokio::test]
    async fn test_report_success_resets_counter() {
        let router = AutoFallbackRouter::new(
            vec!["qwen3-72b-int4".to_string()],
            "deepseek-chat",
        );

        router.report_failure("err1").await;
        router.report_failure("err2").await;
        router.report_success().await;

        let status = router.status.read().await;
        assert_eq!(status.fail_count, 0);
    }

    #[tokio::test]
    async fn test_status_summary_format() {
        let router = AutoFallbackRouter::new(
            vec!["qwen3-72b-int4".to_string()],
            "deepseek-chat",
        );
        let summary = router.status_summary().await;
        assert!(summary.contains("Target:"));
        assert!(summary.contains("Failures:"));
        assert!(summary.contains("qwen3-72b-int4"));
    }

    #[tokio::test]
    async fn test_resolve_target_returns_cloneable_data() {
        let router = AutoFallbackRouter::new(
            vec!["model-a".to_string(), "model-b".to_string()],
            "cloud-model",
        );

        let target = router.resolve_target().await;
        match target {
            InferenceTarget::Local { model } => {
                assert_eq!(model, "model-a");
            }
            _ => panic!("Expected local target"),
        }
    }
}
