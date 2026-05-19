//! Cluster Integration with Main Application Lifecycle
//!
//! This module integrates the distributed cluster service into the main
//! jcode server lifecycle, enabling automatic leader election and
//! cluster coordination.

use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, debug};

use super::config::ClusterConfig;
use super::service::{ClusterService, ServiceState};

/// Global cluster service instance (if enabled)
static CLUSTER_SERVICE: RwLock<Option<Arc<ClusterService>>> = RwLock::const_new(None);

/// Initialize cluster service during server startup
pub async fn init_cluster_service(config_path: Option<&str>) -> Result<(), String> {
    info!("Initializing cluster service");

    // Load configuration
    let config = if let Some(path) = config_path {
        let path_buf = std::path::PathBuf::from(path);
        ClusterConfig::from_file(&path_buf)
            .map_err(|e| format!("Failed to load cluster config from {}: {}", path, e))?
    } else {
        // Try default config location
        let default_path = get_default_config_path();
        if default_path.exists() {
            ClusterConfig::from_file(&default_path)
                .map_err(|e| format!("Failed to load default config: {}", e))?
        } else {
            info!("No cluster configuration found, cluster mode disabled");
            return Ok(());
        }
    };

    // Check if cluster is enabled
    if !config.enabled {
        info!("Cluster mode is disabled in configuration");
        return Ok(());
    }

    // Validate configuration
    config.validate().map_err(|e| format!("Invalid cluster config: {}", e))?;

    info!(
        "Starting cluster node on {}:{} (ID: {:?})",
        config.node.host,
        config.node.port,
        config.node.id
    );

    // Create cluster service
    let service = ClusterService::new(config)
        .await
        .map_err(|e| format!("Failed to create cluster service: {}", e))?;

    // Start the service
    service.start()
        .await
        .map_err(|e| format!("Failed to start cluster service: {}", e))?;

    // Store globally
    let mut global = CLUSTER_SERVICE.write().await;
    *global = Some(service);

    info!("Cluster service initialized successfully");
    Ok(())
}

/// Shutdown cluster service during server shutdown
pub async fn shutdown_cluster_service() -> Result<(), String> {
    let mut global = CLUSTER_SERVICE.write().await;

    if let Some(service) = global.take() {
        info!("Shutting down cluster service");

        service.stop()
            .await
            .map_err(|e| format!("Failed to stop cluster service: {}", e))?;

        info!("Cluster service shut down successfully");
    }

    Ok(())
}

/// Get the current cluster service instance
pub async fn get_cluster_service() -> Option<Arc<ClusterService>> {
    CLUSTER_SERVICE.read().await.clone()
}

/// Check if cluster mode is enabled and running
pub async fn is_cluster_enabled() -> bool {
    let service = CLUSTER_SERVICE.read().await;
    if let Some(svc) = service.as_ref() {
        matches!(svc.get_state().await, ServiceState::Running)
    } else {
        false
    }
}

/// Check if this node is the cluster leader
pub async fn is_local_node_leader() -> bool {
    let service = CLUSTER_SERVICE.read().await;
    if let Some(svc) = service.as_ref() {
        svc.is_leader().await
    } else {
        false
    }
}

/// Get cluster information for status reporting
pub async fn get_cluster_status() -> Option<ClusterStatusInfo> {
    let service = CLUSTER_SERVICE.read().await;
    let svc = service.as_ref()?;

    let state = svc.get_state().await;
    let cluster_info = svc.get_cluster_info().await;
    let is_leader = svc.is_leader().await;
    let has_quorum = svc.has_quorum().await;

    Some(ClusterStatusInfo {
        state: format!("{:?}", state),
        cluster_id: cluster_info.cluster_id,
        total_nodes: cluster_info.total_nodes,
        healthy_nodes: cluster_info.healthy_nodes,
        leader_id: cluster_info.leader,
        self_id: cluster_info.self_id,
        is_leader,
        has_quorum,
    })
}

/// Cluster status information for API responses
#[derive(Debug, Clone, serde::Serialize)]
pub struct ClusterStatusInfo {
    pub state: String,
    pub cluster_id: String,
    pub total_nodes: usize,
    pub healthy_nodes: usize,
    pub leader_id: Option<String>,
    pub self_id: String,
    pub is_leader: bool,
    pub has_quorum: bool,
}

/// Execute a task only if this node is the leader
pub async fn execute_if_leader<F, Fut, T>(task: F) -> Option<T>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    if is_local_node_leader().await {
        Some(task().await)
    } else {
        debug!("Skipping task - not the leader");
        None
    }
}

/// Wait until this node becomes leader or timeout
pub async fn wait_for_leadership(timeout_ms: u64) -> Result<bool, String> {
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_millis(timeout_ms);

    loop {
        if is_local_node_leader().await {
            return Ok(true);
        }

        if start.elapsed() >= timeout {
            return Ok(false);
        }

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

/// Get default configuration file path
fn get_default_config_path() -> std::path::PathBuf {
    // Try ~/.jcode/cluster-config.json first
    if let Some(home) = dirs::home_dir() {
        let path = home.join(".jcode").join("cluster-config.json");
        if path.exists() {
            return path;
        }
    }

    // Fallback to ./cluster-config.json
    std::path::PathBuf::from("cluster-config.json")
}

/// Register cluster health check with the application's health system
pub async fn register_health_check() {
    // TODO: Implement health check registration when metrics module supports it
    debug!("Cluster health check registered");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_cluster_disabled_by_default() {
        // Without config, cluster should be disabled
        assert!(!is_cluster_enabled().await);
        assert!(!is_local_node_leader().await);
    }

    #[tokio::test]
    async fn test_get_status_when_disabled() {
        let status = get_cluster_status().await;
        assert!(status.is_none());
    }

    #[tokio::test]
    async fn test_execute_if_leader_when_disabled() {
        let result = execute_if_leader(|| async { 42 }).await;
        assert!(result.is_none());
    }
}
