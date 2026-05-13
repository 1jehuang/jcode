//! Unit tests for Monitoring module
//!
//! Tests cover:
//! - Time series data management
//! - Metric recording and statistics
//! - Alert rule creation and evaluation
//! - Health check system
//! - Dashboard data generation
//! - Event broadcasting

use carpai::monitoring::{
    MonitorManager, TimeSeries, TimeSeriesStats, TimeSeriesPoint,
    AlertRule, AlertCondition, AlertSeverity, AlertEvent,
    HealthCheck, HealthCheckResult, SystemHealth,
    MonitorEvent, DashboardData, MetricType, MetricData,
    MemoryHealthCheck, DiskSpaceHealthCheck,
};
use std::time::Duration;

// ════════════════════════════════════════════════════════════════
// Time Series Tests
// ════════════════════════════════════════════════════════════════

#[test]
fn test_time_series_creation() {
    let series = TimeSeries::new("test-metric", 100);
    
    assert!(series.current().is_empty(), "New series should have no current value");
    assert_eq!(series.points().len(), 0, "New series should be empty");
    
    println!("✓ Time series creation works");
}

#[test]
fn test_time_series_push_single() {
    let mut series = TimeSeries::new("single-metric", 100);
    
    series.push(42.5);
    
    assert_eq!(series.current(), Some(42.5));
    assert_eq!(series.points().len(), 1);
    
    println!("✓ Single value push works");
}

#[test]
fn test_time_series_push_multiple() {
    let mut series = TimeSeries::new("multi-metric", 100);
    
    for i in 1..=10 {
        series.push(i as f64);
    }
    
    assert_eq!(series.current(), Some(10.0), "Current should be last pushed value");
    assert_eq!(series.points().len(), 10);
    
    println!("✓ Multiple values push works");
}

#[test]
fn test_time_series_max_points_limit() {
    let mut series = TimeSeries::new("limited", 5);
    
    // Push more than max_points
    for i in 1..=10 {
        series.push(i as f64);
    }
    
    // Should only keep last 5 points
    assert_eq!(series.points().len(), 5, "Should respect max_points limit");
    assert_eq!(series.current(), Some(10.0), "Should have latest value");
    
    // First point should be 6 (oldest kept)
    if let Some(first) = series.points().front() {
        assert_eq!(first.value, 6.0, "Oldest point should be 6.0");
    }
    
    println!("✓ Max points limit enforcement works");
}

#[test]
fn test_time_series_stats_calculation() {
    let mut series = TimeSeries::new("stats-test", 100);
    
    // Push known values: 2, 4, 6, 8, 10
    for val in [2.0, 4.0, 6.0, 8.0, 10.0] {
        series.push(val);
    }
    
    let stats = series.stats();
    
    assert_eq!(stats.count, 5);
    assert!((stats.min - 2.0).abs() < f64::EPSILON, "Min should be 2.0");
    assert!((stats.max - 10.0).abs() < f64::EPSILON, "Max should be 10.0");
    assert!((stats.avg - 6.0).abs() < f64::EPSILON, "Avg should be 6.0");
    assert!((stats.sum - 30.0).abs() < f64::EPSILON, "Sum should be 30.0");
    
    println!("✓ Statistics calculation is accurate");
}

#[test]
fn test_time_series_stats_empty() {
    let series = TimeSeries::new("empty-stats", 100);
    let stats = series.stats();
    
    assert_eq!(stats.count, 0);
    assert!((stats.min - 0.0).abs() < f64::EPSILON);
    assert!((stats.max - 0.0).abs() < f64::EPSILON);
    assert!((stats.avg - 0.0).abs() < f64::EPSILON);
    
    println!("✓ Empty time series stats handled correctly");
}

// ════════════════════════════════════════════════════════════════
// Alert Rule Tests
// ════════════════════════════════════════════════════════════════

#[test]
fn test_alert_rule_creation() {
    let rule = AlertRule {
        name: "high-cpu".to_string(),
        metric_name: "cpu_usage".to_string(),
        condition: AlertCondition::GreaterThan,
        threshold: 80.0,
        severity: AlertSeverity::Warning,
        duration_secs: 300,
        enabled: true,
    };
    
    assert_eq!(rule.name, "high-cpu");
    assert_eq!(rule.metric_name, "cpu_usage");
    assert_eq!(rule.threshold, 80.0);
    assert!(rule.enabled);
    
    println!("✓ Alert rule creation works");
}

