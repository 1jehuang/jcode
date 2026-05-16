use std::sync::{Arc, Mutex};
use std::path::PathBuf;
use std::fs;
use std::io::Write;
use std::collections::HashMap;
use chrono::{DateTime, Utc};
use serde::{Serialize, Deserialize};

/// SSH Event Types for audit logging
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SshEventType {
    ConnectionAttempt,
    ConnectionSuccess,
    ConnectionFailure,
    Disconnection,
    AuthenticationSuccess,
    AuthenticationFailure,
    CommandExecutionStart,
    CommandExecutionComplete,
    FileUploadStart,
    FileUploadComplete,
    FileDownloadStart,
    FileDownloadComplete,
    PortForwardCreated,
    PortForwardRemoved,
    SessionError,
    SecurityViolation,
    ConfigChange,
    Custom(String),
}

/// SSH Audit Event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshAuditEvent {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub event_type: SshEventType,
    pub session_id: Option<String>,
    pub user: Option<String>,
    pub host: Option<String>,
    pub details: AuditDetails,
    pub severity: EventSeverity,
    pub source_ip: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

/// Detailed event information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditDetails {
    pub description: String,
    pub command: Option<String>,
    pub file_path: Option<PathBuf>,
    pub file_size: Option<u64>,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<u64>,
    pub bytes_transferred: Option<u64>,
    pub error_message: Option<String>,
    pub additional_info: HashMap<String, String>,
}

impl Default for AuditDetails {
    fn default() -> Self {
        AuditDetails {
            description: String::new(),
            command: None,
            file_path: None,
            file_size: None,
            exit_code: None,
            duration_ms: None,
            bytes_transferred: None,
            error_message: None,
            additional_info: HashMap::new(),
        }
    }
}

/// Event Severity Levels
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, PartialOrd, Ord, Eq)]
pub enum EventSeverity {
    Info,
    Warning,
    Error,
    Critical,
}

impl Default for EventSeverity {
    fn default() -> Self { EventSeverity::Info }
}

/// Audit Logger Configuration
#[derive(Debug, Clone)]
pub struct AuditConfig {
    pub enabled: bool,
    pub max_events_in_memory: usize,
    pub log_to_file: bool,
    pub log_file_path: Option<PathBuf>,
    pub log_format: LogFormat,
    pub include_command_output: bool,
    pub retention_days: u32,
}

#[derive(Debug, Clone, Copy)]
pub enum LogFormat {
    Json,
    Text,
}

impl Default for AuditConfig {
    fn default() -> Self {
        AuditConfig {
            enabled: true,
            max_events_in_memory: 10000,
            log_to_file: false,
            log_file_path: None,
            log_format: LogFormat::Json,
            include_command_output: false,
            retention_days: 30,
        }
    }
}

/// Enhanced SSH Audit Logger
pub struct SshAuditLogger {
    config: AuditConfig,
    events: Arc<Mutex<Vec<SshAuditEvent>>>,
    file_handle: Arc<Mutex<Option<fs::File>>>,
}

impl SshAuditLogger {
    /// Create new audit logger with default configuration
    pub fn new() -> Self {
        Self::with_config(AuditConfig::default())
    }

    /// Create audit logger with custom configuration
    pub fn with_config(config: AuditConfig) -> Self {
        let logger = SshAuditLogger {
            config: config.clone(),
            events: Arc::new(Mutex::new(Vec::new())),
            file_handle: Arc::new(Mutex::new(None)),
        };

        // Initialize file logging if configured
        if config.log_to_file && config.log_file_path.is_some() {
            if let Err(e) = logger._init_file_logging() {
                eprintln!("Warning: Failed to initialize file logging: {}", e);
            }
        }

        logger
    }

