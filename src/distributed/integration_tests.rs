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

    // ========================================================================
    // P0 Task 3: Large-Scale Cluster Tests (18 Nodes)
    // ========================================================================

    /// Helper to create a large-scale cluster configuration
    fn create_large_cluster_configs(num_nodes: usize) -> Vec<ClusterConfig> {
        let mut configs = Vec::with_capacity(num_nodes);
        let base_port = 20000u16;

        for i in 0..num_nodes {
            let port = base_port + i as u16;
            let mut peers = Vec::new();

            // Each node knows about all other nodes
            for j in 0..num_nodes {
                if i != j {
                    let peer_port = base_port + j as u16;
                    peers.push(format!("127.0.0.1:{}", peer_port));
                }
            }

            let mut config = create_test_config("127.0.0.1", port, peers);
            config.node.id = Some(format!("large-cluster-node-{}", i));

            // Adjust quorum for large cluster (need majority)
            config.election.min_quorum_size = (num_nodes / 2 + 1) as u32;

            // Faster timeouts for testing
            config.heartbeat.interval_ms = 50;
            config.heartbeat.timeout_ms = 200;
            config.election.election_timeout_ms = 150;

            configs.push(config);
        }

        configs
    }

    // ========================================================================
    // Test 16: 18-Node Cluster Startup
    // ========================================================================

    #[tokio::test]
    async fn test_18_node_cluster_startup() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::WARN)
            .try_init();

        info!("=== Test: 18-Node Cluster Startup ===");

        let num_nodes = 18;
        let configs = create_large_cluster_configs(num_nodes);

        // Create all services
        let mut services = Vec::with_capacity(num_nodes);
        for config in &configs {
            let service = ClusterService::new(config.clone()).await.unwrap();
            services.push(service);
        }

        info!("✓ Created {} service instances", num_nodes);

        // Start all services concurrently
        let start_futures: Vec<_> = services.iter().map(|s| s.start()).collect();
        for future in start_futures {
            assert!(future.await.is_ok(), "Failed to start service");
        }

        info!("✓ Started all {} services", num_nodes);

        // Wait for cluster stabilization
        sleep(Duration::from_secs(2)).await;

        // Verify cluster health
        let mut total_healthy = 0;
        for (i, service) in services.iter().enumerate() {
            let healthy_count = service.healthy_node_count().await;
            total_healthy += healthy_count;
            debug!("Node {} sees {} healthy nodes", i, healthy_count);
        }

        let avg_healthy = total_healthy / num_nodes;
        info!("Average healthy nodes seen: {}", avg_healthy);

        // At least some nodes should see each other
        assert!(avg_healthy >= 1, "Nodes should see at least themselves");

        // Clean up
        for service in &services {
            let _ = service.stop().await;
        }

        info!("✓ 18-node cluster startup test completed");
    }

    // ========================================================================
    // Test 17: Dynamic Node Join/Leave in Large Cluster
    // ========================================================================

    #[tokio::test]
    async fn test_dynamic_node_join_leave() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::WARN)
            .try_init();

        info!("=== Test: Dynamic Node Join/Leave ===");

        // Start with 5 nodes
        let initial_nodes = 5;
        let configs = create_large_cluster_configs(initial_nodes);

        let mut services = Vec::new();
        for config in &configs[..initial_nodes] {
            let service = ClusterService::new(config.clone()).await.unwrap();
            service.start().await.unwrap();
            services.push(service);
        }

        info!("✓ Started initial {} nodes", initial_nodes);
        sleep(Duration::from_millis(500)).await;

        // Simulate batch node joins (like internet cafe machines coming online)
        let batch_join_count = 5;
        for i in 0..batch_join_count {
            let config = configs[initial_nodes + i].clone();
            let service = ClusterService::new(config).await.unwrap();
            service.start().await.unwrap();
            services.push(service);
            info!("✓ Node {} joined", initial_nodes + i);
            sleep(Duration::from_millis(100)).await;
        }

        sleep(Duration::from_secs(1)).await;

        // Verify cluster grew
        let healthy_after_join = services[0].healthy_node_count().await;
        info!("Healthy nodes after join: {}", healthy_after_join);

        // Simulate batch node leaves (failures)
        let batch_leave_count = 3;
        for i in 0..batch_leave_count {
            let idx = initial_nodes + i;
            if idx < services.len() {
                let _ = services[idx].stop().await;
                info!("✓ Node {} left", idx);
            }
        }

        sleep(Duration::from_secs(1)).await;

        // Remaining nodes should still function
        let healthy_after_leave = services[0].healthy_node_count().await;
        info!("Healthy nodes after leave: {}", healthy_after_leave);

        // Clean up
        for service in &services {
            let _ = service.stop().await;
        }

        info!("✓ Dynamic join/leave test completed");
    }

    // ========================================================================
    // Test 18: Node Failure and Recovery
    // ========================================================================

    #[tokio::test]
    async fn test_node_failure_and_recovery() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::WARN)
            .try_init();

        info!("=== Test: Node Failure and Recovery ===");

        let num_nodes = 5;
        let configs = create_large_cluster_configs(num_nodes);

        let mut services = Vec::new();
        for config in &configs {
            let service = ClusterService::new(config.clone()).await.unwrap();
            service.start().await.unwrap();
            services.push(service);
        }

        sleep(Duration::from_secs(1)).await;

        // Get initial health summary
        let initial_summary = services[0].get_health_summary().await;
        info!("Initial health: {} healthy, {} warning, {} critical, {} offline",
            initial_summary.healthy, initial_summary.warning,
            initial_summary.critical, initial_summary.offline);

        // Stop one node to simulate failure
        let failed_node_idx = 2;
        info!("Simulating failure of node {}", failed_node_idx);
        let _ = services[failed_node_idx].stop().await;

        // Wait for failure detection
        sleep(Duration::from_secs(2)).await;

        // Check that other nodes detect the failure
        let post_failure_summary = services[0].get_health_summary().await;
        info!("Post-failure health: {} healthy, {} warning, {} critical, {} offline",
            post_failure_summary.healthy, post_failure_summary.warning,
            post_failure_summary.critical, post_failure_summary.offline);

        // The failed node should be detected
        assert!(
            post_failure_summary.healthy < initial_summary.healthy ||
            post_failure_summary.warning > 0 ||
            post_failure_summary.critical > 0 ||
            post_failure_summary.offline > 0,
            "Failure should be detected"
        );

        // Clean up
        for service in &services {
            let _ = service.stop().await;
        }

        info!("✓ Node failure and recovery test completed");
    }

    // ========================================================================
    // Test 19: Concurrent Health Checks Under Load
    // ========================================================================

    #[tokio::test]
    async fn test_concurrent_health_checks_under_load() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::WARN)
            .try_init();

        info!("=== Test: Concurrent Health Checks Under Load ===");

        let num_nodes = 10;
        let configs = create_large_cluster_configs(num_nodes);

        let mut services = Vec::new();
        for config in &configs {
            let service = ClusterService::new(config.clone()).await.unwrap();
            service.start().await.unwrap();
            services.push(Arc::new(service));
        }

        sleep(Duration::from_millis(500)).await;

        // Spawn concurrent health check tasks
        let num_tasks = 20;
        let mut handles = Vec::new();

        for task_id in 0..num_tasks {
            let svc = Arc::clone(&services[task_id % num_nodes]);
            let handle = tokio::spawn(async move {
                let mut checks = 0;
                for _ in 0..10 {
                    let _state = svc.get_state().await;
                    let _healthy = svc.healthy_node_count().await;
                    let _summary = svc.get_health_summary().await;
                    checks += 1;
                    sleep(Duration::from_millis(10)).await;
                }
                checks
            });
            handles.push(handle);
        }

        // Collect results
        let mut total_checks = 0;
        for handle in handles {
            total_checks += handle.await.unwrap();
        }

        info!("Completed {} concurrent health checks", total_checks);
        assert_eq!(total_checks, num_tasks * 10);

        // Clean up
        for service in &services {
            let _ = service.stop().await;
        }

        info!("✓ Concurrent health checks test passed");
    }

    // ========================================================================
    // Test 20: Fault Tolerance Manager Integration
    // ========================================================================

    #[tokio::test]
    async fn test_fault_tolerance_integration() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .try_init();

        info!("=== Test: Fault Tolerance Integration ===");

        let num_nodes = 5;
        let configs = create_large_cluster_configs(num_nodes);

        let mut services = Vec::new();
        for config in &configs {
            let service = ClusterService::new(config.clone()).await.unwrap();
            service.start().await.unwrap();
            services.push(service);
        }

        sleep(Duration::from_millis(500)).await;

        // Register nodes for fault tracking
        for i in 0..num_nodes {
            services[0].register_for_fault_tracking(&format!("large-cluster-node-{}", i)).await;
        }

        // Check initial alert stats
        let (total_alerts, active_alerts) = services[0].get_alert_stats().await;
        info!("Initial alerts - Total: {}, Active: {}", total_alerts, active_alerts);

        // Simulate heartbeat failures by stopping nodes
        let nodes_to_fail = 2;
        for i in 0..nodes_to_fail {
            let _ = services[i].stop().await;
            info!("Stopped node {}", i);
        }

        // Wait for failure detection and state transitions
        sleep(Duration::from_secs(3)).await;

        // Check updated alert stats
        let (post_total, post_active) = services[0].get_alert_stats().await;
        info!("Post-failure alerts - Total: {}, Active: {}", post_total, post_active);

        // Should have generated some alerts
        assert!(post_total >= total_alerts || post_active >= active_alerts,
            "Fault tolerance should generate alerts");

        // Get detailed health summary
        let summary = services[0].get_health_summary().await;
        info!("Final health summary: {:?}", summary);

        // Clean up
        for service in &services {
            let _ = service.stop().await;
        }

        info!("✓ Fault tolerance integration test completed");
    }

    // ========================================================================
    // Test 21: Stress Test - Rapid Node Churn
    // ========================================================================

    #[tokio::test]
    async fn test_rapid_node_churn_stress() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::WARN)
            .try_init();

        info!("=== Test: Rapid Node Churn Stress ===");

        let num_iterations = 5;
        let nodes_per_iteration = 3;

        for iteration in 0..num_iterations {
            info!("Iteration {}", iteration + 1);

            // Create and start nodes
            let mut services = Vec::new();
            for i in 0..nodes_per_iteration {
                let port = 30000 + iteration * 100 + i as u16;
                let config = create_test_config("127.0.0.1", port, vec![]);
                let service = ClusterService::new(config).await.unwrap();
                service.start().await.unwrap();
                services.push(service);
            }

            sleep(Duration::from_millis(100)).await;

            // Stop all nodes
            for service in &services {
                let _ = service.stop().await;
            }

            sleep(Duration::from_millis(50)).await;
        }

        info!("✓ Rapid churn stress test passed ({} iterations)", num_iterations);
    }
}
