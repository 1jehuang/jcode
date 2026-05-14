use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::session::{SshSession, SshConfig, SshOutput};
use super::config::{ConfigParser, SshHostConfig};
use super::tunnel::{PortForwarder, TunnelManager, JumpHostChain};
use super::transfer::FileTransfer;
use super::pool::SshConnectionPool;
use super::audit::{SshAuditLogger, SshAuditEvent, SshEventType, AuditDetails, EventSeverity};

/// Enhanced SSH Manager - Central management for all SSH operations
pub struct SshManager {
    sessions: Arc<Mutex<HashMap<String, SshSession>>>,
    pool: Arc<SshConnectionPool>,
    tunnel_manager: Arc<Mutex<TunnelManager>>,
    audit_logger: Arc<SshAuditLogger>,
    config_cache: Arc<Mutex<HashMap<String, SshHostConfig>>>,
}

impl SshManager {
    /// Create new SSH manager instance
    pub fn new() -> Self {
        let pool = Arc::new(SshConnectionPool::new(10));
        let audit_logger = Arc::new(SshAuditLogger::new());

        SshManager {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            pool,
            tunnel_manager: Arc::new(Mutex::new(TunnelManager::new())),
            audit_logger,
            config_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Connect to a host and return session ID
    pub fn connect(&self, host: &str) -> Result<String, String> {
        // Try to load from ~/.ssh/config first
        let config = if let Some(host_config) = self._get_cached_config(host) {
            SshConfig::from_host_config(&host_config.host)?
        } else if let Ok(Some(host_config)) = ConfigParser::find_host(host) {
            // Cache the config
            if let Ok(mut cache) = self.config_cache.lock() {
                cache.insert(host.to_string(), host_config.clone());
            }
            SshConfig::from_host_config(&host_config.host)?
        } else {
            SshConfig::with_host(host)
        };

        let mut session = SshSession::new(config);
        
        // Log connection attempt
        self.audit_logger.log_connection_attempt(
            &session.config().user,
            &session.config().host,
            session.id()
        );

        match session.connect() {
            Ok(msg) => {
                let id = session.id().to_string();
                
                // Store session
                if let Ok(mut sessions) = self.sessions.lock() {
                    sessions.insert(id.clone(), session);
                }

                // Log success
                self.audit_logger.log_connection_success(
                    &session.config().user,
                    &session.config().host,
                    &id
                );

                Ok(msg)
            }
            Err(e) => {
                // Log failure
                self.audit_logger.log_connection_failure(
                    &config.user,
                    &config.host,
                    &e
                );
                
                Err(e)
            }
        }
    }

    /// Execute command on connected host (auto-manages connection)
    pub fn execute(&self, host: &str, command: &str) -> Result<SshOutput, String> {
        let start = std::time::Instant::now();
        
        // Use connection pool for better resource management
        let result = self.pool.execute_on(host, |session| {
            session.execute(command)
        });

        let duration = start.elapsed();

        // Log command execution
        if let Ok(ref output) = result {
            self.audit_logger.log_command_execution(
                "",  // Session ID not available in pool context
                command,
                output.exit_code,
                duration.as_millis() as u64,
                if output.stdout.len() > 1024 { 
                    Some(&format!("{}... (truncated)", &output.stdout[..1024])) 
                } else { 
                    Some(&output.stdout) 
                },
            );
        }

        result
    }

    /// Execute command with streaming output
    pub fn execute_streaming<F>(&self, host: &str, command: &str, on_line: F) -> Result<(), String>
    where
        F: FnMut(&str) + Send + 'static,
    {
        // Get or create session
        let session_id = self.pool.get_session(host)?;
        
        // Get session reference
        let sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        let session = sessions.values()
            .find(|s| s.id() == session_id)
            .ok_or("Session not found")?;

        session.execute_streaming(command, on_line)
    }

    /// Upload file to remote host
    pub fn upload_file(&self, host: &str, local_path: &Path, remote_path: &Path) -> Result<u64, String> {
        let transfer = self._create_transfer(host)?;
        
        let file_size = std::fs::metadata(local_path)
            .map(|m| m.len())
            .unwrap_or(0);

        let result = transfer.upload_file(local_path, remote_path)?;

        // Log file upload
        self.audit_logger.log_file_transfer(
            "",
            true,
            &local_path.to_path_buf(),
            file_size,
            result.success,
            result.error.as_deref(),
        );

        if result.success {
            Ok(result.bytes_transferred)
        } else {
            Err(result.error.unwrap_or("Upload failed".to_string()))
        }
    }

    /// Download file from remote host
    pub fn download_file(&self, host: &str, remote_path: &Path, local_path: &Path) -> Result<u64, String> {
        let transfer = self._create_transfer(host)?;
        
        let result = transfer.download_file(remote_path, local_path);

        // Get actual file size after download
        let downloaded_size = std::fs::metadata(local_path)
            .map(|m| m.len())
            .unwrap_or(0);

        match result {
            Ok(r) => {
                // Log file download
                self.audit_logger.log_file_transfer(
                    "",
                    false,
                    &remote_path.to_path_buf(),
                    r.bytes_transferred,
                    r.success,
                    r.error.as_deref(),
                );

                if r.success { Ok(r.bytes_transferred) } 
                else { Err(r.error.unwrap_or("Download failed".to_string())) }
            }
            Err(e) => Err(e),
        }
    }

    /// Upload directory recursively
    pub fn upload_directory(&self, host: &str, local_dir: &Path, remote_dir: &Path) -> Result<Vec<String>, String> {
        let transfer = self._create_transfer(host)?;
        let results = transfer.upload_directory(local_dir, remote_dir, None)?;
        
        Ok(results.into_iter().filter_map(|r| {
            if !r.success {
                Some(r.error.unwrap_or("Unknown error".to_string()))
            } else {
                None
            }
        }).collect())
    }

    /// Download directory recursively
    pub fn download_directory(&self, host: &str, remote_dir: &Path, local_dir: &Path) -> Result<Vec<String>, String> {
        let transfer = self._create_transfer(host)?;
        let results = transfer.download_directory(remote_dir, local_dir, None)?;
        
        Ok(results.into_iter().filter_map(|r| {
            if !r.success {
                Some(r.error.unwrap_or("Unknown error".to_string()))
            } else {
                None
            }
        }).collect())
    }

    /// Create port forwarding tunnel
    pub fn create_tunnel(&self, tunnel: PortForwarder) -> Result<String, String> {
        let mut manager = self.tunnel_manager.lock().map_err(|e| e.to_string())?;
        manager.add_tunnel(tunnel)
    }

    /// Remove tunnel by ID
    pub fn remove_tunnel(&self, id: &str) -> Result<(), String> {
        let mut manager = self.tunnel_manager.lock().map_err(|e| e.to_string())?;
        manager.remove_tunnel(id)
    }

    /// List active tunnels
    pub fn list_tunnels(&self) -> Vec<super::tunnel::TunnelInfo> {
        let manager = self.tunnel_manager.lock().unwrap_or_else(|e| e.into_inner());
        manager.list_tunnels()
    }

    /// Disconnect specific session
    pub fn disconnect(&self, session_id: &str) -> Result<String, String> {
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        
        if let Some(mut session) = sessions.remove(session_id) {
            let msg = session.disconnect()?;
            
            // Log disconnection
            self.audit_logger.log(SshAuditEvent {
                id: uuid::Uuid::new_v4().to_string(),
                timestamp: chrono::Utc::now(),
                event_type: SshEventType::Disconnection,
                session_id: Some(session_id.to_string()),
                user: Some(session.config().user.clone()),
                host: Some(session.config().host.clone()),
                details: AuditDetails {
                    description: "Session disconnected".to_string(),
                    ..Default::default()
                },
                severity: EventSeverity::Info,
                source_ip: None,
                metadata: None,
            });

            Ok(msg)
        } else {
            Err(format!("Session '{}' not found", session_id))
        }
    }

    /// Shutdown all connections and cleanup resources
    pub fn shutdown(&self) -> Result<(), String> {
        // Close all tunnels
        if let Ok(mut manager) = self.tunnel_manager.lock() {
            let _ = manager.stop_all();
        }

        // Shutdown connection pool
        let _ = self.pool.shutdown();

        // Disconnect all sessions
        if let Ok(mut sessions) = self.sessions.lock() {
            for (_, mut session) in sessions.drain() {
                let _ = session.disconnect();
            }
        }

        // Export final audit log
        let _ = self.audit_logger.export_to_json(
            &PathBuf::from("ssh_audit_log.json")
        );

        Ok(())
    }

    /// List all configured hosts from ~/.ssh/config
    pub fn list_hosts(&self) -> Result<Vec<String>, String> {
        ConfigParser::list_hosts()
    }

    /// Validate SSH configuration
    pub fn validate_config(&self) -> Result<Vec<String>, String> {
        ConfigParser::validate_config()
    }

    /// Get audit statistics
    pub fn get_audit_stats(&self) -> super::audit::AuditStatistics {
        self.audit_logger.get_statistics()
    }

    /// Get recent audit events
    pub fn get_recent_events(&self, limit: usize) -> Vec<SshAuditEvent> {
        self.audit_logger.get_recent_events(limit)
    }

    /// Export audit log to file
    pub fn export_audit_log(&self, path: &PathBuf) -> Result<(), String> {
        self.audit_logger.export_to_json(path)
    }

    /// Get pool statistics
    pub fn get_pool_stats(&self) -> super::pool::PoolStats {
        self.pool.stats()
    }

    // Private helper methods

    fn _get_cached_config(&self, host: &str) -> Option<SshHostConfig> {
        self.config_cache.lock().ok()?.get(host).cloned()
    }

    fn _create_transfer(&self, host: &str) -> Result<FileTransfer, String> {
        // Try to get user/host info from cached config or create default
        let (user, ssh_host, port) = if let Some(config) = self._get_cached_config(host) {
            (
                config.user.unwrap_or_else(|| "user".to_string()),
                config.host_name.unwrap_or_else(|| host.to_string()),
                config.port.unwrap_or(22),
            )
        } else {
            ("user".to_string(), host.to_string(), 22)
        };

        let mut transfer = FileTransfer::new(&user, &ssh_host, port);

        // Add identity file if available
        if let Some(config) = self._get_cached_config(host) {
            if !config.identity_file.is_empty() {
                transfer = transfer.with_identity(&config.identity_file[0]);
            }
        }

        Ok(transfer)
    }
}

impl Default for SshManager {
    fn default() -> Self {
        Self::new()
    }
}

/// SSH Command Builder - Fluent API for building complex SSH commands
pub struct SshCommandBuilder {
    target: String,
    command: String,
    args: Vec<String>,
    env_vars: HashMap<String, String>,
    timeout: Option<Duration>,
    working_dir: Option<PathBuf>,
    request_pty: bool,
    background: bool,
}

impl SshCommandBuilder {
    /// Start building a new SSH command
    pub fn new(target: &str, command: &str) -> Self {
        SshCommandBuilder {
            target: target.to_string(),
            command: command.to_string(),
            args: vec![],
            env_vars: HashMap::new(),
            timeout: None,
            working_dir: None,
            request_pty: false,
            background: false,
        }
    }

    /// Add argument to command
    pub fn arg(mut self, arg: &str) -> Self {
        self.args.push(arg.to_string());
        self
    }

    /// Add multiple arguments
    pub fn args(mut self, args: &[&str]) -> Self {
        self.args.extend(args.iter().map(|s| s.to_string()));
        self
    }

    /// Set environment variable
    pub fn env(mut self, key: &str, value: &str) -> Self {
        self.env_vars.insert(key.to_string(), value.to_string());
        self
    }

    /// Set execution timeout
    pub fn timeout(mut self, duration: Duration) -> Self {
        self.timeout = Some(duration);
        self
    }

    /// Set working directory on remote
    pub fn working_dir(mut self, dir: &Path) -> Self {
        self.working_dir = Some(dir.to_path_buf());
        self
    }

    /// Request pseudo-terminal allocation
    pub fn pty(mut self) -> Self {
        self.request_pty = true;
        self
    }

    /// Run command in background
    pub fn background(mut self) -> Self {
        self.background = true;
        self
    }

    /// Build the final command string
    pub fn build(self) -> String {
        let mut cmd_parts = vec![];

        // Change to working directory if specified
        if let Some(ref dir) = self.working_dir {
            cmd_parts.push(format!("cd {}", dir.display()));
        }

        // Set environment variables
        for (key, value) in &self.env_vars {
            cmd_parts.push(format!("export {}=\"{}\"", key, value));
        }

        // Add main command with arguments
        cmd_parts.push(self.command);
        cmd_parts.extend(self.args);

        // Join all parts
        let full_cmd = cmd_parts.join(" && ");

        // Wrap in nohup if background
        if self.background {
            format!("nohup {} > /dev/null 2>&1 &", full_cmd)
        } else {
            full_cmd
        }
    }
}

/// Batch Operations Executor - Execute commands across multiple hosts
pub struct BatchExecutor {
    manager: Arc<SshManager>,
    results: Arc<Mutex<Vec<BatchResult>>>,
}

#[derive(Debug, Clone)]
pub struct BatchResult {
    pub host: String,
    pub command: String,
    pub success: bool,
    pub output: Option<SshOutput>,
    pub duration: Duration,
    pub error: Option<String>,
}

impl BatchExecutor {
    pub fn new(manager: Arc<SshManager>) -> Self {
        BatchExecutor {
            manager,
            results: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Execute command on multiple hosts sequentially
    pub fn execute_sequential(&self, hosts: &[&str], command: &str) -> Vec<BatchResult> {
        hosts.iter()
            .map(|&host| {
                let start = std::time::Instant::now();
                
                match self.manager.execute(host, command) {
                    Ok(output) => {
                        let duration = start.elapsed();
                        BatchResult {
                            host: host.to_string(),
                            command: command.to_string(),
                            success: true,
                            output: Some(output),
                            duration,
                            error: None,
                        }
                    }
                    Err(e) => {
                        let duration = start.elapsed();
                        BatchResult {
                            host: host.to_string(),
                            command: command.to_string(),
                            success: false,
                            output: None,
                            duration,
                            error: Some(e),
                        }
                    }
                }
            })
            .collect()
    }

    /// Execute command on multiple hosts in parallel using connection pool
    pub fn execute_parallel(&self, hosts: Vec<&str>, command: &str) -> Vec<BatchResult> {
        use std::thread;

        let (tx, rx) = std::sync::mpsc::channel();
        let manager = self.manager.clone();

        for host in hosts {
            let tx = tx.clone();
            let mgr = manager.clone();
            let cmd = command.to_string();
            let host_str = host.to_string();

            thread::spawn(move || {
                let start = std::time::Instant::now();
                
                let result = match mgr.execute(&host_str, &cmd) {
                    Ok(output) => BatchResult {
                        host: host_str.clone(),
                        command: cmd.clone(),
                        success: true,
                        output: Some(output),
                        duration: start.elapsed(),
                        error: None,
                    },
                    Err(e) => BatchResult {
                        host: host_str.clone(),
                        command: cmd.clone(),
                        success: false,
                        output: None,
                        duration: start.elapsed(),
                        error: Some(e),
                    },
                };

                let _ = tx.send(result);
            });
        }

        drop(tx); // Drop sender so receiver knows when done

        rx.into_iter().collect()
    }

    /// Get summary of batch execution
    pub fn get_summary(&self, results: &[BatchResult]) -> BatchSummary {
        let total = results.len();
        let successful = results.iter().filter(|r| r.success).count();
        let failed = total - successful;

        let avg_duration = if total > 0 {
            let total_duration: Duration = results.iter().map(|r| r.duration).sum();
            total_duration / total as u32
        } else {
            Duration::ZERO
        };

        BatchSummary {
            total_commands: total,
            successful,
            failed,
            success_rate: if total > 0 { successful as f64 / total as f64 } else { 0.0 },
            average_duration: avg_duration,
            errors: results.iter()
                .filter_map(|r| r.error.clone())
                .collect(),
        }
    }
}

#[derive(Debug)]
pub struct BatchSummary {
    pub total_commands: usize,
    pub successful: usize,
    pub failed: usize,
    pub success_rate: f64,
    pub average_duration: Duration,
    pub errors: Vec<String>,
}
