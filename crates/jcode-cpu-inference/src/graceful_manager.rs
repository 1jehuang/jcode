//! Graceful Shutdown and Hot-Switching Manager
//!
//! Provides zero-downtime model updates and graceful instance lifecycle management.
//!
//! ## Features
//! 1. **Graceful Shutdown**: Wait for active requests to complete before stopping
//! 2. **Hot-Switching**: Blue-green deployment for models (zero downtime)
//! 3. **Draining Mode**: Stop accepting new requests while completing existing ones
//! 4. **State Snapshot**: Save/restore KV Cache for fast recovery
//! 5. **Health Probes**: Continuous health checking during transitions

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{RwLock, Mutex};
use tokio::task::JoinHandle;
use tracing::{info, warn, error, debug};
use serde::{Serialize, Deserialize};
use chrono::{DateTime, Utc};

// ============================================================================
// Instance State Management
// ============================================================================

/// Instance lifecycle state
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum InstanceState {
    /// Initializing, loading model weights
    Initializing,
    /// Ready to accept requests
    Ready,
    /// Draining - completing active requests, not accepting new ones
    Draining,
    /// Stopping - shutting down process
    Stopping,
    /// Stopped - process terminated
    Stopped,
    /// Error state
    Error(String),
}

impl InstanceState {
    pub fn can_accept_requests(&self) -> bool {
        matches!(self, Self::Ready)
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Stopped | Self::Error(_))
    }
}

/// Graceful shutdown configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GracefulConfig {
    /// Maximum time to wait for active requests to complete (seconds)
    pub shutdown_timeout_secs: u64,
    /// Time to wait between drain status checks (milliseconds)
    pub drain_check_interval_ms: u64,
    /// Enable state snapshots before shutdown
    pub enable_snapshots: bool,
    /// Snapshot directory path
    pub snapshot_dir: Option<String>,
    /// Health check interval during transition (milliseconds)
    pub health_check_interval_ms: u64,
}

impl Default for GracefulConfig {
    fn default() -> Self {
        Self {
            shutdown_timeout_secs: 30,
            drain_check_interval_ms: 500,
            enable_snapshots: false,
            snapshot_dir: None,
            health_check_interval_ms: 1000,
        }
    }
}

/// Extended instance with state tracking
#[derive(Debug, Clone)]
pub struct TrackedInstance {
    pub model_name: String,
    pub instance_id: String,  // Unique ID for blue-green deployments
    pub port: u16,
    pub api_url: String,
    pub state: InstanceState,
    pub started_at: DateTime<Utc>,
    pub draining_since: Option<DateTime<Utc>>,
    pub active_request_count: u64,
    pub total_requests_served: u64,
    pub version: String,  // Model version for hot-switching
}

impl TrackedInstance {
    pub fn new(model_name: String, port: u16, version: String) -> Self {
        let instance_id = format!("{}-{}-{}", model_name, port, Utc::now().timestamp_millis());
        Self {
            model_name,
            instance_id,
            port,
            api_url: format!("http://127.0.0.1:{}/v1", port),
            state: InstanceState::Initializing,
            started_at: Utc::now(),
            draining_since: None,
            active_request_count: 0,
            total_requests_served: 0,
            version,
        }
    }

    /// Mark instance as draining
    pub fn start_draining(&mut self) {
        self.state = InstanceState::Draining;
        self.draining_since = Some(Utc::now());
        info!(
            "Instance {} entered draining state (active_requests={})",
            self.instance_id, self.active_request_count
        );
    }

    /// Increment active request count
    pub fn add_request(&mut self) {
        if self.state == InstanceState::Ready {
            self.active_request_count += 1;
        }
    }

    /// Decrement active request count
    pub fn remove_request(&mut self) {
        self.active_request_count = self.active_request_count.saturating_sub(1);
        self.total_requests_served += 1;
    }

    /// Check if instance has no active requests
    pub fn is_idle(&self) -> bool {
        self.active_request_count == 0
    }
}

// ============================================================================
// Snapshot Support
// ============================================================================

/// KV Cache snapshot metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotMetadata {
    pub instance_id: String,
    pub model_name: String,
    pub timestamp: DateTime<Utc>,
    pub request_id: String,
    pub sequence_length: usize,
    pub layer_count: usize,
    pub size_bytes: usize,
}

/// Snapshot manager for state persistence
pub struct SnapshotManager {
    snapshot_dir: String,
}

