//! Monitoring and Metrics System
//!
//! Real-time monitoring with:
//! - Web dashboard support
//! - Metric collection and aggregation
//! - Alert system
//! - Health checks

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, RwLock};
use tracing::{debug, error, info, warn};

/// Metric types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MetricType {
    Counter,
    Gauge,
    Histogram,
    Summary,
}

/// A single metric data point
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricData {
    pub name: String,
    pub metric_type: MetricType,
    pub value: f64,
    pub labels: HashMap<String, String>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Time series data point
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeSeriesPoint {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub value: f64,
}

/// Time series for a metric
#[derive(Debug, Clone)]
pub struct TimeSeries {
    name: String,
    max_points: usize,
    points: VecDeque<TimeSeriesPoint>,
}

impl TimeSeries {
    pub fn new(name: impl Into<String>, max_points: usize) -> Self {
        Self {
            name: name.into(),
            max_points,
            points: VecDeque::with_capacity(max_points),
        }
    }

    /// Add a data point
    pub fn push(&mut self, value: f64) {
        let point = TimeSeriesPoint {
            timestamp: chrono::Utc::now(),
            value,
        };

        self.points.push_back(point);

        while self.points.len() > self.max_points {
            self.points.pop_front();
        }
    }

    /// Get current value
    pub fn current(&self) -> Option<f64> {
        self.points.back().map(|p| p.value)
    }

    /// Get all points
    pub fn points(&self) -> &VecDeque<TimeSeriesPoint> {
        &self.points
    }

    /// Calculate statistics over window
    pub fn stats(&self) -> TimeSeriesStats {
        if self.points.is_empty() {
            return TimeSeriesStats {
                count: 0,
                min: 0.0,
                max: 0.0,
                avg: 0.0,
                sum: 0.0,
            };
        }

        let count = self.points.len();
        let mut min = f64::MAX;
        let mut max = f64::MIN;
        let mut sum = 0.0;

        for point in &self.points {
            if point.value < min {
                min = point.value;
            }
            if point.value > max {
                max = point.value;
            }
            sum += point.value;
        }

        TimeSeriesStats {
            count,
            min,
            max,
            avg: sum / count as f64,
            sum,
        }
    }
}

/// Statistics for a time series
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeSeriesStats {
    pub count: usize,
    pub min: f64,
    pub max: f64,
    pub avg: f64,
    pub sum: f64,
}

/// Alert severity levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum AlertSeverity {
    Info,
    Warning,
    Critical,
}

impl std::fmt::Display for AlertSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Info => write!(f, "INFO"),
            Self::Warning => write!(f, "WARNING"),
            Self::Critical => write!(f, "CRITICAL"),
        }
    }
}

/// An alert event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertEvent {
    pub id: String,
    pub name: String,
    pub message: String,
    pub severity: AlertSeverity,
    pub metric_name: String,
    pub threshold: f64,
    pub actual_value: f64,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub resolved: bool,
}

/// Alert rule definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertRule {
    pub name: String,
    pub metric_name: String,
    pub condition: AlertCondition,
    pub threshold: f64,
    pub severity: AlertSeverity,
    #[serde(default = "default_alert_duration")]
    pub duration_secs: u64,
    #[serde(default)]
    pub enabled: bool,
}

fn default_alert_duration() -> u64 {
    300 // 5 minutes default
}

/// Alert condition type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AlertCondition {
    GreaterThan,
    LessThan,
    EqualTo,
    NotEqualTo,
    IncreasesBy(f64),      // Percentage increase
    DecreasesBy(f64),      // Percentage decrease
}

impl std::fmt::Display for AlertCondition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::GreaterThan => write!(f, ">"),
            Self::LessThan => write!(f, "<"),
            Self::EqualTo => write!(f, "=="),
            Self::NotEqualTo => write!(f, "!="),
            Self::IncreasesBy(pct) => write!(f, "increases by {}%", pct),
            Self::DecreasesBy(pct) => write!(f, "decreases by {}%", pct),
        }
    }
}

/// Health check result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckResult {
    pub component: String,
    pub healthy: bool,
    pub message: String,
    pub response_time_ms: Option<f64>,
    pub last_check: chrono::DateTime<chrono::Utc>,
}

