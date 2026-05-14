use std::collections::HashMap;
use serde::{Serialize, Deserialize};
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageEvent {
    pub id: String,
    pub event_type: EventType,
    pub timestamp: DateTime<Utc>,
    pub user_id: Option<String>,
    pub session_id: String,
    pub data: EventData,
    pub duration_ms: Option<u64>,
    pub success: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EventType {
    CommandExecution,
    FileEdit,
    TaskCreated,
    TaskCompleted,
    PluginInstalled,
    SSHConnection,
    SessionExport,
    Error,
    PerformanceMetric,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventData {
    pub command: Option<String>,
    pub file_path: Option<String>,
    pub lines_changed: Option<u32>,
    pub tokens_used: Option<u32>,
    pub custom: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregatedMetrics {
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
    pub total_events: u64,
    pub events_by_type: HashMap<String, u64>,
    pub average_duration_ms: f64,
    pub success_rate: f64,
    pub top_commands: Vec<(String, u64)>,
    pub peak_usage_hours: Vec<u8>,
}

pub struct DataCollector {
    events: Vec<UsageEvent>,
    max_events: usize,
    enabled: bool,
}

impl DataCollector {
    pub fn new(max_events: usize) -> Self {
        DataCollector {
            events: vec![],
            max_events,
            enabled: true,
        }
    }

    pub fn record(&mut self, event: UsageEvent) {
        if !self.enabled { return; }
        if self.events.len() >= self.max_events {
            self.events.remove(0);
        }
        self.events.push(event);
    }

    pub fn get_events(&self) -> &[UsageEvent] { &self.events }
    pub fn event_count(&self) -> usize { self.events.len() }

    pub fn aggregate_metrics(&self, hours: u64) -> AggregatedMetrics {
        let cutoff = Utc::now() - chrono::Duration::hours(hours as i64);
        let recent: Vec<_> = self.events.iter()
            .filter(|e| e.timestamp > cutoff)
            .collect();

        let mut events_by_type = HashMap::new();
        let mut command_counts = HashMap::new();
        let mut total_duration = 0u64;
        let mut success_count = 0u64;

        for event in &recent {
            *events_by_type.entry(format!("{:?}", event.event_type)).or_insert(0) += 1;
            if let Some(ref cmd) = event.data.command {
                *command_counts.entry(cmd.clone()).or_insert(0) += 1;
            }
            if let Some(dur) = event.duration_ms { total_duration += dur; }
            if event.success { success_count += 1; }
        }

        let mut top_commands: Vec<_> = command_counts.into_iter().collect();
        top_commands.sort_by(|a, b| b.1.cmp(&a.1));
        top_commands.truncate(10);

        AggregatedMetrics {
            period_start: cutoff,
            period_end: Utc::now(),
            total_events: recent.len() as u64,
            events_by_type,
            average_duration_ms: if !recent.is_empty() { total_duration as f64 / recent.len() as f64 } else { 0.0 },
            success_rate: if !recent.is_empty() { success_count as f64 / recent.len() as f64 } else { 1.0 },
            top_commands,
            peak_usage_hours: vec![],
        }
    }

    pub fn enable(&mut self) { self.enabled = true; }
    pub fn disable(&mut self) { self.enabled = false; }
}