    /// Log a new audit event
    pub fn log(&self, event: SshAuditEvent) {
        if !self.config.enabled {
            return;
        }

        // Add to in-memory store
        {
            if let Ok(mut events) = self.events.lock() {
                events.push(event.clone());

                // Enforce maximum size limit (FIFO eviction)
                while events.len() > self.config.max_events_in_memory {
                    events.remove(0);
                }
            }
        }

        // Write to file if configured
        if self.config.log_to_file {
            if let Err(e) = self._write_event_to_file(&event) {
                eprintln!("Warning: Failed to write audit event to file: {}", e);
            }
        }
    }

    /// Create and log a connection attempt event
    pub fn log_connection_attempt(&self, user: &str, host: &str, session_id: &str) {
        self.log(SshAuditEvent {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            event_type: SshEventType::ConnectionAttempt,
            session_id: Some(session_id.to_string()),
            user: Some(user.to_string()),
            host: Some(host.to_string()),
            details: AuditDetails {
                description: format!("Connection attempt to {}@{}", user, host),
                ..Default::default()
            },
            severity: EventSeverity::Info,
            source_ip: None,
            metadata: None,
        });
    }

    /// Create and log connection success event
    pub fn log_connection_success(&self, user: &str, host: &str, session_id: &str) {
        self.log(SshAuditEvent {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            event_type: SshEventType::ConnectionSuccess,
            session_id: Some(session_id.to_string()),
            user: Some(user.to_string()),
            host: Some(host.to_string()),
            details: AuditDetails {
                description: format!("Successfully connected to {}@{}", user, host),
                ..Default::default()
            },
            severity: EventSeverity::Info,
            source_ip: None,
            metadata: None,
        });
    }

    /// Create and log connection failure event
    pub fn log_connection_failure(&self, user: &str, host: &str, error: &str) {
        self.log(SshAuditEvent {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            event_type: SshEventType::ConnectionFailure,
            session_id: None,
            user: Some(user.to_string()),
            host: Some(host.to_string()),
            details: AuditDetails {
                description: format!("Failed to connect to {}@{}", user, host),
                error_message: Some(error.to_string()),
                ..Default::default()
            },
            severity: EventSeverity::Error,
            source_ip: None,
            metadata: None,
        });
    }

    /// Create and log command execution event
    pub fn log_command_execution(
        &self,
        session_id: &str,
        command: &str,
        exit_code: Option<i32>,
        duration_ms: u64,
        output_truncated: Option<&str>,
    ) {
        let mut details = AuditDetails {
            description: format!("Command executed: {}", command),
            command: Some(command.to_string()),
            exit_code,
            duration_ms: Some(duration_ms),
            ..Default::default()
        };

        if self.config.include_command_output {
            if let Some(output) = output_truncated {
                details.additional_info.insert("output".to_string(), output.to_string());
            }
        }

        self.log(SshAuditEvent {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            event_type: if exit_code.map_or(true, |c| c == 0) {
                SshEventType::CommandExecutionComplete
            } else {
                SshEventType::CommandExecutionComplete  // We use same type but check exit code
            },
            session_id: Some(session_id.to_string()),
            user: None,
            host: None,
            details,
            severity: if exit_code.map_or(false, |c| c != 0) { 
                EventSeverity::Warning 
            } else { 
                EventSeverity::Info 
            },
            source_ip: None,
            metadata: None,
        });
    }

    /// Create and log file transfer event
    pub fn log_file_transfer(
        &self,
        session_id: &str,
        is_upload: bool,
        file_path: &PathBuf,
        file_size: u64,
        success: bool,
        error_msg: Option<&str>,
    ) {
        self.log(SshAuditEvent {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            event_type: if is_upload {
                if success { SshEventType::FileUploadComplete } else { SshEventType::FileUploadStart }
            } else {
                if success { SshEventType::FileDownloadComplete } else { SshEventType::FileDownloadStart }
            },
            session_id: Some(session_id.to_string()),
            user: None,
            host: None,
            details: AuditDetails {
                description: format!(
                    "{} file: {} ({:.1} KB)",
                    if is_upload { "Upload" } else { "Download" },
                    file_path.display(),
                    file_size as f64 / 1024.0
                ),
                file_path: Some(file_path.clone()),
                file_size: Some(file_size),
                bytes_transferred: if success { Some(file_size) } else { None },
                error_message: error_msg.map(|s| s.to_string()),
                ..Default::default()
            },
            severity: if success { EventSeverity::Info } else { EventSeverity::Error },
            source_ip: None,
            metadata: None,
        });
    }