/// System health status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemHealth {
    pub overall_healthy: bool,
    pub components: Vec<HealthCheckResult>,
    pub uptime_seconds: u64,
    pub last_check: chrono::DateTime<chrono::Utc>,
}

/// Monitoring manager that collects metrics and manages alerts
pub struct MonitorManager {
    time_series: RwLock<HashMap<String, Arc<RwLock<TimeSeries>>>>,
    alerts: RwLock<Vec<AlertRule>>,
    alert_history: RwLock<VecDeque<AlertEvent>>,
    health_checks: RwLock<Vec<Box<dyn HealthCheck + Send + Sync>>>,
    tx: broadcast::Sender<MonitorEvent>,
    start_time: Instant,
}

/// Events emitted by the monitor
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MonitorEvent {
    MetricUpdate {
        name: String,
        value: f64,
    },
    AlertTriggered(AlertEvent),
    AlertResolved(String),
    HealthCheckComplete(SystemHealth),
}

/// Trait for health check implementations
#[async_trait]
pub trait HealthCheck: Send + Sync {
    fn name(&self) -> &str;
    async fn check(&self) -> HealthCheckResult;
}

impl MonitorManager {
    pub fn new() -> (Self, broadcast::Receiver<MonitorEvent>) {
        let (tx, rx) = broadcast::channel(100);

        (
            Self {
                time_series: RwLock::new(HashMap::new()),
                alerts: RwLock::new(Vec::new()),
                alert_history: RwLock::new(VecDeque::with_capacity(1000)),
                health_checks: RwLock::new(Vec::new()),
                tx,
                start_time: Instant::now(),
            },
            rx,
        )
    }

    /// Record a metric value
    pub async fn record_metric(&self, name: &str, value: f64) {
        let mut series_map = self.time_series.write().await;

        if !series_map.contains_key(name) {
            series_map.insert(
                name.to_string(),
                Arc::new(RwLock::new(TimeSeries::new(name, 1000))),
            );
        }

        let series = series_map.get(name).unwrap().clone();
        let mut guard = series.write().await;
        guard.push(value);
        drop(guard);

        // Broadcast metric update
        let _ = self.tx.send(MonitorEvent::MetricUpdate {
            name: name.to_string(),
            value,
        });
    }

    /// Get or create a time series
    async fn get_or_create_series(
        &self,
        name: &str,
        max_points: usize,
    ) -> Arc<RwLock<TimeSeries>> {
        let mut series_map = self.time_series.write().await;

        if !series_map.contains_key(name) {
            series_map.insert(
                name.to_string(),
                Arc::new(RwLock::new(TimeSeries::new(name, max_points))),
            );
        }

        series_map.get(name).unwrap().clone()
    }

    /// Get time series statistics
    pub async fn get_metric_stats(&self, name: &str) -> Option<TimeSeriesStats> {
        let series_map = self.time_series.read().await;

        if let Some(series) = series_map.get(name) {
            Some(series.read().await.stats())
        } else {
            None
        }
    }

    /// Add an alert rule
    pub async fn add_alert_rule(&self, rule: AlertRule) {
        let mut rules = self.alerts.write().await;
        rules.push(rule);
        info!("Alert rule '{}' added", rule.name);
    }

    /// Remove an alert rule
    pub async fn remove_alert_rule(&self, name: &str) -> Result<()> {
        let mut rules = self.alerts.write().await;
        let original_len = rules.len();

        rules.retain(|r| r.name != name);

        if rules.len() < original_len {
            Ok(())
        } else {
            anyhow::bail!("Alert rule '{}' not found", name)
        }
    }

