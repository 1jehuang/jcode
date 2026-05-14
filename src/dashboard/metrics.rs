use std::collections::HashMap;
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemMetrics {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub system: SystemInfo,
    pub tasks: TaskMetrics,
    pub plugins: PluginMetrics,
    pub sessions: SessionMetrics,
    pub performance: PerformanceMetrics,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemInfo {
    pub cpu_usage_percent: f64,
    pub memory_used_mb: u64,
    pub memory_total_mb: u64,
    pub memory_usage_percent: f64,
    pub disk_used_gb: f64,
    pub disk_total_gb: f64,
    pub disk_usage_percent: f64,
    pub load_average_1m: f64,
    pub load_average_5m: f64,
    pub load_average_15m: f64,
}

impl Default for SystemInfo {
    fn default() -> Self {
        SystemInfo {
            cpu_usage_percent: 0.0,
            memory_used_mb: 0,
            memory_total_mb: 0,
            memory_usage_percent: 0.0,
            disk_used_gb: 0.0,
            disk_total_gb: 0.0,
            disk_usage_percent: 0.0,
            load_average_1m: 0.0,
            load_average_5m: 0.0,
            load_average_15m: 0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskMetrics {
    pub total_tasks: usize,
    pub todo_count: usize,
    pub in_progress_count: usize,
    pub done_count: usize,
    pub cancelled_count: usize,
    pub high_priority_count: usize,
    pub average_completion_time_secs: f64,
}

impl Default for TaskMetrics {
    fn default() -> Self {
        TaskMetrics {
            total_tasks: 0,
            todo_count: 0,
            in_progress_count: 0,
            done_count: 0,
            cancelled_count: 0,
            high_priority_count: 0,
            average_completion_time_secs: 0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginMetrics {
    pub total_plugins: usize,
    pub enabled_plugins: usize,
    pub disabled_plugins: usize,
    pub plugins_with_errors: usize,
    pub plugin_list: Vec<PluginStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginStatus {
    pub name: String,
    pub version: String,
    pub status: String,
    pub capabilities: Vec<String>,
}

impl Default for PluginMetrics {
    fn default() -> Self {
        PluginMetrics {
            total_plugins: 0,
            enabled_plugins: 0,
            disabled_plugins: 0,
            plugins_with_errors: 0,
            plugin_list: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetrics {
    pub active_sessions: usize,
    pub total_sessions_today: usize,
    pub average_session_duration_secs: f64,
    pub messages_per_session_avg: f64,
}

impl Default for SessionMetrics {
    fn default() -> Self {
        SessionMetrics {
            active_sessions: 0,
            total_sessions_today: 0,
            average_session_duration_secs: 0.0,
            messages_per_session_avg: 0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceMetrics {
    pub requests_per_second: f64,
    pub average_response_time_ms: f64,
    pub p50_response_time_ms: f64,
    pub p95_response_time_ms: f64,
    pub p99_response_time_ms: f64,
    pub error_rate_percent: f64,
    pub uptime_seconds: u64,
}

impl Default for PerformanceMetrics {
    fn default() -> Self {
        PerformanceMetrics {
            requests_per_second: 0.0,
            average_response_time_ms: 0.0,
            p50_response_time_ms: 0.0,
            p95_response_time_ms: 0.0,
            p99_response_time_ms: 0.0,
            error_rate_percent: 0.0,
            uptime_seconds: 0,
        }
    }
}

impl SystemMetrics {
    pub fn new() -> Self {
        SystemMetrics {
            timestamp: chrono::Utc::now(),
            system: SystemInfo::default(),
            tasks: TaskMetrics::default(),
            plugins: PluginMetrics::default(),
            sessions: SessionMetrics::default(),
            performance: PerformanceMetrics::default(),
        }
    }

    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self)
            .map_err(|e| format!("Serialization error: {}", e))
    }
}
