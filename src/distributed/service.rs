//! Cluster Service - Main entry point for distributed cluster functionality
//!
//! This module provides the high-level interface for cluster operations,
//! integrating election, node management, and state synchronization.

use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tracing::{info, warn, error, debug};

use super::config::ClusterConfig;
use super::node::{ClusterNode, NodeRole};
use super::cluster::ClusterManager;
use super::election::ElectionService;
use super::load_balancer::{LoadBalancer, LoadBalancingStrategy};
use super::fault_tolerance::{FaultToleranceManager, FaultToleranceConfig};

/// Main cluster service handle
pub struct ClusterService {
    /// Cluster configuration
    config: ClusterConfig,

    /// Cluster manager
    manager: Arc<RwLock<ClusterManager>>,

    /// Election service
    election: Arc<Mutex<ElectionService>>,

    /// Load balancer
    load_balancer: Arc<RwLock<LoadBalancer>>,

    /// Fault tolerance manager
    fault_tolerance: Arc<RwLock<FaultToleranceManager>>,

    /// Service state
    state: Arc<RwLock<ServiceState>>,

    /// Background tasks handle
    tasks: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
}

/// Service state
#[derive(Debug, Clone, PartialEq)]
pub enum ServiceState {
    /// Service not initialized
    Initialized,
    /// Service starting
    Starting,
    /// Service running
    Running,
    /// Service stopping
    Stopping,
    /// Service stopped
    Stopped,
    /// Service in error state
    Error(String),
}

impl ClusterService {
    /// Create a new cluster service
    pub async fn new(config: ClusterConfig) -> Result<Arc<Self>, String> {
        // Validate configuration
        config.validate()?;

        if !config.enabled {
            info!("Cluster mode is disabled");
            return Err("Cluster mode is disabled".to_string());
        }

        info!("Initializing cluster service");

        // Create self node
        let mut self_node = ClusterNode::new(&config.node.host, config.node.port);

        // Set node ID if provided
        if let Some(id) = &config.node.id {
            self_node.id = id.clone();
        }

        // Add metadata from config
        if let Some(region) = &config.node.metadata.region {
            self_node.metadata.custom.insert("region".to_string(), region.clone());
        }
        if let Some(zone) = &config.node.metadata.zone {
            self_node.metadata.custom.insert("zone".to_string(), zone.clone());
        }
        for (k, v) in &config.node.metadata.labels {
            self_node.metadata.custom.insert(k.clone(), v.clone());
        }

        // Create cluster manager
        let manager = ClusterManager::new(&self_node);

        // Create election service
        let election = ElectionService::new();

        // Create load balancer
        let load_balancer = LoadBalancer::new(LoadBalancingStrategy::RoundRobin);

        // Create fault tolerance manager
        let ft_config = FaultToleranceConfig::default();
        let cluster_id = self_node.id.clone();
        let fault_tolerance = FaultToleranceManager::new(ft_config, cluster_id);

        let service = Arc::new(Self {
            config,
            manager: Arc::new(RwLock::new(manager)),
            election: Arc::new(Mutex::new(election)),
            load_balancer: Arc::new(RwLock::new(load_balancer)),
            fault_tolerance: Arc::new(RwLock::new(fault_tolerance)),
            state: Arc::new(RwLock::new(ServiceState::Initialized)),
            tasks: Arc::new(Mutex::new(Vec::new())),
        });

        info!("Cluster service initialized");
        Ok(service)
    }

    /// Start the cluster service
    pub async fn start(self: &Arc<Self>) -> Result<(), String> {
        let mut state = self.state.write().await;
        if *state != ServiceState::Initialized {
            return Err(format!("Cannot start service in state: {:?}", *state));
        }

        *state = ServiceState::Starting;
        drop(state);

        info!("Starting cluster service");

        // Register peer nodes
        self.register_peers().await?;

        // Start background tasks
        self.start_background_tasks().await?;

        // Attempt to become leader if preferred
        if matches!(self.config.node.preferred_role, Some(super::config::NodeRolePreference::Leader)) {
            self.attempt_election().await?;
        }

        let mut state = self.state.write().await;
        *state = ServiceState::Running;

        info!("Cluster service started successfully");
        Ok(())
    }

    /// Stop the cluster service
    pub async fn stop(self: &Arc<Self>) -> Result<(), String> {
        let mut state = self.state.write().await;
        if *state != ServiceState::Running {
            return Err(format!("Cannot stop service in state: {:?}", *state));
        }

        *state = ServiceState::Stopping;
        drop(state);

        info!("Stopping cluster service");

        // Cancel background tasks
        let mut tasks = self.tasks.lock().await;
        for task in tasks.drain(..) {
            task.abort();
        }

        let mut state = self.state.write().await;
        *state = ServiceState::Stopped;

        info!("Cluster service stopped");
        Ok(())
    }