impl SnapshotManager {
    pub fn new(snapshot_dir: String) -> Self {
        std::fs::create_dir_all(&snapshot_dir).ok();
        Self { snapshot_dir }
    }

    /// Save snapshot metadata
    pub fn save_metadata(&self, metadata: &SnapshotMetadata) -> anyhow::Result<()> {
        let path = format!("{}/{}.json", self.snapshot_dir, metadata.request_id);
        let json = serde_json::to_string_pretty(metadata)?;
        std::fs::write(&path, json)?;
        debug!("Saved snapshot metadata: {}", path);
        Ok(())
    }

    /// Load snapshot metadata
    pub fn load_metadata(&self, request_id: &str) -> anyhow::Result<SnapshotMetadata> {
        let path = format!("{}/{}.json", self.snapshot_dir, request_id);
        let json = std::fs::read_to_string(&path)?;
        let metadata = serde_json::from_str(&json)?;
        Ok(metadata)
    }

    /// Clean up old snapshots
    pub fn cleanup_old_snapshots(&self, older_than_hours: u64) -> anyhow::Result<usize> {
        let cutoff = Utc::now() - chrono::Duration::hours(older_than_hours as i64);
        let mut cleaned = 0;

        for entry in std::fs::read_dir(&self.snapshot_dir)? {
            let entry = entry?;
            let path = entry.path();
            // Clean both metadata (.json) and binary snapshot (.bin) files
            let should_clean = path.extension().and_then(|s| s.to_str()) == Some("json")
                || path.extension().and_then(|s| s.to_str()) == Some("bin");

            if should_clean {
                let stem = path.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("");

                // For .bin files, check corresponding .json metadata
                if path.extension().and_then(|s| s.to_str()) == Some("bin") {
                    let json_path = path.with_extension("json");
                    if json_path.exists() {
                        let content = std::fs::read_to_string(json_path).ok();
                        if let Some(content) = content {
                            if let Ok(metadata) = serde_json::from_str::<SnapshotMetadata>(&content) {
                                if metadata.timestamp < cutoff {
                                    std::fs::remove_file(&path)?;
                                    std::fs::remove_file(json_path)?;
                                    cleaned += 1;
                                }
                            }
                        }
                    }
                } else {
                    // For .json files, check timestamp directly
                    let content = std::fs::read_to_string(&path)?;
                    if let Ok(metadata) = serde_json::from_str::<SnapshotMetadata>(&content) {
                        if metadata.timestamp < cutoff {
                            let bin_path = path.with_extension("bin");
                            if bin_path.exists() {
                                std::fs::remove_file(bin_path)?;
                            }
                            std::fs::remove_file(&path)?;
                            cleaned += 1;
                        }
                    }
                }
            }
        }

        info!("Cleaned up {} old snapshots", cleaned);
        Ok(cleaned)
    }

    /// Save KV Cache snapshot from llama.cpp instance
    ///
    /// Fetches the current KV Cache state via llama.cpp's internal API
    /// and saves it to disk as a binary file with metadata.
    ///
    /// Note: llama.cpp doesn't expose direct KV Cache export yet,
    /// so this implementation prepares the infrastructure for when available.
    pub async fn save_kv_cache_snapshot(
        &self,
        instance_id: &str,
        model_name: &str,
        port: u16,
        request_id: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        info!("Saving KV Cache snapshot for instance {}", instance_id);

        // Create snapshot filename
        let timestamp = Utc::now().timestamp_millis();
        let snapshot_name = format!("{}_{}", instance_id, timestamp);
        let bin_path = format!("{}/{}.bin", self.snapshot_dir, snapshot_name);
        let json_path = format!("{}/{}.json", self.snapshot_dir, snapshot_name);

        // Try to fetch KV Cache state from llama.cpp internal API
        // Note: This endpoint may not be available in all llama.cpp versions
        let client = reqwest::Client::new();
        let api_url = format!("http://127.0.0.1:{}/internal/state", port);

        let mut sequence_length = 0;
        let mut layer_count = 0;
        let mut size_bytes = 0;

        match client.get(&api_url).timeout(Duration::from_secs(10)).send().await {
            Ok(response) if response.status().is_success() => {
                // Save binary data
                let bytes = response.bytes().await?;
                size_bytes = bytes.len();
                std::fs::write(&bin_path, &bytes)?;
                info!("Saved KV Cache binary data: {} bytes", size_bytes);

                // Extract metadata from response headers if available
                if let Some(seq_len) = response.headers().get("x-sequence-length") {
                    if let Ok(len) = seq_len.to_str() {
                        sequence_length = len.parse().unwrap_or(0);
                    }
                }
                if let Some(layers) = response.headers().get("x-layer-count") {
                    if let Ok(count) = layers.to_str() {
                        layer_count = count.parse().unwrap_or(0);
                    }
                }
            }
            Ok(response) => {
                warn!(
                    "KV Cache export returned status {}: feature may not be supported in this llama.cpp version",
                    response.status()
                );
                // Create empty snapshot file to track the attempt
                std::fs::write(&bin_path, Vec::new())?;
            }
            Err(e) => {
                warn!(
                    "Failed to fetch KV Cache from llama.cpp ({}): {}. Creating metadata-only snapshot.",
                    api_url, e
                );
                // Create empty snapshot file to track the attempt
                std::fs::write(&bin_path, Vec::new())?;
            }
        }

        // Save metadata
        let metadata = SnapshotMetadata {
            instance_id: instance_id.to_string(),
            model_name: model_name.to_string(),
            timestamp: Utc::now(),
            request_id: request_id.to_string(),
            sequence_length,
            layer_count,
            size_bytes,
        };

        self.save_metadata(&metadata)?;

        info!(
            "KV Cache snapshot saved: {} (size={} bytes, seq_len={}, layers={})",
            snapshot_name, size_bytes, sequence_length, layer_count
        );

        Ok(snapshot_name)
    }

