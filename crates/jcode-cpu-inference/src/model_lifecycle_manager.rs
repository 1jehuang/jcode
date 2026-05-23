//! Model Lifecycle Manager - Hot-swapping and graceful shutdown
//!
//! This module provides:
//! 1. Graceful shutdown with active request draining
//! 2. Blue-green deployment for zero-downtime model switching
//! 3. State snapshot/restore for fast recovery
//! 4. Request retry hints during transition

use crate::{CpuEngine, LlamaInstance, InstanceStatus};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};
use tracing::{info, warn, error, debug};

/// Model instance state for hot-swapping
#[derive(Debug, Clone)]
pub enum ModelState {
    /// Active and serving requests
    Active,
    /// Draining - not accepting new requests, waiting for active to complete
    Draining {
        started_at: Instant,
        active_requests: usize,
        timeout_secs: u64,
    },
    /// Standby - warmed up but not serving (for blue-green)
    Standby,
    /// Stopped
    Stopped,
}

/// Configuration for graceful shutdown
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GracefulShutdownConfig {
    /// Maximum time to wait for active requests to complete (seconds)
    pub drain_timeout_secs: u64,
    /// Interval to check if all requests completed (milliseconds)
    pub check_interval_ms: u64,
    /// Whether to save KV Cache snapshots
    pub save_cache_snapshot: bool,
    /// Path for cache snapshots
    pub snapshot_dir: PathBuf,
}

impl Default for GracefulShutdownConfig {
    fn default() -> Self {
        Self {
            drain_timeout_secs: 30,
            check_interval_ms: 500,
            save_cache_snapshot: true,
            snapshot_dir: PathBuf::from("/tmp/carpai/model_snapshots"),
        }
    }
}

/// Snapshot metadata for fast recovery
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSnapshot {
    pub model_name: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub kv_cache_size_bytes: u64,
    pub config_hash: String,
    pub snapshot_path: PathBuf,
}

/// Hot-swap coordinator for blue-green deployments
pub struct ModelLifecycleManager {
    engine: Arc<CpuEngine>,
    config: GracefulShutdownConfig,
    /// Track model states (model_name -> ModelState)
    model_states: Arc<RwLock<HashMap<String, ModelState>>>,
    /// Active request counters per model
    active_requests: Arc<RwLock<HashMap<String, usize>>>,
    /// Model snapshots for fast recovery
    snapshots: Arc<Mutex<Vec<ModelSnapshot>>>,
    /// Blue-green pairs (old_model -> new_model)
    blue_green_pairs: Arc<RwLock<HashMap<String, String>>>,
}