    /// Get current service state
    pub async fn get_state(&self) -> ServiceState {
        self.state.read().await.clone()
    }

    /// Check if this node is the leader
    pub async fn is_leader(&self) -> bool {
        let manager = self.manager.read().await;
        manager.is_leader()
    }

    /// Get current leader information
    pub async fn get_leader(&self) -> Option<ClusterNode> {
        let manager = self.manager.read().await;
        manager.get_leader()
    }

    /// Get cluster information
    pub async fn get_cluster_info(&self) -> super::cluster::ClusterInfo {
        let manager = self.manager.read().await;
        manager.get_cluster_info()
    }

    /// Get healthy node count
    pub async fn healthy_node_count(&self) -> usize {
        let manager = self.manager.read().await;
        manager.healthy_count()
    }

    /// Check if we have quorum
    pub async fn has_quorum(&self) -> bool {
        let manager = self.manager.read().await;
        manager.has_quorum()
    }

    /// Select a node using load balancer
    pub async fn select_node(&self) -> Option<ClusterNode> {
        let manager = self.manager.read().await;
        let lb = self.load_balancer.read().await;

        let healthy_nodes = manager.healthy_nodes();
        let nodes_ref: Vec<&ClusterNode> = healthy_nodes.iter().collect();
        lb.select_node(&nodes_ref).cloned()
    }

    /// Get fault tolerance health summary
    pub async fn get_health_summary(&self) -> super::fault_tolerance::HealthSummary {
        self.fault_tolerance.read().await.get_health_summary()
    }

    /// Get node health state
    pub async fn get_node_health_state(&self, node_id: &str) -> Option<super::fault_tolerance::NodeHealthState> {
        self.fault_tolerance.read().await.get_node_state(node_id)
    }

    /// Register a node for fault tolerance tracking
    pub async fn register_for_fault_tracking(&self, node_id: &str) {
        self.fault_tolerance.write().await.register_node(node_id);
    }

    /// Get alert statistics
    pub async fn get_alert_stats(&self) -> (u64, u64) {
        self.fault_tolerance.read().await.get_alert_stats()
    }

    /// Register peer nodes from configuration
    async fn register_peers(&self) -> Result<(), String> {
        for peer_config in &self.config.peers {
            let addr = peer_config.address.parse::<std::net::SocketAddr>()
                .map_err(|e| format!("Invalid peer address {}: {}", peer_config.address, e))?;

            let peer = ClusterNode::new(addr.ip().to_string().as_str(), addr.port());
            let manager = self.manager.write().await;
            manager.register_node(peer)?;

            // Also register for fault tolerance tracking
            drop(manager);
            self.fault_tolerance.write().await.register_node(&peer.id);

            info!("Registered peer node: {}", peer_config.address);
        }

        Ok(())
    }

    /// Start background maintenance tasks
    async fn start_background_tasks(self: &Arc<Self>) -> Result<(), String> {
        let mut tasks = self.tasks.lock().await;

        // Heartbeat task
        let service_clone = Arc::clone(self);
        let heartbeat_task = tokio::spawn(async move {
            service_clone.heartbeat_loop().await;
        });
        tasks.push(heartbeat_task);

        // Election check task
        let service_clone = Arc::clone(self);
        let election_task = tokio::spawn(async move {
            service_clone.election_check_loop().await;
        });
        tasks.push(election_task);

        // Health check task
        let service_clone = Arc::clone(self);
        let health_task = tokio::spawn(async move {
            service_clone.health_check_loop().await;
        });
        tasks.push(health_task);

        info!("Started {} background tasks", tasks.len());
        Ok(())
    }

    /// Heartbeat loop
    async fn heartbeat_loop(&self) {
        let interval = self.config.heartbeat.interval();
        let mut interval_timer = tokio::time::interval(interval);

        loop {
            interval_timer.tick().await;

            // Update our own heartbeat
            let self_id = {
                let manager = self.manager.read().await;
                manager.get_self_id()
            };

            if let Err(e) = self.manager.write().await.update_node_heartbeat(&self_id) {
                warn!("Failed to update heartbeat: {}", e);
            }

            // Record success in fault tolerance tracker
            self.fault_tolerance.write().await.record_heartbeat(&self_id);

            debug!("Heartbeat sent");
        }
    }