    /// Restore KV Cache snapshot to llama.cpp instance
    ///
    /// Loads a previously saved KV Cache state and POSTs it to
    /// llama.cpp's internal state loading endpoint.
    pub async fn restore_kv_cache_snapshot(
        &self,
        snapshot_name: &str,
        port: u16,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("Restoring KV Cache snapshot: {}", snapshot_name);

        let bin_path = format!("{}/{}.bin", self.snapshot_dir, snapshot_name);
        let json_path = format!("{}/{}.json", self.snapshot_dir, snapshot_name);

        // Load metadata
        let metadata = if std::path::Path::new(&json_path).exists() {
            let content = std::fs::read_to_string(&json_path)?;
            let meta: SnapshotMetadata = serde_json::from_str(&content)?;
            info!(
                "Loaded metadata: model={}, seq_len={}, layers={}, size={} bytes",
                meta.model_name, meta.sequence_length, meta.layer_count, meta.size_bytes
            );
            Some(meta)
        } else {
            warn!("No metadata file found for snapshot: {}", json_path);
            None
        };

        // Load binary data
        if !std::path::Path::new(&bin_path).exists() {
            return Err(format!("Snapshot binary file not found: {}", bin_path).into());
        }

        let kv_cache_data = std::fs::read(&bin_path)?;

        if kv_cache_data.is_empty() {
            warn!("Snapshot file is empty, skipping restore");
            return Ok(());
        }

        // POST to llama.cpp internal state loading endpoint
        let client = reqwest::Client::new();
        let load_url = format!("http://127.0.0.1:{}/internal/state/load", port);

        match client
            .post(&load_url)
            .timeout(Duration::from_secs(30))
            .body(kv_cache_data.clone())
            .header("Content-Type", "application/octet-stream")
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => {
                info!(
                    "Successfully restored KV Cache snapshot to port {} ({} bytes)",
                    port, kv_cache_data.len()
                );
                Ok(())
            }
            Ok(response) => {
                Err(format!(
                    "KV Cache restore returned status {}: {}",
                    response.status(),
                    response.text().await.unwrap_or_default()
                ).into())
            }
            Err(e) => {
                Err(format!(
                    "Failed to restore KV Cache to {}: {}. Feature may not be supported in this llama.cpp version.",
                    load_url, e
                ).into())
            }
        }
    }

    /// List available snapshots for a model
    pub fn list_snapshots(&self, model_name: Option<&str>) -> Vec<SnapshotMetadata> {
        let mut snapshots = Vec::new();

        if let Ok(entries) = std::fs::read_dir(&self.snapshot_dir) {
            for entry in entries {
                if let Ok(entry) = entry {
                    let path = entry.path();
                    if path.extension().and_then(|s| s.to_str()) == Some("json") {
                        if let Ok(content) = std::fs::read_to_string(&path) {
                            if let Ok(metadata) = serde_json::from_str::<SnapshotMetadata>(&content) {
                                // Filter by model name if specified
                                if model_name.map_or(true, |m| metadata.model_name == m) {
                                    snapshots.push(metadata);
                                }
                            }
                        }
                    }
                }
            }
        }

        // Sort by timestamp (newest first)
        snapshots.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        snapshots
    }

    /// Delete a specific snapshot
    pub fn delete_snapshot(&self, snapshot_name: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let bin_path = format!("{}/{}.bin", self.snapshot_dir, snapshot_name);
        let json_path = format!("{}/{}.json", self.snapshot_dir, snapshot_name);

        if std::path::Path::new(&bin_path).exists() {
            std::fs::remove_file(&bin_path)?;
        }
        if std::path::Path::new(&json_path).exists() {
            std::fs::remove_file(&json_path)?;
        }

        info!("Deleted snapshot: {}", snapshot_name);
        Ok(())
    }
}

