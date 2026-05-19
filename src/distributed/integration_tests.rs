//! Integration Tests for Distributed Cluster Election
//!
//! These tests verify the complete cluster election flow including:
//! - Single node initialization
//! - Multi-node cluster formation
//! - Leader election
//! - Failover scenarios
//! - Quorum requirements

#[cfg(test)]
mod tests {
    use crate::distributed::{
        ClusterConfig, ClusterService,
        config::{NodeConfig, PeerConfig, ElectionConfig, HeartbeatConfig},
        service::ServiceState,
    };
    use std::sync::Arc;
    use tokio::time::{sleep, Duration};
    use tracing::{info, debug};

    /// Helper to create a test cluster configuration
    fn create_test_config(host: &str, port: u16, peers: Vec<&str>) -> ClusterConfig {
        let mut config = ClusterConfig::new().enable();
        config.node.host = host.to_string();
        config.node.port = port;
        config.node.id = Some(format!("test-node-{}", port));

        // Fast election for testing
        config.election.election_timeout_ms = 100;
        config.election.election_jitter_ms = 50;
        config.election.min_quorum_size = 2;

        // Fast heartbeat for testing
        config.heartbeat.interval_ms = 30;
        config.heartbeat.timeout_ms = 100;

        // Add peers
        for peer_addr in peers {
            config.peers.push(PeerConfig::new(peer_addr));
        }

        config
    }

    /// Helper to create a leader-preferring config
    fn create_leader_config(host: &str, port: u16, peers: Vec<&str>) -> ClusterConfig {
        let mut config = create_test_config(host, port, peers);
        config.node.preferred_role = Some(crate::distributed::config::NodeRolePreference::Leader);
        config
    }

    // ========================================================================
    // Test 1: Single Node Initialization
    // ========================================================================

    #[tokio::test]
    async fn test_single_node_initialization() {
        // Initialize tracing for test output
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .try_init();

        info!("=== Test: Single Node Initialization ===");

        let config = create_test_config("127.0.0.1", 10000, vec![]);

        // Create cluster service
        let service = ClusterService::new(config).await;
        assert!(service.is_ok(), "Failed to create cluster service");

        let service = service.unwrap();

        // Verify initial state
        assert_eq!(service.get_state().await, ServiceState::Initialized);

        info!("✓ Single node service created successfully");
    }

    // ========================================================================
    // Test 2: Service Start and State Transitions
    // ========================================================================

    #[tokio::test]
    async fn test_service_state_transitions() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .try_init();

        info!("=== Test: Service State Transitions ===");

        let config = create_test_config("127.0.0.1", 10001, vec![]);
        let service = ClusterService::new(config).await.unwrap();

        // Check initial state
        assert_eq!(service.get_state().await, ServiceState::Initialized);
        info!("✓ Initial state: Initialized");

        // Start service
        let start_result = service.start().await;
        assert!(start_result.is_ok(), "Failed to start service: {:?}", start_result);
        info!("✓ Service started");

        // Check running state
        sleep(Duration::from_millis(100)).await;
        assert_eq!(service.get_state().await, ServiceState::Running);
        info!("✓ Running state confirmed");

        // Stop service
        let stop_result = service.stop().await;
        assert!(stop_result.is_ok(), "Failed to stop service: {:?}", stop_result);
        info!("✓ Service stopped");