    /// Election check loop
    async fn election_check_loop(&self) {
        let timeout = self.config.election.timeout();
        let mut check_timer = tokio::time::interval(timeout);

        loop {
            check_timer.tick().await;

            // Check if we should start an election
            {
                let election = self.election.lock().await;
                if election.should_start_election() {
                    let manager = self.manager.read().await;
                    if !manager.is_leader() && manager.has_quorum() {
                        drop(manager);
                        drop(election);

                        if let Err(e) = self.attempt_election().await {
                            warn!("Election attempt failed: {}", e);
                        }
                    }
                }
            }
        }
    }

    /// Health check loop
    async fn health_check_loop(&self) {
        let interval = self.config.heartbeat.timeout();
        let mut check_timer = tokio::time::interval(interval);

        loop {
            check_timer.tick().await;

            // Check health of all nodes and update fault tolerance tracker
            let unhealthy_nodes = {
                let manager = self.manager.read().await;
                manager.unhealthy_nodes()
            };

            for node in &unhealthy_nodes {
                // Record failure in fault tolerance manager
                let new_state = self.fault_tolerance.write().await.record_heartbeat_failure(
                    &node.id,
                    format!("Node {} failed health check", node.id),
                );

                warn!(
                    "Node {} is unhealthy (state: {:?}, consecutive_failures: {})",
                    node.id,
                    new_state,
                    self.fault_tolerance.read().await
                        .get_node_state(&node.id)
                        .map(|_| 0) // Placeholder - actual count tracked internally
                        .unwrap_or(0)
                );

                // Check if node should be removed based on graded state
                if self.fault_tolerance.read().await.should_remove_node(&node.id) {
                    error!(
                        "Node {} has reached offline threshold, initiating removal",
                        node.id
                    );

                    // Attempt to unregister the node
                    if let Err(e) = self.manager.write().await.unregister_node(&node.id) {
                        error!("Failed to unregister unhealthy node {}: {}", node.id, e);
                    } else {
                        info!("Successfully unregistered unhealthy node: {}", node.id);

                        // Mark as removed in fault tolerance tracker
                        self.fault_tolerance.write().await.mark_node_removed(&node.id);

                        // TODO: Trigger layer rebalance via UnifiedScheduler
                        // This would require passing a reference to the scheduler
                    }
                } else if new_state == super::fault_tolerance::NodeHealthState::Critical {
                    warn!(
                        "Node {} is in CRITICAL state - monitoring closely for removal",
                        node.id
                    );
                }
            }

            // Periodically clean up old trackers
            self.fault_tolerance.write().await.cleanup_removed_nodes(3600); // 1 hour

            // Log health summary periodically
            static SUMMARY_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            if SUMMARY_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % 60 == 0 {
                let summary = self.fault_tolerance.read().await.get_health_summary();
                info!(
                    "Cluster health summary: total={}, healthy={}, warning={}, critical={}, offline={}",
                    summary.total_nodes,
                    summary.healthy,
                    summary.warning,
                    summary.critical,
                    summary.offline
                );

                if summary.needs_attention() {
                    warn!("Cluster requires attention: {} critical, {} offline nodes",
                        summary.critical, summary.offline);
                }
            }
        }
    }

    /// Attempt to become leader through election
    async fn attempt_election(&self) -> Result<(), String> {
        let node_id = {
            let manager = self.manager.read().await;
            manager.get_self().ok_or("Self node not found")?.id.clone()
        };

        let mut election = self.election.lock().await;
        let manager = self.manager.read().await;

        match election.start_election(&node_id, &manager, 0, 0).await {
            Ok(NodeRole::Leader) => {
                drop(manager);
                drop(election);

                // Set ourselves as leader
                let manager = self.manager.write().await;
                manager.set_leader(&node_id)?;

                info!("This node ({}) became LEADER", node_id);
                Ok(())
            }
            Ok(NodeRole::Candidate) => {
                info!("This node ({}) is a candidate, waiting for votes", node_id);
                Ok(())
            }
            Ok(_) => {
                Ok(())
            }
            Err(e) => {
                error!("Election failed: {}", e);
                Err(e.to_string())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_disabled_service() {
        let config = ClusterConfig::default();
        let result = ClusterService::new(config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_create_enabled_service() {
        let config = ClusterConfig::new().enable();
        let result = ClusterService::new(config).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_service_state_transitions() {
        let config = ClusterConfig::new().enable();
        let service = ClusterService::new(config).await.unwrap();

        assert_eq!(service.get_state().await, ServiceState::Initialized);

        // Note: We can't fully test start/stop without actual network setup
        // but we can verify the initial state
    }
}