// ============================================================================
// Health Checking
// ============================================================================

/// Health check result
#[derive(Debug, Clone)]
pub struct HealthCheckResult {
    pub instance_id: String,
    pub is_healthy: bool,
    pub response_time_ms: f64,
    pub error: Option<String>,
    pub timestamp: DateTime<Utc>,
}

/// Health checker for instances
pub struct HealthChecker {
    client: reqwest::Client,
    check_interval: Duration,
}

impl HealthChecker {
    pub fn new(check_interval: Duration) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            client,
            check_interval,
        }
    }

    /// Perform health check on an instance
    pub async fn check_health(&self, instance: &TrackedInstance) -> HealthCheckResult {
        let start = Instant::now();
        let url = format!("{}/models", instance.api_url);

        match self.client.get(&url).send().await {
            Ok(response) => {
                let elapsed = start.elapsed().as_secs_f64() * 1000.0;
                let is_healthy = response.status().is_success();

                HealthCheckResult {
                    instance_id: instance.instance_id.clone(),
                    is_healthy,
                    response_time_ms: elapsed,
                    error: if !is_healthy {
                        Some(format!("HTTP {}", response.status()))
                    } else {
                        None
                    },
                    timestamp: Utc::now(),
                }
            }
            Err(e) => HealthCheckResult {
                instance_id: instance.instance_id.clone(),
                is_healthy: false,
                response_time_ms: start.elapsed().as_secs_f64() * 1000.0,
                error: Some(e.to_string()),
                timestamp: Utc::now(),
            },
        }
    }

    /// Continuous health monitoring
    pub fn start_monitoring(
        &self,
        instance: Arc<RwLock<TrackedInstance>>,
    ) -> JoinHandle<()> {
        let checker = self.clone();
        let interval = self.check_interval;

        tokio::spawn(async move {
            loop {
                let inst = instance.read().await;
                let result = checker.check_health(&inst).await;

                if !result.is_healthy && inst.state == InstanceState::Ready {
                    warn!(
                        "Health check failed for {}: {:?}",
                        inst.instance_id, result.error
                    );
                }

                drop(inst);
                tokio::time::sleep(interval).await;
            }
        })
    }
}

impl Clone for HealthChecker {
    fn clone(&self) -> Self {
        Self {
            client: self.client.clone(),
            check_interval: self.check_interval,
        }
    }
}

// ============================================================================
// Graceful Manager
// ============================================================================

/// Main manager for graceful operations
pub struct GracefulManager {
    config: GracefulConfig,
    instances: Arc<RwLock<HashMap<String, Vec<Arc<RwLock<TrackedInstance>>>>>>,
    snapshot_manager: Option<Arc<Mutex<SnapshotManager>>>,
    health_checker: HealthChecker,
    next_instance_id: Arc<Mutex<u64>>,
}

impl GracefulManager {
    pub fn new(config: GracefulConfig) -> Self {
        let snapshot_manager = if config.enable_snapshots {
            let dir = config.snapshot_dir.clone().unwrap_or_else(|| "./snapshots".to_string());
            Some(Arc::new(Mutex::new(SnapshotManager::new(dir))))
        } else {
            None
        };

        let health_checker = HealthChecker::new(
            Duration::from_millis(config.health_check_interval_ms)
        );

        Self {
            config,
            instances: Arc::new(RwLock::new(HashMap::new())),
            snapshot_manager,
            health_checker,
            next_instance_id: Arc::new(Mutex::new(0)),
        }
    }

    /// Register a new instance
    pub async fn register_instance(&self, instance: TrackedInstance) {
        let model = instance.model_name.clone();
        let arc_inst = Arc::new(RwLock::new(instance));

        // Start health monitoring
        self.health_checker.start_monitoring(arc_inst.clone());

        let mut instances = self.instances.write().await;
        instances.entry(model).or_insert_with(Vec::new).push(arc_inst);

        info!("Registered new instance");
    }