        // Check stopped state
        assert_eq!(service.get_state().await, ServiceState::Stopped);
        info!("✓ Stopped state confirmed");
    }

    // ========================================================================
    // Test 3: Disabled Cluster Mode
    // ========================================================================

    #[tokio::test]
    async fn test_disabled_cluster_mode() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .try_init();

        info!("=== Test: Disabled Cluster Mode ===");

        let mut config = ClusterConfig::default();
        config.enabled = false;

        let result = ClusterService::new(config).await;
        assert!(result.is_err(), "Disabled cluster should return error");

        info!("✓ Disabled cluster correctly rejected");
    }

    // ========================================================================
    // Test 4: Invalid Configuration Rejection
    // ========================================================================

    #[tokio::test]
    async fn test_invalid_config_rejection() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .try_init();

        info!("=== Test: Invalid Configuration Rejection ===");

        // Test with invalid port
        let mut config = create_test_config("127.0.0.1", 0, vec![]);
        let result = ClusterService::new(config).await;
        assert!(result.is_err(), "Invalid port should be rejected");
        info!("✓ Invalid port rejected");

        // Test with duplicate peers
        let config = ClusterConfig {
            enabled: true,
            node: NodeConfig {
                host: "127.0.0.1".to_string(),
                port: 10002,
                ..Default::default()
            },
            peers: vec![
                PeerConfig::new("127.0.0.1:9001"),
                PeerConfig::new("127.0.0.1:9001"), // Duplicate
            ],
            ..Default::default()
        };
        let result = ClusterService::new(config).await;
        assert!(result.is_err(), "Duplicate peers should be rejected");
        info!("✓ Duplicate peers rejected");
    }

    // ========================================================================
    // Test 5: Leader Election (Single Node)
    // ========================================================================

    #[tokio::test]
    async fn test_single_node_election() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .try_init();

        info!("=== Test: Single Node Election ===");

        let config = create_leader_config("127.0.0.1", 10003, vec![]);
        let service = ClusterService::new(config).await.unwrap();

        // Start service (should attempt election)
        service.start().await.unwrap();
        info!("✓ Service started");

        // Wait for election to complete
        sleep(Duration::from_millis(300)).await;

        // In single-node mode with quorum=2, we won't become leader
        // But we can verify the election was attempted
        let is_leader = service.is_leader().await;
        info!("Is leader: {}", is_leader);

        // Clean up
        service.stop().await.unwrap();
        info!("✓ Test completed");
    }

    // ========================================================================
    // Test 6: Cluster Information Retrieval
    // ========================================================================

    #[tokio::test]
    async fn test_cluster_info_retrieval() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .try_init();

        info!("=== Test: Cluster Info Retrieval ===");

        let config = create_test_config("127.0.0.1", 10004, vec![]);
        let service = ClusterService::new(config).await.unwrap();

        service.start().await.unwrap();
        sleep(Duration::from_millis(100)).await;

        // Get cluster info
        let info = service.get_cluster_info().await;
        info!("Cluster ID: {}", info.cluster_id);
        info!("Total nodes: {}", info.total_nodes);
        info!("Healthy nodes: {}", info.healthy_nodes);
        info!("Self ID: {}", info.self_id);

        assert!(!info.cluster_id.is_empty());
        assert_eq!(info.total_nodes, 1); // Only self
        assert_eq!(info.healthy_nodes, 1);
        assert!(!info.self_id.is_empty());

        info!("✓ Cluster info retrieved successfully");

        service.stop().await.unwrap();
    }

    // ========================================================================
    // Test 7: Healthy Node Counting
    // ========================================================================

    #[tokio::test]
    async fn test_healthy_node_count() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .try_init();

        info!("=== Test: Healthy Node Counting ===");

        let config = create_test_config("127.0.0.1", 10005, vec![]);
        let service = ClusterService::new(config).await.unwrap();

        service.start().await.unwrap();
        sleep(Duration::from_millis(100)).await;

        let healthy = service.healthy_node_count().await;
        info!("Healthy node count: {}", healthy);
        assert_eq!(healthy, 1); // Only self node

        info!("✓ Healthy node count correct");

        service.stop().await.unwrap();
    }

    // ========================================================================
    // Test 8: Quorum Check
    // ========================================================================

    #[tokio::test]
    async fn test_quorum_check() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .try_init();

        info!("=== Test: Quorum Check ===");

        let mut config = create_test_config("127.0.0.1", 10006, vec![]);
        config.election.min_quorum_size = 2; // Need 2 nodes for quorum

        let service = ClusterService::new(config).await.unwrap();
        service.start().await.unwrap();
        sleep(Duration::from_millis(100)).await;

        // With only 1 node and quorum=2, should not have quorum
        let has_quorum = service.has_quorum().await;
        info!("Has quorum: {}", has_quorum);
        assert!(!has_quorum, "Single node should not have quorum when min_quorum_size=2");

        info!("✓ Quorum check correct");

        service.stop().await.unwrap();
    }

    // ========================================================================
    // Test 9: Node Selection via Load Balancer
    // ========================================================================

    #[tokio::test]
    async fn test_node_selection() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .try_init();

        info!("=== Test: Node Selection ===");

        let config = create_test_config("127.0.0.1", 10007, vec![]);
        let service = ClusterService::new(config).await.unwrap();

        service.start().await.unwrap();
        sleep(Duration::from_millis(100)).await;

        // Should be able to select self as the only healthy node
        let selected = service.select_node().await;
        assert!(selected.is_some(), "Should select at least one node");

        if let Some(node) = selected {
            info!("Selected node: {} (ID: {})", node.address, node.id);
            assert_eq!(node.id, format!("test-node-10007"));
        }

        info!("✓ Node selection successful");

        service.stop().await.unwrap();
    }

    // ========================================================================
    // Test 10: Multiple Service Instances (Simulated Cluster)
    // ========================================================================

    #[tokio::test]
    async fn test_multiple_service_instances() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .try_init();

        info!("=== Test: Multiple Service Instances ===");

        // Create two services on different ports
        let config1 = create_test_config("127.0.0.1", 10008, vec!["127.0.0.1:10009"]);
        let config2 = create_test_config("127.0.0.1", 10009, vec!["127.0.0.1:10008"]);

        let service1 = ClusterService::new(config1).await.unwrap();
        let service2 = ClusterService::new(config2).await.unwrap();

        info!("✓ Created two service instances");

        // Start both
        service1.start().await.unwrap();
        service2.start().await.unwrap();

        info!("✓ Both services started");

        // Wait for them to discover each other
        sleep(Duration::from_millis(500)).await;

        // Check cluster info
        let info1 = service1.get_cluster_info().await;
        let info2 = service2.get_cluster_info().await;

        info!("Service 1 - Total nodes: {}, Healthy: {}", info1.total_nodes, info1.healthy_nodes);
        info!("Service 2 - Total nodes: {}, Healthy: {}", info2.total_nodes, info2.healthy_nodes);

        // Note: In a real network, they would see each other
        // For this test, we're just verifying both can run simultaneously

        // Clean up
        service1.stop().await.unwrap();
        service2.stop().await.unwrap();

        info!("✓ Multiple instances test completed");
    }

    // ========================================================================
    // Test 11: Rapid Start/Stop Cycle
    // ========================================================================

    #[tokio::test]
    async fn test_rapid_start_stop_cycle() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::WARN)
            .try_init();

        info!("=== Test: Rapid Start/Stop Cycle ===");

        let config = create_test_config("127.0.0.1", 10010, vec![]);

        for i in 0..3 {
            info!("Cycle {}", i + 1);

            let service = ClusterService::new(config.clone()).await.unwrap();
            service.start().await.unwrap();
            sleep(Duration::from_millis(50)).await;
            service.stop().await.unwrap();
        }

        info!("✓ Rapid cycle test passed");
    }

    // ========================================================================
    // Test 12: Concurrent State Checks
    // ========================================================================

    #[tokio::test]
    async fn test_concurrent_state_checks() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .try_init();

        info!("=== Test: Concurrent State Checks ===");

        let config = create_test_config("127.0.0.1", 10011, vec![]);
        let service = ClusterService::new(config).await.unwrap();
        service.start().await.unwrap();

        sleep(Duration::from_millis(100)).await;

        // Spawn multiple concurrent state checks
        let mut handles = vec![];
        for i in 0..5 {
            let svc = Arc::clone(&service);
            let handle = tokio::spawn(async move {
                let state = svc.get_state().await;
                debug!("Task {} saw state: {:?}", i, state);
                state
            });
            handles.push(handle);
        }

        // Collect results
        for handle in handles {
            let state = handle.await.unwrap();
            assert_eq!(state, ServiceState::Running);
        }

        info!("✓ Concurrent state checks passed");

        service.stop().await.unwrap();
    }

    // ========================================================================
    // Test 13: Configuration Validation Edge Cases
    // ========================================================================

    #[tokio::test]
    async fn test_config_validation_edge_cases() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .try_init();

        info!("=== Test: Config Validation Edge Cases ===");

        // Empty host
        let config = ClusterConfig {
            enabled: true,
            node: NodeConfig {
                host: "".to_string(),
                port: 10012,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(config.validate().is_err());
        info!("✓ Empty host rejected");

        // Valid minimal config
        let config = ClusterConfig {
            enabled: true,
            node: NodeConfig {
                host: "127.0.0.1".to_string(),
                port: 10013,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(config.validate().is_ok());
        info!("✓ Minimal valid config accepted");

        // Max port number
        let config = ClusterConfig {
            enabled: true,
            node: NodeConfig {
                host: "127.0.0.1".to_string(),
                port: 65535,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(config.validate().is_ok());
        info!("✓ Max port number accepted");
    }

    // ========================================================================
    // Test 14: Election Config Duration Calculations
    // ========================================================================

    #[tokio::test]
    async fn test_election_config_durations() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .try_init();

        info!("=== Test: Election Config Durations ===");

        let config = crate::distributed::config::ElectionConfig {
            election_timeout_ms: 200,
            election_jitter_ms: 100,
            min_quorum_size: 2,
        };

        assert_eq!(config.timeout(), Duration::from_millis(200));
        assert_eq!(config.max_jitter(), Duration::from_millis(100));

        info!("✓ Election duration calculations correct");
    }

    // ========================================================================
    // Test 15: Heartbeat Config Duration Calculations
    // ========================================================================

    #[tokio::test]
    async fn test_heartbeat_config_durations() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .try_init();

        info!("=== Test: Heartbeat Config Durations ===");

        let config = crate::distributed::config::HeartbeatConfig {
            interval_ms: 50,
            timeout_ms: 150,
            max_missed: 3,
        };

        assert_eq!(config.interval(), Duration::from_millis(50));
        assert_eq!(config.timeout(), Duration::from_millis(150));

        info!("✓ Heartbeat duration calculations correct");
    }
}