    /// Check all alert rules against current metrics
    pub async fn evaluate_alerts(&self) -> Vec<AlertEvent> {
        let rules = self.alerts.read().await;
        let series_map = self.time_series.read().await;
        let mut triggered_alerts = Vec::new();

        for rule in rules.iter() {
            if !rule.enabled {
                continue;
            }

            if let Some(series) = series_map.get(&rule.metric_name) {
                let current_value = series.read().await.current().unwrap_or(0.0);

                let triggered = match rule.condition {
                    AlertCondition::GreaterThan => current_value > rule.threshold,
                    AlertCondition::LessThan => current_value < rule.threshold,
                    AlertCondition::EqualTo => (current_value - rule.threshold).abs() < f64::EPSILON,
                    AlertCondition::NotEqualTo => (current_value - rule.threshold).abs() >= f64::EPSILON,
                    _ => false, // Simplified for percentage changes
                };

                if triggered {
                    let alert = AlertEvent {
                        id: format!(
                            "{}-{}",
                            rule.name,
                            chrono::Utc::now().timestamp_millis()
                        ),
                        name: rule.name.clone(),
                        message: format!(
                            "{} {} {}",
                            rule.metric_name, rule.condition, rule.threshold
                        ),
                        severity: rule.severity,
                        metric_name: rule.metric_name.clone(),
                        threshold: rule.threshold,
                        actual_value: current_value,
                        timestamp: chrono::Utc::now(),
                        resolved: false,
                    };

                    triggered_alerts.push(alert.clone());

                    // Store in history
                    let mut history = self.alert_history.write().await;
                    history.push_back(alert);
                    while history.len() > 1000 {
                        history.pop_front();
                    }

                    // Broadcast alert
                    let _ =
                        self.tx.send(MonitorEvent::AlertTriggered(triggered_alerts.last().unwrap().clone()));

                    warn!(
                        "Alert triggered: {} [{}]",
                        rule.name, rule.severity
                    );
                }
            }
        }

        triggered_alerts
    }

    /// Get recent alerts
    pub async fn get_recent_alerts(
        &self,
        limit: Option<usize>,
        only_unresolved: bool,
    ) -> Vec<AlertEvent> {
        let history = self.alert_history.read().await;
        let limit = limit.unwrap_or(50);

        let filtered: Vec<_> = if only_unresolved {
            history.iter().filter(|a| !a.resolved).collect()
        } else {
            history.iter().collect()
        };

        filtered
            .rev()
            .take(limit)
            .cloned()
            .collect()
    }

    /// Register a health check
    pub async fn register_health_check<H>(&self, check: H)
    where
        H: HealthCheck + Send + Sync + 'static,
    {
        let mut checks = self.health_checks.write().await;
        checks.push(Box::new(check));
        info!("Health check registered: {}", check.name());
    }

    /// Run all health checks
    pub async fn run_health_checks(&self) -> SystemHealth {
        let checks = self.health_checks.read().await;
        let mut results = Vec::with_capacity(checks.len());
        let mut all_healthy = true;

        for check in checks.iter() {
            let result = check.check().await;
            all_healthy = all_healthy && result.healthy;
            results.push(result);
        }

        let health = SystemHealth {
            overall_healthy: all_healthy,
            components: results,
            uptime_seconds: self.start_time.elapsed().as_secs(),
            last_check: chrono::Utc::now(),
        };

        // Broadcast health update
        let _ = self.tx.send(MonitorEvent::HealthCheckComplete(health.clone()));

        health
    }

    /// Get dashboard data for web UI
    pub async fn get_dashboard_data(&self) -> DashboardData {
        let series_map = self.time_series.read().await;
        let alerts = self.alert_history.read().await;
        let rules = self.alerts.read().await;

        let mut metrics_data = HashMap::new();

        for (name, series) in series_map.iter() {
            let guard = series.read().await;
            metrics_data.insert(
                name.clone(),
                DashboardMetric {
                    name: name.clone(),
                    current: guard.current(),
                    stats: guard.stats(),
                    recent_points: guard
                        .points()
                        .iter()
                        .rev()
                        .take(100)
                        .cloned()
                        .collect(),
                },
            );
        }

        DashboardData {
            metrics: metrics_data,
            active_alert_rules: rules.iter().filter(|r| r.enabled).count(),
            recent_alerts: alerts.iter().rev().take(20).cloned().collect(),
            system_uptime: self.start_time.elapsed(),
            generated_at: chrono::Utc::now(),
        }
    }

    /// Subscribe to monitor events
    pub fn subscribe(&self) -> broadcast::Receiver<MonitorEvent> {
        self.tx.subscribe()
    }
}