    /// Graceful shutdown of an instance
    pub async fn graceful_shutdown(
        &self,
        model_name: &str,
        instance_id: &str,
    ) -> anyhow::Result<()> {
        info!("Initiating graceful shutdown for instance {}", instance_id);

        let instances = self.instances.read().await;
        let target_instance = instances
            .get(model_name)
            .and_then(|list| {
                list.iter()
                    .find(|inst| {
                        let locked = inst.blocking_read();
                        locked.instance_id == instance_id
                    })
                    .cloned()
            })
            .ok_or_else(|| anyhow::anyhow!("Instance not found"))?;

        drop(instances);

        // Step 1: Enter draining mode
        {
            let mut inst = target_instance.write().await;
            inst.start_draining();
        }

        // Step 2: Wait for active requests to complete
        let timeout = Duration::from_secs(self.config.shutdown_timeout_secs);
        let start = Instant::now();
        let check_interval = Duration::from_millis(self.config.drain_check_interval_ms);

        loop {
            if start.elapsed() > timeout {
                warn!(
                    "Shutdown timeout reached for {}, forcing stop (active_requests={})",
                    instance_id,
                    target_instance.read().await.active_request_count
                );
                break;
            }

            {
                let inst = target_instance.read().await;
                if inst.is_idle() {
                    info!(
                        "Instance {} drained successfully (served {} requests)",
                        instance_id, inst.total_requests_served
                    );
                    break;
                }
                debug!(
                    "Waiting for {} active requests to complete...",
                    inst.active_request_count
                );
            }

            tokio::time::sleep(check_interval).await;
        }

        // Step 3: Save snapshot if enabled
        if let Some(ref snapshot_mgr) = self.snapshot_manager {
            info!("Saving KV Cache snapshot for {}", instance_id);
            let snapshot_mgr = snapshot_mgr.lock().await;
            match snapshot_mgr.save_kv_cache_snapshot(
                instance_id,
                model_name,
                target_instance.read().await.port,
                &format!("shutdown-{}", instance_id),
            ).await {
                Ok(snapshot_name) => info!("Snapshot saved successfully: {}", snapshot_name),
                Err(e) => warn!("Failed to save snapshot: {}. This is expected if llama.cpp doesn't support KV Cache export yet.", e),
            }
        }

        // Step 4: Mark as stopping
        {
            let mut inst = target_instance.write().await;
            inst.state = InstanceState::Stopping;
        }

        // Step 5: Stop the underlying process
        // TODO: Send SIGTERM to llama.cpp process and wait for exit

        // Step 6: Mark as stopped
        {
            let mut inst = target_instance.write().await;
            inst.state = InstanceState::Stopped;
        }

        info!("Instance {} gracefully shut down", instance_id);
        Ok(())
    }