impl ModelLifecycleManager {
    pub fn new(engine: Arc<CpuEngine>, config: GracefulShutdownConfig) -> Self {
        info!(
            "[ModelLifecycleManager] Initialized with drain_timeout={}s",
            config.drain_timeout_secs
        );

        // Create snapshot directory if it doesn't exist
        if !config.snapshot_dir.exists() {
            if let Err(e) = std::fs::create_dir_all(&config.snapshot_dir) {
                warn!("Failed to create snapshot directory: {}", e);
            }
        }

        Self {
            engine,
            config,
            model_states: Arc::new(RwLock::new(HashMap::new())),
            active_requests: Arc::new(RwLock::new(HashMap::new())),
            snapshots: Arc::new(Mutex::new(Vec::new())),
            blue_green_pairs: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Start a model and mark it as active
    pub async fn start_model(
        &self,
        model_name: &str,
        model_path: &PathBuf,
        ctx_size: u32,
        threads: u32,
    ) -> Result<LlamaInstance> {
        info!("[ModelLifecycleManager] Starting model: {}", model_name);

        let instance = self.engine
            .start(model_name, model_path, ctx_size, threads)
            .await
            .context("Failed to start model")?;

        // Track state
        let mut states = self.model_states.write().await;
        states.insert(model_name.to_string(), ModelState::Active);

        let mut req_counters = self.active_requests.write().await;
        req_counters.insert(model_name.to_string(), 0);

        Ok(instance)
    }

    /// Gracefully stop a model (drain active requests first)
    pub async fn graceful_stop(&self, model_name: &str) -> Result<()> {
        info!("[ModelLifecycleManager] Initiating graceful stop for: {}", model_name);

        // 1. Mark as draining
        {
            let mut states = self.model_states.write().await;
            let active_count = {
                let counters = self.active_requests.read().await;
                counters.get(model_name).copied().unwrap_or(0)
            };

            states.insert(
                model_name.to_string(),
                ModelState::Draining {
                    started_at: Instant::now(),
                    active_requests: active_count,
                    timeout_secs: self.config.drain_timeout_secs,
                },
            );
        }

        info!(
            "[ModelLifecycleManager] Model {} is now draining ({} active requests)",
            model_name,
            {
                let counters = self.active_requests.read().await;
                counters.get(model_name).copied().unwrap_or(0)
            }
        );

        // 2. Wait for active requests to complete or timeout
        self.wait_for_drain(model_name).await?;

        // 3. Save KV Cache snapshot if configured
        if self.config.save_cache_snapshot {
            if let Err(e) = self.save_cache_snapshot(model_name).await {
                warn!(
                    "[ModelLifecycleManager] Failed to save cache snapshot for {}: {}",
                    model_name, e
                );
            }
        }

        // 4. Stop the model
        self.engine.stop(model_name).await?;

        // 5. Update state
        {
            let mut states = self.model_states.write().await;
            states.insert(model_name.to_string(), ModelState::Stopped);
        }

        info!("[ModelLifecycleManager] Model {} gracefully stopped", model_name);
        Ok(())
    }

    /// Hot-swap: Replace old model with new model without downtime
    pub async fn hot_swap(
        &self,
        old_model: &str,
        new_model: &str,
        new_model_path: &PathBuf,
        ctx_size: u32,
        threads: u32,
    ) -> Result<()> {
        info!(
            "[ModelLifecycleManager] Hot-swapping {} -> {}",
            old_model, new_model
        );

        // 1. Start new model in standby mode
        info!("[ModelLifecycleManager] Warming up new model: {}", new_model);
        let new_instance = self.start_model(new_model, new_model_path, ctx_size, threads).await?;

        // Wait for new model to be ready
        loop {
            if let Some(inst) = self.engine.get_ready_instance(new_model).await {
                if inst.status == InstanceStatus::Ready {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        info!("[ModelLifecycleManager] New model {} is ready", new_model);

        // 2. Record blue-green pair
        {
            let mut pairs = self.blue_green_pairs.write().await;
            pairs.insert(old_model.to_string(), new_model.to_string());
        }

        // 3. Mark old model as draining (stop accepting new requests)
        {
            let mut states = self.model_states.write().await;
            let active_count = {
                let counters = self.active_requests.read().await;
                counters.get(old_model).copied().unwrap_or(0)
            };

            states.insert(
                old_model.to_string(),
                ModelState::Draining {
                    started_at: Instant::now(),
                    active_requests: active_count,
                    timeout_secs: self.config.drain_timeout_secs,
                },
            );
        }

        // 4. Wait for old model to drain
        self.wait_for_drain(old_model).await?;

        // 5. Save old model's cache snapshot
        if self.config.save_cache_snapshot {
            if let Err(e) = self.save_cache_snapshot(old_model).await {
                warn!(
                    "[ModelLifecycleManager] Failed to save cache snapshot for {}: {}",
                    old_model, e
                );
            }
        }

        // 6. Stop old model
        self.engine.stop(old_model).await?;

        // 7. Promote new model to active
        {
            let mut states = self.model_states.write().await;
            states.insert(new_model.to_string(), ModelState::Active);
        }

        // 8. Clean up blue-green pair
        {
            let mut pairs = self.blue_green_pairs.write().await;
            pairs.remove(old_model);
        }

        info!(
            "[ModelLifecycleManager] Hot-swap complete: {} -> {}",
            old_model, new_model
        );

        Ok(())
    }

    /// Check if a model can accept new requests
    pub async fn can_accept_requests(&self, model_name: &str) -> bool {
        let states = self.model_states.read().await;
        matches!(states.get(model_name), Some(ModelState::Active))
    }

    /// Get retry hint for clients when model is unavailable
    pub async fn get_retry_hint(&self, model_name: &str) -> Option<RetryHint> {
        let states = self.model_states.read().await;

        match states.get(model_name) {
            Some(ModelState::Draining { timeout_secs, .. }) => {
                // Check if there's a replacement model
                let pairs = self.blue_green_pairs.read().await;
                let replacement = pairs.get(model_name).cloned();

                Some(RetryHint {
                    should_retry: true,
                    retry_after_ms: 1000,
                    alternative_model: replacement,
                    reason: format!("Model {} is draining", model_name),
                })
            }
            Some(ModelState::Standby) => {
                Some(RetryHint {
                    should_retry: false,
                    retry_after_ms: 0,
                    alternative_model: None,
                    reason: format!("Model {} is in standby", model_name),
                })
            }
            Some(ModelState::Stopped) => {
                Some(RetryHint {
                    should_retry: false,
                    retry_after_ms: 0,
                    alternative_model: None,
                    reason: format!("Model {} is stopped", model_name),
                })
            }
            _ => None, // Active or unknown - allow request
        }
    }

    /// Increment active request counter
    pub async fn increment_active_requests(&self, model_name: &str) {
        let mut counters = self.active_requests.write().await;
        let count = counters.entry(model_name.to_string()).or_insert(0);
        *count += 1;
        debug!(
            "[ModelLifecycleManager] Model {} active requests: {}",
            model_name, *count
        );
    }

    /// Decrement active request counter
    pub async fn decrement_active_requests(&self, model_name: &str) {
        let mut counters = self.active_requests.write().await;
        if let Some(count) = counters.get_mut(model_name) {
            if *count > 0 {
                *count -= 1;
            }
            debug!(
                "[ModelLifecycleManager] Model {} active requests: {}",
                model_name, *count
            );
        }
    }

    /// Get current model state
    pub async fn get_model_state(&self, model_name: &str) -> Option<ModelState> {
        let states = self.model_states.read().await;
        states.get(model_name).cloned()
    }

    /// List all models and their states
    pub async fn list_models(&self) -> HashMap<String, String> {
        let states = self.model_states.read().await;
        states
            .iter()
            .map(|(name, state)| {
                let state_str = match state {
                    ModelState::Active => "active".to_string(),
                    ModelState::Draining { active_requests, .. } => {
                        format!("draining ({} active)", active_requests)
                    }
                    ModelState::Standby => "standby".to_string(),
                    ModelState::Stopped => "stopped".to_string(),
                };
                (name.clone(), state_str)
            })
            .collect()
    }

    // ========================================================================
    // Private methods
    // ========================================================================

    /// Wait for model to drain active requests
    async fn wait_for_drain(&self, model_name: &str) -> Result<()> {
        let start = Instant::now();
        let timeout = Duration::from_secs(self.config.drain_timeout_secs);
        let check_interval = Duration::from_millis(self.config.check_interval_ms);

        loop {
            let active_count = {
                let counters = self.active_requests.read().await;
                counters.get(model_name).copied().unwrap_or(0)
            };

            if active_count == 0 {
                info!(
                    "[ModelLifecycleManager] Model {} drained successfully ({:.1}s)",
                    model_name,
                    start.elapsed().as_secs_f64()
                );
                return Ok(());
            }

            if start.elapsed() >= timeout {
                warn!(
                    "[ModelLifecycleManager] Model {} drain timeout after {}s ({} requests still active)",
                    model_name,
                    self.config.drain_timeout_secs,
                    active_count
                );
                return Err(anyhow::anyhow!(
                    "Drain timeout: {} active requests after {}s",
                    active_count,
                    self.config.drain_timeout_secs
                ));
            }

            debug!(
                "[ModelLifecycleManager] Waiting for {} to drain: {} active requests",
                model_name, active_count
            );

            tokio::time::sleep(check_interval).await;
        }
    }

    /// Save KV Cache snapshot for fast recovery
    async fn save_cache_snapshot(&self, model_name: &str) -> Result<()> {
        let snapshot_path = self.config.snapshot_dir.join(format!(
            "{}_{}.bin",
            model_name,
            chrono::Utc::now().timestamp()
        ));

        // In production, this would serialize the actual KV Cache from GPU/CPU memory
        // For now, we create a metadata-only snapshot
        let snapshot = ModelSnapshot {
            model_name: model_name.to_string(),
            timestamp: chrono::Utc::now(),
            kv_cache_size_bytes: 0, // Would be actual cache size
            config_hash: "placeholder".to_string(), // Would hash model config
            snapshot_path: snapshot_path.clone(),
        };

        // Save metadata
        let metadata_path = snapshot_path.with_extension("json");
        let metadata_json = serde_json::to_string_pretty(&snapshot)?;
        tokio::fs::write(&metadata_path, metadata_json).await
            .context("Failed to write snapshot metadata")?;

        // Create empty snapshot file (placeholder for actual cache data)
        tokio::fs::write(&snapshot_path, vec![]).await
            .context("Failed to write snapshot file")?;

        // Track snapshot
        {
            let mut snapshots = self.snapshots.lock().await;
            snapshots.push(snapshot);
        }

        info!(
            "[ModelLifecycleManager] Saved snapshot for {}: {:?}",
            model_name, metadata_path
        );

        Ok(())
    }

    /// Restore from snapshot (for fast warmup)
    pub async fn restore_from_snapshot(
        &self,
        model_name: &str,
        snapshot_timestamp: i64,
    ) -> Result<Option<PathBuf>> {
        let snapshots = self.snapshots.lock().await;

        let snapshot = snapshots
            .iter()
            .find(|s| s.model_name == model_name && s.timestamp.timestamp() == snapshot_timestamp)
            .ok_or_else(|| anyhow::anyhow!("Snapshot not found"))?;

        info!(
            "[ModelLifecycleManager] Restoring {} from snapshot: {:?}",
            model_name, snapshot.snapshot_path
        );

        // In production, this would load KV Cache into memory
        // For now, just return the path
        Ok(Some(snapshot.snapshot_path.clone()))
    }

    /// Get available snapshots for a model
    pub async fn list_snapshots(&self, model_name: &str) -> Vec<ModelSnapshot> {
        let snapshots = self.snapshots.lock().await;
        snapshots
            .iter()
            .filter(|s| s.model_name == model_name)
            .cloned()
            .collect()
    }
}

/// Retry hint for clients during model transitions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryHint {
    /// Whether client should retry
    pub should_retry: bool,
    /// How long to wait before retrying (milliseconds)
    pub retry_after_ms: u64,
    /// Alternative model to use (if available)
    pub alternative_model: Option<String>,
    /// Human-readable reason
    pub reason: String,
}

/// Builder for ModelLifecycleManager
pub struct ModelLifecycleManagerBuilder {
    engine: Arc<CpuEngine>,
    config: GracefulShutdownConfig,
}

impl ModelLifecycleManagerBuilder {
    pub fn new(engine: Arc<CpuEngine>) -> Self {
        Self {
            engine,
            config: GracefulShutdownConfig::default(),
        }
    }

    pub fn drain_timeout(mut self, secs: u64) -> Self {
        self.config.drain_timeout_secs = secs;
        self
    }

    pub fn check_interval(mut self, ms: u64) -> Self {
        self.config.check_interval_ms = ms;
        self
    }

    pub fn save_cache_snapshot(mut self, save: bool) -> Self {
        self.config.save_cache_snapshot = save;
        self
    }

    pub fn snapshot_dir(mut self, dir: PathBuf) -> Self {
        self.config.snapshot_dir = dir;
        self
    }

    pub fn build(self) -> ModelLifecycleManager {
        ModelLifecycleManager::new(self.engine, self.config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_graceful_shutdown_config_default() {
        let config = GracefulShutdownConfig::default();
        assert_eq!(config.drain_timeout_secs, 30);
        assert_eq!(config.check_interval_ms, 500);
        assert!(config.save_cache_snapshot);
    }

    #[tokio::test]
    async fn test_retry_hint_generation() {
        let engine = Arc::new(CpuEngine::new());
        let manager = ModelLifecycleManager::new(engine, GracefulShutdownConfig::default());

        // Test with non-existent model (should return None)
        let hint = manager.get_retry_hint("nonexistent").await;
        assert!(hint.is_none());
    }

    #[tokio::test]
    async fn test_list_models_empty() {
        let engine = Arc::new(CpuEngine::new());
        let manager = ModelLifecycleManager::new(engine, GracefulShutdownConfig::default());

        let models = manager.list_models().await;
        assert!(models.is_empty());
    }
}