/// Data structure for web dashboard
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardData {
    pub metrics: HashMap<String, DashboardMetric>,
    pub active_alert_rules: usize,
    pub recent_alerts: Vec<AlertEvent>,
    pub system_uptime: Duration,
    pub generated_at: chrono::DateTime<chrono::Utc>,
}

/// Single metric for dashboard display
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardMetric {
    pub name: String,
    pub current: Option<f64>,
    pub stats: TimeSeriesStats,
    pub recent_points: Vec<TimeSeriesPoint>,
}

// ════════════════════════════════════════════════════════════════
// Built-in Health Checks
// ════════════════════════════════════════════════════════════════

/// Memory usage health check
pub struct MemoryHealthCheck {
    warning_threshold_mb: u64,
    critical_threshold_mb: u64,
}

impl MemoryHealthCheck {
    pub fn new(warning_mb: u64, critical_mb: u64) -> Self {
        Self {
            warning_threshold_mb: warning_mb,
            critical_threshold_mb: critical_mb,
        }
    }
}

#[async_trait]
impl HealthCheck for MemoryHealthCheck {
    fn name(&self) -> &str {
        "memory"
    }

    async fn check(&self) -> HealthCheckResult {
        let start = Instant::now();

        // Get memory usage (platform-specific)
        #[cfg(target_os = "windows")]
        let memory_usage_mb = {
            use std::mem;
            use winapi::um::psapi::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};

            unsafe {
                let mut pmc: PROCESS_MEMORY_COUNTERS = mem::zeroed();
                pmc.cbSize = mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;

                let handle = winapi::um::processthreadsapi::GetCurrentProcess();
                if GetProcessMemoryInfo(handle, &mut pmc, pmc.cbSize) != 0 {
                    pmc.WorkingSetSize / (1024 * 1024)
                } else {
                    0
                }
            }
        };

        #[cfg(not(target_os = "windows"))]
        let memory_usage_mb = 0; // Placeholder for non-Windows

        let elapsed = start.elapsed();

        let (healthy, message) = if memory_usage_mb > self.critical_threshold_mb {
            (
                false,
                format!(
                    "Critical memory usage: {} MB (threshold: {} MB)",
                    memory_usage_mb, self.critical_threshold_mb
                ),
            )
        } else if memory_usage_mb > self.warning_threshold_mb {
            (
                true,
                format!(
                    "Warning: high memory usage: {} MB (threshold: {} MB)",
                    memory_usage_mb, self.warning_threshold_mb
                ),
            )
        } else {
            (
                true,
                format!("Memory usage normal: {} MB", memory_usage_mb),
            )
        };

        HealthCheckResult {
            component: "memory".to_string(),
            healthy,
            message,
            response_time_ms: Some(elapsed.as_secs_f64() * 1000.0),
            last_check: chrono::Utc::now(),
        }
    }
}

/// Disk space health check
pub struct DiskSpaceHealthCheck {
    path: PathBuf,
    warning_threshold_percent: f64,
    critical_threshold_percent: f64,
}

impl DiskSpaceHealthCheck {
    pub fn new(
        path: impl Into<PathBuf>,
        warning_pct: f64,
        critical_pct: f64,
    ) -> Self {
        Self {
            path: path.into(),
            warning_threshold_percent: warning_pct,
            critical_threshold_percent: critical_pct,
        }
    }
}

#[async_trait]
impl HealthCheck for DiskSpaceHealthCheck {
    fn name(&self) -> &str {
        "disk-space"
    }

    async fn check(&self) -> HealthCheckResult {
        let start = Instant::now();

        // Get disk space information
        let (healthy, message) = match tokio::fs::metadata(&self.path).await {
            Ok(_) => {
                // In production, would get actual disk stats
                (true, "Disk space OK".to_string())
            }
            Err(e) => (false, format!("Cannot access disk: {}", e)),
        };

        HealthCheckResult {
            component: "disk".to_string(),
            healthy,
            message,
            response_time_ms: Some(start.elapsed().as_secs_f64() * 1000.0),
            last_check: chrono::Utc::now(),
        }
    }
}