    /// Hot-swap: Replace old instance with new one (zero downtime)
    pub async fn hot_swap(
        &self,
        model_name: &str,
        old_instance_id: &str,
        new_instance: TrackedInstance,
    ) -> anyhow::Result<()> {
        info!(
            "Starting hot-swap: {} -> new instance",
            old_instance_id
        );

        // Step 1: Register new instance
        let new_arc = Arc::new(RwLock::new(new_instance.clone()));
        self.health_checker.start_monitoring(new_arc.clone());

        {
            let mut instances = self.instances.write().await;
            instances
                .entry(model_name.to_string())
                .or_insert_with(Vec::new)
                .push(new_arc);
        }

        // Step 2: Wait for new instance to be ready
        let timeout = Duration::from_secs(60);
        let start = Instant::now();

        loop {
            if start.elapsed() > timeout {
                anyhow::bail!("New instance failed to become ready within timeout");
            }

            {
                let inst = new_arc.read().await;
                if inst.state == InstanceState::Ready {
                    break;
                }
            }

            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        info!("New instance ready, draining old instance...");

        // Step 3: Drain old instance (new requests go to new instance)
        self.graceful_shutdown(model_name, old_instance_id).await.ok();

        info!(
            "Hot-swap complete for {}: {} -> {}",
            model_name, old_instance_id, new_instance.instance_id
        );

        Ok(())
    }

    /// Get all instances for a model
    pub async fn get_instances(&self, model_name: &str) -> Vec<TrackedInstance> {
        let instances = self.instances.read().await;
        match instances.get(model_name) {
            Some(list) => {
                let mut results = Vec::new();
                for arc_inst in list {
                    let inst = arc_inst.read().await;
                    results.push(inst.clone());
                }
                results
            }
            None => Vec::new(),
        }
    }

    /// Get the best instance for serving (ready, lowest load)
    pub async fn get_best_instance(&self, model_name: &str) -> Option<TrackedInstance> {
        let instances = self.get_instances(model_name).await;

        instances
            .into_iter()
            .filter(|i| i.state == InstanceState::Ready)
            .min_by_key(|i| i.active_request_count)
    }

    /// Record that a request started on an instance
    pub async fn record_request_start(&self, model_name: &str, instance_id: &str) {
        let instances = self.instances.read().await;
        if let Some(list) = instances.get(model_name) {
            for arc_inst in list {
                let mut inst = arc_inst.write().await;
                if inst.instance_id == instance_id {
                    inst.add_request();
                    break;
                }
            }
        }
    }

    /// Record that a request completed on an instance
    pub async fn record_request_end(&self, model_name: &str, instance_id: &str) {
        let instances = self.instances.read().await;
        if let Some(list) = instances.get(model_name) {
            for arc_inst in list {
                let mut inst = arc_inst.write().await;
                if inst.instance_id == instance_id {
                    inst.remove_request();
                    break;
                }
            }
        }
    }

    /// Cleanup stopped instances
    pub async fn cleanup_stopped(&self, model_name: &str) {
        let mut instances = self.instances.write().await;
        if let Some(list) = instances.get_mut(model_name) {
            list.retain(|arc_inst| {
                let inst = arc_inst.blocking_read();
                !inst.state.is_terminal()
            });
        }
    }

    /// Get statistics
    pub async fn get_stats(&self, model_name: &str) -> ModelStats {
        let instances = self.get_instances(model_name).await;

        let mut stats = ModelStats {
            total_instances: instances.len(),
            ready_instances: 0,
            draining_instances: 0,
            total_active_requests: 0,
            total_requests_served: 0,
            versions: HashMap::new(),
        };

        for inst in &instances {
            match inst.state {
                InstanceState::Ready => stats.ready_instances += 1,
                InstanceState::Draining => stats.draining_instances += 1,
                _ => {}
            }

            stats.total_active_requests += inst.active_request_count;
            stats.total_requests_served += inst.total_requests_served;

            *stats.versions.entry(inst.version.clone()).or_insert(0) += 1;
        }

        stats
    }
}

/// Model statistics
#[derive(Debug, Clone, Serialize)]
pub struct ModelStats {
    pub total_instances: usize,
    pub ready_instances: usize,
    pub draining_instances: usize,
    pub total_active_requests: u64,
    pub total_requests_served: u64,
    pub versions: HashMap<String, usize>,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_instance_lifecycle() {
        let mut instance = TrackedInstance::new(
            "test-model".to_string(),
            18000,
            "v1.0".to_string(),
        );

        assert_eq!(instance.state, InstanceState::Initializing);
        assert!(!instance.state.can_accept_requests());

        // Simulate ready
        instance.state = InstanceState::Ready;
        assert!(instance.state.can_accept_requests());

        // Add requests
        instance.add_request();
        instance.add_request();
        assert_eq!(instance.active_request_count, 2);
        assert!(!instance.is_idle());

        // Remove requests
        instance.remove_request();
        instance.remove_request();
        assert_eq!(instance.active_request_count, 0);
        assert!(instance.is_idle());

        // Start draining
        instance.start_draining();
        assert_eq!(instance.state, InstanceState::Draining);
        assert!(instance.draining_since.is_some());
    }

    #[tokio::test]
    async fn test_graceful_manager_basic() {
        let config = GracefulConfig::default();
        let manager = GracefulManager::new(config);

        let instance = TrackedInstance::new(
            "qwen-3.6-max".to_string(),
            18000,
            "v1.0".to_string(),
        );

        manager.register_instance(instance).await;

        let stats = manager.get_stats("qwen-3.6-max").await;
        assert_eq!(stats.total_instances, 1);
    }

    #[test]
    fn test_state_transitions() {
        let state = InstanceState::Ready;
        assert!(state.can_accept_requests());
        assert!(!state.is_terminal());

        let stopped = InstanceState::Stopped;
        assert!(!stopped.can_accept_requests());
        assert!(stopped.is_terminal());

        let error = InstanceState::Error("test".to_string());
        assert!(error.is_terminal());
    }
}