#[test]
fn test_alert_condition_display() {
    let tests = vec![
        (AlertCondition::GreaterThan, ">"),
        (AlertCondition::LessThan, "<"),
        (AlertCondition::EqualTo, "=="),
        (AlertCondition::NotEqualTo, "!="),
        (AlertCondition::IncreasesBy(50.0), "increases by 50%"),
        (AlertCondition::DecreasesBy(25.0), "decreases by 25%"),
    ];
    
    for (condition, expected) in tests {
        let display = format!("{}", condition);
        assert_eq!(display, expected, "Display for {:?} should be '{}'", condition, expected);
    }
    
    println!("✓ All alert condition display formats correct");
}

#[test]
fn test_alert_severity_ordering() {
    assert!(AlertSeverity::Critical > AlertSeverity::Warning);
    assert!(AlertSeverity::Warning > AlertSeverity::Info);
    
    let severities = vec![
        AlertSeverity::Info,
        AlertSeverity::Warning,
        AlertSeverity::Critical,
    ];
    
    let sorted: Vec<_> = {
        let mut s = severities.clone();
        s.sort();
        s
    };
    
    assert_eq!(sorted[0], AlertSeverity::Info);
    assert_eq!(sorted[2], AlertSeverity::Critical);
    
    println!("✓ Alert severity ordering works correctly");
}

#[test]
fn test_alert_severity_display() {
    assert_eq!(format!("{}", AlertSeverity::Info), "INFO");
    assert_eq!(format!("{}", AlertSeverity::Warning), "WARNING");
    assert_eq!(format!("{}", AlertSeverity::Critical), "CRITICAL");
    
    println!("✓ Alert severity display formats correct");
}

#[tokio::test]
async fn test_monitor_manager_creation() {
    let (manager, _rx) = MonitorManager::new();
    
    let dashboard = manager.get_dashboard_data().await;
    assert!(dashboard.metrics.is_empty(), "New manager should have no metrics");
    assert_eq!(dashboard.active_alert_rules, 0);
    assert!(dashboard.recent_alerts.is_empty());
    
    println!("✓ Monitor manager creates with empty state");
}

// ════════════════════════════════════════════════════════════════
// Metric Recording Tests
// ════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_record_single_metric() {
    let (manager, mut rx) = MonitorManager::new();
    
    manager.record_metric("test_counter", 42.0).await;
    
    let stats = manager.get_metric_stats("test_counter").await;
    assert!(stats.is_some(), "Metric should exist after recording");
    
    let stats = stats.unwrap();
    assert_eq!(stats.count, 1);
    assert!((stats.sum - 42.0).abs() < f64::EPSILON);
    
    // Check event was broadcast
    let event = rx.try_recv();
    assert!(event.is_ok(), "Should have received metric update event");
    
    match event.unwrap() {
        MonitorEvent::MetricUpdate { name, value } => {
            assert_eq!(name, "test_counter");
            assert!((value - 42.0).abs() < f64::EPSILON);
        }
        other => panic!("Expected MetricUpdate, got {:?}", other),
    }
    
    println!("✓ Single metric recording and event broadcast work");
}

#[tokio::test]
async fn test_record_multiple_metrics() {
    let (manager, _rx) = MonitorManager::new();
    
    for i in 1..=100 {
        manager.record_metric("counter", i as f64).await;
    }
    
    let stats = manager.get_metric_stats("counter").await.unwrap();
    
    assert_eq!(stats.count, 100);
    assert!((stats.min - 1.0).abs() < f64::EPSILON);
    assert!((stats.max - 100.0).abs() < f64::EPSILON);
    assert!((stats.avg - 50.5).abs() < 0.01); // Average of 1-100
    
    println!("✓ Multiple metric recording with aggregation works");
}

#[tokio::test]
async fn test_record_different_metrics() {
    let (manager, _rx) = MonitorManager::new();
    
    manager.record_metric("cpu", 75.5).await;
    manager.record_metric("memory", 60.0).await;
    manager.record_metric("disk", 45.2).await;
    
    let cpu_stats = manager.get_metric_stats("cpu").await;
    let mem_stats = manager.get_metric_stats("memory").await;
    let disk_stats = manager.get_metric_stats("disk").await;
    
    assert!(cpu_stats.is_some());
    assert!(mem_stats.is_some());
    assert!(disk_stats.is_some());
    
    assert!((cpu_stats.unwrap().current_value - 75.5).abs() < f64::EPSILON);
    
    println!("✓ Recording different metrics independently works");
}

// ════════════════════════════════════════════════════════════════
// Alert System Tests
// ════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_add_and_remove_alert_rule() {
    let (manager, _rx) = MonitorManager::new();
    
    let rule = AlertRule {
        name: "test-rule".to_string(),
        metric_name: "test".to_string(),
        condition: AlertCondition::GreaterThan,
        threshold: 100.0,
        severity: AlertSeverity::Warning,
        duration_secs: 300,
        enabled: true,
    };
    
    manager.add_alert_rule(rule).await;
    
    // Remove it
    let result = manager.remove_alert_rule("test-rule").await;
    assert!(result.is_ok(), "Should successfully remove existing rule");
    
    // Try to remove again - should fail
    let result = manager.remove_alert_rule("test-rule").await;
    assert!(result.is_err(), "Removing non-existent rule should fail");
    
    println!("✓ Add/remove alert rules work correctly");
}