    /// Create and log security violation event
    pub fn log_security_violation(&self, session_id: &str, violation_type: &str, details: &str) {
        self.log(SshAuditEvent {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            event_type: SshEventType::SecurityViolation,
            session_id: Some(session_id.to_string()),
            user: None,
            host: None,
            details: AuditDetails {
                description: format!("Security violation: {}", violation_type),
                additional_info: {
                    let mut map = HashMap::new();
                    map.insert("violation_type".to_string(), violation_type.to_string());
                    map.insert("details".to_string(), details.to_string());
                    map
                },
                ..Default::default()
            },
            severity: EventSeverity::Critical,
            source_ip: None,
            metadata: None,
        });
    }

    /// Get recent events (most recent first)
    pub fn get_recent_events(&self, limit: usize) -> Vec<SshAuditEvent> {
        self.events.lock()
            .map(|events| {
                events.iter()
                    .rev()
                    .take(limit)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get events by session ID
    pub fn get_events_by_session(&self, session_id: &str) -> Vec<SshAuditEvent> {
        self.events.lock()
            .map(|events| {
                events.iter()
                    .filter(|e| e.session_id.as_deref() == Some(session_id))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get events by type
    pub fn get_events_by_type(&self, event_type: &SshEventType) -> Vec<SshAuditEvent> {
        self.events.lock()
            .map(|events| {
                events.iter()
                    .filter(|e| &e.event_type == event_type)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get events by severity level or higher
    pub fn get_events_by_severity(&self, min_severity: EventSeverity) -> Vec<SshAuditEvent> {
        self.events.lock()
            .map(|events| {
                events.iter()
                    .filter(|e| e.severity >= min_severity)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Export all events to JSON file
    pub fn export_to_json(&self, path: &PathBuf) -> Result<(), String> {
        let events = self.get_recent_events(self.config.max_events_in_memory);
        
        let json = serde_json::to_string_pretty(&events)
            .map_err(|e| format!("JSON serialization failed: {}", e))?;

        fs::write(path, json)
            .map_err(|e| format!("Failed to write export file: {}", e))?;

        Ok(())
    }

    /// Export events to CSV format
    pub fn export_to_csv(&self, path: &PathBuf) -> Result<(), String> {
        let events = self.get_recent_events(self.config.max_events_in_memory);

        let mut csv_content = String::from("id,timestamp,event_type,session_id,user,host,severity,description\n");

        for event in &events {
            csv_content.push_str(&format!(
                "{},{},{},{},{},{},{},\"{}\"\n",
                event.id,
                event.timestamp.format("%Y-%m-%d %H:%M:%S UTC"),
                match &event.event_type {
                    SshEventType::Custom(s) => s.clone(),
                    other => format!("{:?}", other),
                },
                event.session_id.as_deref().unwrap_or(""),
                event.user.as_deref().unwrap_or(""),
                event.host.as_deref().unwrap_or(""),
                match event.severity {
                    EventSeverity::Info => "INFO",
                    EventSeverity::Warning => "WARNING",
                    EventSeverity::Error => "ERROR",
                    EventSeverity::Critical => "CRITICAL",
                },
                event.details.description.replace('"', "\"\"")
            ));
        }

        fs::write(path, csv_content)
            .map_err(|e| format!("Failed to write CSV file: {}", e))?;

        Ok(())
    }

    /// Clear all events from memory
    pub fn clear_events(&self) {
        if let Ok(mut events) = self.events.lock() {
            events.clear();
        }
    }

    /// Get event count by type (for statistics)
    pub fn get_statistics(&self) -> AuditStatistics {
        let events = match self.events.lock() {
            Ok(e) => e.clone(),
            Err(_) => return AuditStatistics::default(),
        };

        let mut stats = AuditStatistics::default();

        for event in &events {
            stats.total_events += 1;

            match event.event_type {
                SshEventType::ConnectionAttempt | SshEventType::ConnectionSuccess => stats.connections += 1,
                SshEventType::ConnectionFailure => stats.connection_failures += 1,
                SshEventType::CommandExecutionStart | SshEventType::CommandExecutionComplete => stats.commands_executed += 1,
                SshEventType::FileUploadStart | SshEventType::FileUploadComplete => stats.file_uploads += 1,
                SshEventType::FileDownloadStart | SshEventType::FileDownloadComplete => stats.file_downloads += 1,
                SshEventType::AuthenticationFailure => stats.auth_failures += 1,
                SshEventType::SecurityViolation => stats.security_violations += 1,
                _ => {}
            }

            match event.severity {
                EventSeverity::Critical => stats.critical_count += 1,
                EventSeverity::Error => stats.error_count += 1,
                EventSeverity::Warning => stats.warning_count += 1,
                EventSeverity::Info => stats.info_count += 1,
            }
        }

        stats
    }

    // Private helper methods

    fn _init_file_logging(&self) -> Result<(), String> {
        let path = self.config.log_file_path.as_ref()
            .ok_or("No log file path configured")?;

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create log directory: {}", e))?;
        }

        // Open/create file in append mode
        let file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| format!("Failed to open log file: {}", e))?;

        if let Ok(mut handle) = self.file_handle.lock() {
            *handle = Some(file);
        }

        Ok(())
    }

    fn _write_event_to_file(&self, event: &SshAuditEvent) -> Result<(), String> {
        let line = match self.config.log_format {
            LogFormat::Json => {
                serde_json::to_string(event)
                    .map_err(|e| format!("JSON serialization failed: {}", e))?
            }
            LogFormat::Text => {
                let event_type_str = match &event.event_type {
                    SshEventType::Custom(s) => s.as_str(),
                    other => {
                        let formatted = format!("{:?}", other);
                        Box::leak(formatted.into_boxed_str())
                    }
                };
                format!(
                    "[{}] [{}] {} - {} | User: {} | Host: {} | {}",
                    event.timestamp.format("%Y-%m-%dT%H:%M:%SZ"),
                    match event.severity {
                        EventSeverity::Info => "INFO",
                        EventSeverity::Warning => "WARN",
                        EventSeverity::Error => "ERROR",
                        EventSeverity::Critical => "CRIT",
                    },
                    event_type_str,
                    event.details.description,
                    event.user.as_deref().unwrap_or("-"),
                    event.host.as_deref().unwrap_or("-"),
                    if let Some(ref err) = event.details.error_message {
                        format!("Error: {}", err)
                    } else {
                        String::new()
                    }
                )
            }
        };

        if let Ok(mut handle) = self.file_handle.lock() {
            if let Some(ref mut file) = *handle {
                writeln!(file, "{}", line)
                    .map_err(|e| format!("Write failed: {}", e))?;
                
                file.flush()
                    .map_err(|e| format!("Flush failed: {}", e))?;
            }
        }

        Ok(())
    }
}

/// Audit Statistics Summary
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuditStatistics {
    pub total_events: usize,
    pub connections: usize,
    pub connection_failures: usize,
    pub commands_executed: usize,
    pub file_uploads: usize,
    pub file_downloads: usize,
    pub auth_failures: usize,
    pub security_violations: usize,
    pub critical_count: usize,
    pub error_count: usize,
    pub warning_count: usize,
    pub info_count: usize,
}