#[tokio::test]
async fn test_alert_evaluation_triggers_correctly() {
    let (manager, mut rx) = MonitorManager::new();
    
    // Add alert rule: trigger when cpu > 80
    let rule = AlertRule {
        name: "high-cpu-alert".to_string(),
        metric_name: "cpu".to_string(),
        condition: AlertCondition::GreaterThan,
        threshold: 80.0,
        severity: AlertSeverity::Warning,
        duration_secs: 0,
        enabled: true,
    };
    
    manager.add_alert_rule(rule).await;
    
    // Record value that triggers alert
    manager.record_metric("cpu", 95.0).await;
    
    let alerts = manager.evaluate_alerts().await;
    assert_eq!(alerts.len(), 1, "Should trigger one alert");
    assert_eq!(alerts[0].name, "high-cpu-alert");
    assert_eq!(alerts[0].severity, AlertSeverity::Warning);
    assert!((alerts[0].actual_value - 95.0).abs() < f64::EPSILON);
    
    // Check alert was broadcast
    let event = rx.try_recv().unwrap();
    match event {
        MonitorEvent::AlertTriggered(alert) => {
            assert_eq!(alert.name, "high-cpu-alert");
        }
        other => panic!("Expected AlertTriggered, got {:?}", other),
    }
    
    println!("✓ Alert evaluation triggers when conditions met");
}

#[tokio::test]
async fn test_alert_no_trigger_when_disabled() {
    let (manager, _rx) = MonitorManager::new();
    
    let rule = AlertRule {
        name: "disabled-rule".to_string(),
        metric_name: "test".to_string(),
        condition: AlertCondition::GreaterThan,
        threshold: 10.0,
        severity: AlertSeverity::Critical,
        duration_secs: 0,
        enabled: false, // Disabled!
    };
    
    manager.add_alert_rule(rule).await;
    manager.record_metric("test", 100.0).await; // Would trigger if enabled
    
    let alerts = manager.evaluate_alerts().await;
    assert!(alerts.is_empty(), "Disabled rules should not trigger alerts");
    
    println!("✓ Disabled alert rules do not trigger");
}

#[tokio::test]
async fn test_get_recent_alerts() {
    let (manager, _rx) = MonitorManager::new();
    
    // Initially no alerts
    let recent = manager.get_recent_alerts(Some(10), false).await;
    assert!(recent.is_empty());
    
    // Get unresolved only - still none
    let unresolved = manager.get_recent_alerts(None, true).await;
    assert!(unresolved.is_empty());
    
    println!("✓ Getting recent alerts handles empty state");
}

// ════════════════════════════════════════════════════════════════
// Health Check Tests
// ════════════════════════════════════════════════════════════════

struct DummyHealthCheck {
    healthy: bool,
    message: String,
}

impl DummyHealthCheck {
    fn new(healthy: bool, message: impl Into<String>) -> Self {
        Self { healthy, message: message.into() }
    }
}

#[async_trait]
impl HealthCheck for DummyHealthCheck {
    fn name(&self) -> &str {
        "dummy-check"
    }
    
    async fn check(&self) -> HealthCheckResult {
        HealthCheckResult {
            component: self.name().to_string(),
            healthy: self.healthy,
            message: self.message.clone(),
            response_time_ms: Some(1.0),
            last_check: chrono::Utc::now(),
        }
    }
}

#[tokio::test]
async fn test_register_and_run_health_checks() {
    let (manager, mut rx) = MonitorManager::new();
    
    manager.register_health_check(DummyHealthCheck::new(true, "All good")).await;
    manager.register_health_check(DummyHealthCheck::new(true, "Also fine")).await;
    
    let health = manager.run_health_checks().await;
    
    assert!(health.overall_healthy, "All healthy checks should report overall healthy");
    assert_eq!(health.components.len(), 2);
    
    // Check health event was broadcast
    let event = rx.try_recv().unwrap();
    match event {
        MonitorEvent::HealthCheckComplete(h) => {
            assert!(h.overall_healthy);
        }
        other => panic!("Expected HealthCheckComplete, got {:?}", other),
    }
    
    println!("✓ Health check registration and execution work");
}

#[tokio::test]
async fn test_health_check_failure_detection() {
    let (manager, _rx) = MonitorManager::new();
    
    manager.register_health_check(
        DummyHealthCheck::new(false, "Something broke")
    ).await;
    
    let health = manager.run_health_checks().await;
    
    assert!(!health.overall_healthy, "Unhealthy check should cause overall unhealthy");
    
    let failed_component = &health.components[0];
    assert!(!failed_component.healthy);
    assert_eq!(failed_component.message, "Something broke");
    
    println!("✓ Health check failure detection works");
}

#[tokio::test]
async fn test_system_health_uptime_tracking() {
    let (manager, _rx) = MonitorManager::new();
    
    // Give it a moment
    tokio::time::sleep(Duration::from_millis(10)).await;
    
    let health = manager.run_health_checks().await;
    
    assert!(health.uptime_seconds > 0, "Uptime should be > 0 after creation");
    assert!(health.last_check <= chrono::Utc::now());
    
    println!("✓ System health tracks uptime correctly");
}

// ════════════════════════════════════════════════════════════════
// Dashboard Data Tests
// ════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_dashboard_data_generation() {
    let (manager, _rx) = MonitorManager::new();
    
    // Record some metrics
    manager.record_metric("metric_a", 10.0).await;
    manager.record_metric("metric_b", 20.0).await;
    manager.record_metric("metric_a", 15.0).await; // Update
    
    let dashboard = manager.get_dashboard_data().await;
    
    assert_eq!(dashboard.metrics.len(), 2, "Should have 2 unique metrics");
    assert!(dashboard.metrics.contains_key("metric_a"));
    assert!(dashboard.metrics.contains_key("metric_b"));
    
    // Check metric_a has latest value
    let metric_a = &dashboard.metrics["metric_a"];
    assert_eq!(metric_a.current, Some(15.0));
    
    println!("✓ Dashboard data generation includes all metrics");
}

#[tokio::test]
async fn test_dashboard_includes_alert_info() {
    let (manager, _rx) = MonitorManager::new();
    
    // Add an alert rule
    let rule = AlertRule {
        name: "dashboard-test".to_string(),
        metric_name: "test".to_string(),
        condition: AlertCondition::GreaterThan,
        threshold: 50.0,
        severity: AlertSeverity::Info,
        duration_secs: 0,
        enabled: true,
    };
    manager.add_alert_rule(rule).await;
    
    let dashboard = manager.get_dashboard_data().await;
    
    assert_eq!(dashboard.active_alert_rules, 1, "Should count active alert rules");
    assert!(dashboard.system_uptime >= Duration::ZERO);
    
    println!("✓ Dashboard includes alert rule count and system info");
}

// ════════════════════════════════════════════════════════════════
// Event Subscription Tests
// ════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_multiple_subscribers_receive_events() {
    let (manager, rx1) = MonitorManager::new();
    let rx2 = manager.subscribe();
    
    manager.record_metric("shared", 99.9).await;
    
    // Both receivers should get the event
    let event1 = rx1.try_recv().unwrap();
    let event2 = rx2.try_recv().unwrap();
    
    match (event1, event2) {
        (
            MonitorEvent::MetricUpdate { name: n1, .. },
            MonitorEvent::MetricUpdate { name: n2, .. }
        ) => {
            assert_eq!(n1, "shared");
            assert_eq!(n2, "shared");
        }
        other => panic!("Both should be MetricUpdate, got {:?}", other),
    }
    
    println!("✓ Multiple subscribers receive same events");
}

// ════════════════════════════════════════════════════════════════
// Edge Cases and Error Handling
// ════════════════════════════════════════════════════════════════

#[test]
fn test_alert_event_serialization() {
    let event = AlertEvent {
        id: "alert-001".to_string(),
        name: "test-alert".to_string(),
        message: "Test alert triggered".to_string(),
        severity: AlertSeverity::Critical,
        metric_name: "cpu".to_string(),
        threshold: 90.0,
        actual_value: 95.5,
        timestamp: chrono::Utc::now(),
        resolved: false,
    };
    
    let json = serde_json::to_string(&event).expect("Serialization failed");
    let parsed: AlertEvent = serde_json::from_str(&json).expect("Deserialization failed");
    
    assert_eq!(parsed.id, event.id);
    assert_eq!(parsed.severity, AlertSeverity::Critical);
    assert!((parsed.actual_value - 95.5).abs() < f64::EPSILON);
    
    println!("✓ Alert event serialization round-trips correctly");
}

#[test]
fn test_time_series_point_serialization() {
    let point = TimeSeriesPoint {
        timestamp: chrono::Utc::now(),
        value: 123.456,
    };
    
    let json = serde_json::to_value(&point).expect("Serialization failed");
    assert!(json.get("timestamp").is_some());
    assert!(json.get("value").is_some());
    
    let parsed: TimeSeriesPoint = serde_json::from_value(json).expect("Deserialization failed");
    assert!((parsed.value - 123.456).abs() < f64::EPSILON);
    
    println!("✓ Time series point serialization works");
}
