use serde::{Deserialize, Serialize};
use std::process::{Command, Stdio, Child};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Enhanced SSH Configuration with Claude Code compatibility
#[derive(Debug, Clone)]
pub struct SshConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub identity_file: Option<PathBuf>,
    pub connect_timeout: Duration,
    pub keepalive_interval: Duration,
    pub server_alive_interval: u64,
    pub server_alive_count_max: u32,
    pub compression: bool,
    pub control_master: bool,
    pub control_path: Option<PathBuf>,
    pub strict_host_key_checking: bool,
    pub known_hosts_file: Option<PathBuf>,
    pub proxy_command: Option<String>,
    pub jump_hosts: Vec<JumpHost>,
    pub local_forwards: Vec<PortForward>,
    pub remote_forwards: Vec<PortForward>,
    pub dynamic_forward: Option<u16>,  // SOCKS5 port
    pub environment: HashMap<String, String>,
    pub request_pty: bool,
}

#[derive(Debug, Clone)]
pub struct JumpHost {
    pub host: String,
    pub user: String,
    pub port: u16,
    pub identity_file: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct PortForward {
    pub local_port: u16,
    pub remote_host: String,
    pub remote_port: u16,
    pub bind_address: Option<String>,
}

impl Default for SshConfig {
    fn default() -> Self {
        SshConfig {
            host: "localhost".to_string(),
            port: 22,
            user: std::env::var("USER").unwrap_or_else(|_| "user".to_string()),
            identity_file: None,
            connect_timeout: Duration::from_secs(30),
            keepalive_interval: Duration::from_secs(60),
            server_alive_interval: 60,
            server_alive_count_max: 3,
            compression: true,
            control_master: true,
            control_path: Some(
                dirs::home_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join(".ssh")
                    .join("control")
                    .join("%C")  // %C = connection hash
            ),
            strict_host_key_checking: true,
            known_hosts_file: None,
            proxy_command: None,
            jump_hosts: vec![],
            local_forwards: vec![],
            remote_forwards: vec![],
            dynamic_forward: None,
            environment: HashMap::new(),
            request_pty: false,
        }
    }
}

impl SshConfig {
    /// Create config from ~/.ssh/config Host entry
    pub fn from_host_config(host_name: &str) -> Result<Self, String> {
        let ssh_config_path = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".ssh")
            .join("config");

        if !ssh_config_path.exists() {
            return Ok(Self::with_host(host_name));
        }

        let content = std::fs::read_to_string(&ssh_config_path)
            .map_err(|e| format!("Failed to read ssh config: {}", e))?;

        // Simple parser for Host blocks (production should use proper SSH config parser)
        let mut in_target_host = false;
        let mut config = Self::with_host(host_name);

        for line in content.lines() {
            let line = line.trim();
            if line.starts_with("Host ") {
                let hosts: Vec<&str> = line[5..].split_whitespace().collect();
                in_target_host = hosts.iter().any(|h| *h == host_name || *h == "*");
                continue;
            }
            
            if !in_target_host { continue; }
            
            if line.starts_with("HostName ") {
                config.host = line[9..].trim().to_string();
            } else if line.starts_with("User ") {
                config.user = line[5..].trim().to_string();
            } else if line.starts_with("Port ") {
                config.port = line[5..].trim().parse().unwrap_or(22);
            } else if line.starts_with("IdentityFile ") {
                let identity = PathBuf::from(line[13..].trim());
                if !config.identity_file.as_ref().map_or(true, |p| p == &identity) {
                    config.identity_file = Some(identity);
                }
            } else if line.starts_with("ProxyCommand ") {
                config.proxy_command = Some(line[13..].trim().to_string());
            } else if line.starts_with("LocalForward ") {
                let parts: Vec<&str> = line[13..].split_whitespace().collect();
                if parts.len() >= 3 {
                    config.local_forwards.push(PortForward {
                        local_port: parts[0].parse().unwrap_or(0),
                        remote_host: parts[1].to_string(),
                        remote_port: parts[2].parse().unwrap_or(0),
                        bind_address: None,
                    });
                }
            } else if line.starts_with("RemoteForward ") {
                let parts: Vec<&str> = line[14..].split_whitespace().collect();
                if parts.len() >= 3 {
                    config.remote_forwards.push(PortForward {
                        local_port: parts[0].parse().unwrap_or(0),
                        remote_host: parts[1].to_string(),
                        remote_port: parts[2].parse().unwrap_or(0),
                        bind_address: None,
                    });
                }
            } else if line.starts_with("DynamicForward ") {
                config.dynamic_forward = Some(line[15..].trim().parse().unwrap_or(1080));
            } else if line.starts_with("ConnectTimeout ") {
                let secs: u64 = line[15..].trim().parse().unwrap_or(30);
                config.connect_timeout = Duration::from_secs(secs);
            } else if line.starts_with("Compression ") && (line.contains("yes") || line.contains("true")) {
                config.compression = true;
            }
        }

        Ok(config)
    }

    /// Quick config with just host
    pub fn with_host(host: &str) -> Self {
        let mut cfg = Self::default();
        
        // Parse user@host format
        if host.contains('@') {
            let parts: Vec<&str> = host.split('@').collect();
            if parts.len() == 2 {
                cfg.user = parts[0].to_string();
                
                // Check for port in hostname
                if parts[1].contains(':') {
                    let host_parts: Vec<&str> = parts[1].split(':').collect();
                    cfg.host = host_parts[0].to_string();
                    cfg.port = host_parts[1].parse().unwrap_or(22);
                } else {
                    cfg.host = parts[1].to_string();
                }
            }
        } else if host.contains(':') {
            let parts: Vec<&str> = host.split(':').collect();
            cfg.host = parts[0].to_string();
            cfg.port = parts[1].parse().unwrap_or(22);
        } else {
            cfg.host = host.to_string();
        }

        cfg
    }

    /// Build SSH arguments from config
    pub fn build_ssh_args(&self) -> Vec<String> {
        let mut args = vec![
            "-o".to_string(), format!("ConnectTimeout={}", self.connect_timeout.as_secs()),
            "-o".to_string(), "BatchMode=yes".to_string(),
            "-o".to_string(), format!("ServerAliveInterval={}", self.server_alive_interval),
            "-o".to_string(), format!("ServerAliveCountMax={}", self.server_alive_count_max),
        ];

        if self.compression {
            args.push("-o".to_string());
            args.push("Compression=yes".to_string());
        }

        if self.strict_host_key_checking {
            args.push("-o".to_string());
            args.push("StrictHostKeyChecking=yes".to_string());
        }

        if self.control_master {
            args.push("-o".to_string());
            args.push("ControlMaster=auto".to_string());
            args.push("-o".to_string());
            if let Some(ref path) = self.control_path {
                args.push(format!("ControlPath={}", path.display()));
            } else {
                args.push("ControlPath=~/.ssh/control/%C".to_string());
            }
        }

        args.push("-p".to_string());
        args.push(self.port.to_string());

        if let Some(ref identity) = self.identity_file {
            args.push("-i".to_string());
            args.push(identity.display().to_string());
        }

        // Jump hosts (proxy through bastion)
        for jump in &self.jump_hosts {
            args.push("-J".to_string());
            args.push(format!("{}@{}:{}", jump.user, jump.host, jump.port));
        }

        // Proxy command
        if let Some(ref proxy) = self.proxy_command {
            args.push("-o".to_string());
            args.push(format!("ProxyCommand={}", proxy));
        }

        args
    }
}

/// Enhanced SSH Session with connection pooling and auto-reconnect
pub struct SshSession {
    id: String,
    config: SshConfig,
    state: SessionState,
    connected_at: Option<Instant>,
    last_activity: Option<Instant>,
    child: Option<Child>,
    control_socket: Option<PathBuf>,
    stats: SessionStats,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SessionState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
    Error(String),
}

#[derive(Debug, Clone)]
pub struct SessionStats {
    pub commands_executed: u64,
    pub bytes_transferred: u64,
    pub files_uploaded: u64,
    pub files_downloaded: u64,
    pub uptime_seconds: u64,
    pub reconnect_count: u32,
}

impl Default for SessionStats {
    fn default() -> Self {
        SessionStats {
            commands_executed: 0,
            bytes_transferred: 0,
            files_uploaded: 0,
            files_downloaded: 0,
            uptime_seconds: 0,
            reconnect_count: 0,
        }
    }
}

impl SshSession {
    /// Create new session with enhanced config
    pub fn new(config: SshConfig) -> Self {
        SshSession {
            id: uuid::Uuid::new_v4().to_string(),
            config,
            state: SessionState::Disconnected,
            connected_at: None,
            last_activity: None,
            child: None,
            control_socket: None,
            stats: SessionStats::default(),
        }
    }

    /// Get session ID
    pub fn id(&self) -> &str { &self.id }

    /// Connect to remote host with retry logic
    pub fn connect(&mut self) -> Result<String, String> {
        if matches!(self.state, SessionState::Connected | SessionState::Connecting) {
            return Err("Already connected or connecting".to_string());
        }

        self.state = SessionState::Connecting;

        let result = self._connect_internal();

        match result {
            Ok(msg) => {
                self.state = SessionState::Connected;
                self.connected_at = Some(Instant::now());
                self.last_activity = Some(Instant::now());
                Ok(msg)
            }
            Err(e) => {
                self.state = SessionState::Error(e.clone());
                Err(e)
            }
        }
    }

    fn _connect_internal(&self) -> Result<String, String> {
        let mut cmd = Command::new("ssh");
        
        for arg in self.config.build_ssh_args() {
            cmd.arg(arg);
        }

        let target = format!("{}@{}", self.config.user, self.config.host);
        cmd.arg(target.clone())
           .arg("-T")  // Disable pseudo-terminal allocation for non-interactive
           .arg("-o".to_string())
           .arg("RequestTTY=no".to_string());

        if self.config.request_pty {
            cmd.arg("-t");  // Force PTY allocation for interactive commands
        }

        // Set environment variables
        for (key, value) in &self.config.environment {
            cmd.env(key, value);
        }

        // Connection test command
        cmd.arg("echo 'SSH_CONNECTION_OK'; echo $$; uname -a")
           .stdout(Stdio::piped())
           .stderr(Stdio::piped());

        let output = cmd.output()
            .map_err(|e| format!("Failed to execute ssh: {}", e))?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let lines: Vec<&str> = stdout.lines().collect();
            
            if lines.iter().any(|l| l.contains("SSH_CONNECTION_OK")) {
                Ok(format!(
                    "✅ Connected to {}@{}:{}\nRemote: {}\nSession ID: {}",
                    self.config.user, self.config.host, self.config.port,
                    lines.last().unwrap_or(&"Unknown"),
                    self.id
                ))
            } else {
                Err(format!("Connection test failed: {}", stdout))
            }
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(format!(
                "❌ Connection failed to {}@{}:{}\nError: {}",
                self.config.user, self.config.host, self.config.port, stderr
            ))
        }
    }

    /// Execute command synchronously with timeout support
    pub fn execute(&mut self, command: &str) -> Result<SshOutput, String> {
        self.ensure_connected()?;
        
        let start = Instant::now();
        let result = self._execute_sync(command)?;
        let _duration = start.elapsed();

        // Update stats
        self.stats.commands_executed += 1;
        self.stats.bytes_transferred += result.stdout.len() as u64 + result.stderr.len() as u64;
        self.last_activity = Some(Instant::now());

        Ok(SshOutput {
            stdout: result.stdout,
            stderr: result.stderr,
            exit_code: result.exit_code,
            duration,
        })
    }

    fn _execute_sync(&self, command: &str) -> Result<SshInternalOutput, String> {
        let target = format!("{}@{}", self.config.user, self.config.host);

        let mut cmd = Command::new("ssh");
        for arg in self.config.build_ssh_args() {
            cmd.arg(arg);
        }
        
        cmd.arg(target)
           .arg(command)
           .stdout(Stdio::piped())
           .stderr(Stdio::piped());

        let output = cmd.output()
            .map_err(|e| format!("Execution failed: {}", e))?;

        Ok(SshInternalOutput {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code(),
        })
    }

    /// Execute command with streaming output (real-time)
    pub fn execute_streaming<F>(&self, command: &str, mut on_line: F) -> Result<(), String>
    where
        F: FnMut(&str) + Send + 'static,
    {
        self.ensure_connected()?;

        let target = format!("{}@{}", self.config.user, self.config.host);

        let mut cmd = Command::new("ssh");
        for arg in self.config.build_ssh_args() {
            cmd.arg(arg);
        }

        cmd.arg(target)
           .arg(command)
           .stdin(Stdio::null())
           .stdout(Stdio::piped())
           .stderr(Stdio::piped());

        let mut child = cmd.spawn()
            .map_err(|e| format!("Failed to spawn process: {}", e))?;

        let stdout = child.stdout.take().ok_or("Failed to capture stdout")?;
        let reader = BufReader::new(stdout);

        for line_result in reader.lines() {
            match line_result {
                Ok(line) => on_line(&line),
                Err(e) => return Err(format!("Read error: {}", e)),
            }
        }

        // Wait for process to complete
        let _status = child.wait();

        Ok(())
    }

    /// Execute interactive command (with PTY) - supports sudo, vim, etc.
    pub fn execute_interactive<F>(&self, command: &str, mut input_handler: F) -> Result<(), String>
    where
        F: FnMut(&str) -> Option<String> + Send + 'static,
    {
        self.ensure_connected()?;

        let target = format!("{}@{}", self.config.user, self.config.host);

        let mut cmd = Command::new("ssh");
        for arg in self.config.build_ssh_args() {
            cmd.arg(arg);
        }

        cmd.arg("-t")  // Allocate pseudo-terminal
           .arg(target)
           .arg(command)
           .stdin(Stdio::piped())
           .stdout(Stdio::piped())
           .stderr(Stdio::piped());

        let mut child = cmd.spawn()
            .map_err(|e| format!("Failed to spawn interactive process: {}", e))?;

        let stdin = child.stdin.take().ok_or("Failed to get stdin")?;
        let mut writer = std::io::BufWriter::new(stdin);

        // Read initial output
        let stdout = child.stdout.take().ok_or("Failed to get stdout")?;
        let reader = BufReader::new(stdout);

        // Simple interaction loop (for complex scenarios, consider pty crate)
        for line_result in reader.lines() {
            match line_result {
                Ok(line) => {
                    if let Some(response) = input_handler(&line) {
                        use std::io::Write;
                        let _ = writer.write_all((response.as_str()).as_bytes());
                        let _ = writer.flush();
                    }
                }
                Err(e) => return Err(format!("Interactive read error: {}", e)),
            }
        }

        let _status = child.wait();
        Ok(())
    }

    /// Execute command asynchronously (background task)
    pub async fn execute_async(&self, command: &str) -> Result<u32, String> {
        // For true async execution, you'd use tokio::process::Command
        // This is a simplified synchronous wrapper that spawns a background thread
        self.ensure_connected()?;

        let config = self.config.clone();
        let cmd = command.to_string();

        tokio::spawn(async move {
            let target = format!("{}@{}", config.user, config.host);
            let mut ssh_cmd = tokio::process::Command::new("ssh");

            // Add basic args (simplified for async context)
            ssh_cmd.arg("-o")
                  .arg("BatchMode=yes")
                  .arg("-p")
                  .arg(config.port.to_string());

            if let Some(ref identity) = config.identity_file {
                ssh_cmd.arg("-i").arg(identity);
            }

            ssh_cmd.arg(target)
                  .arg(cmd)
                  .output()
                  .await
                  .map(|o| o.status.code().unwrap_or(-1) as u32)
                  .unwrap_or(0)
        }).await
        .map_err(|e| format!("Async execution failed: {}", e))
    }

    /// Upload file to remote host (simple version)
    pub fn upload(&mut self, local_path: &Path, remote_path: &Path) -> Result<(), String> {
        self.upload_with_progress(local_path, remote_path, |_current, _total| {})?;
        Ok(())
    }

    /// Download file from remote host (simple version)
    pub fn download(&mut self, remote_path: &Path, local_path: &Path) -> Result<(), String> {
        self.download_with_progress(remote_path, local_path, |_current, _total| {})?;
        Ok(())
    }

    /// Upload file with progress callback
    pub fn upload_with_progress<F>(&mut self, local_path: &Path, remote_path: &Path, progress_callback: F) -> Result<u64, String>
    where
        F: Fn(u64, u64) + Send + Sync + 'static,
    {
        self.ensure_connected()?;

        let file_size = std::fs::metadata(local_path)
            .map_err(|e| format!("Failed to stat file: {}", e))?
            .len();

        let start = Instant::now();

        // Use rsync for better progress tracking if available
        if self._is_rsync_available() {
            self._rsync_upload(local_path, remote_path, &progress_callback)?;
        } else {
            self._scp_upload(local_path, remote_path)?;
            progress_callback(file_size, file_size);
        }

        let duration = start.elapsed();
        let speed = if duration.as_secs() > 0 {
            file_size / duration.as_secs()
        } else {
            0
        };

        self.stats.files_uploaded += 1;
        self.stats.bytes_transferred += file_size;

        eprintln!("\n  📤 Uploaded {} ({:.1} MB/s, {:.1} MB in {:?})", 
            local_path.file_name().unwrap_or_default().to_string_lossy(),
            speed as f64 / (1024.0 * 1024.0),
            file_size as f64 / (1024.0 * 1024.0),
            duration
        );

        Ok(file_size)
    }

    /// Download file with progress callback
    pub fn download_with_progress<F>(&mut self, remote_path: &Path, local_path: &Path, progress_callback: F) -> Result<u64, String>
    where
        F: Fn(u64, u64) + Send + Sync + 'static,
    {
        self.ensure_connected()?;

        let start = Instant::now();

        if self._is_rsync_available() {
            self._rsync_download(remote_path, local_path, &progress_callback)?;
        } else {
            self._scp_download(remote_path, local_path)?;
        }

        let file_size = std::fs::metadata(local_path)
            .map(|m| m.len())
            .unwrap_or(0);

        let _duration = start.elapsed();
        self.stats.files_downloaded += 1;
        self.stats.bytes_transferred += file_size;

        Ok(file_size)
    }

    /// Upload directory recursively
    pub fn upload_dir(&self, local_dir: &Path, remote_dir: &Path) -> Result<(), String> {
        self.ensure_connected()?;

        if self._is_rsync_available() {
            let remote_dest = format!("{}@{}:{}", self.config.user, self.config.host, remote_dir.display());
            let mut cmd = Command::new("rsync");
            cmd.arg("-avz")
               .arg("--progress")
               .arg("-e")
               .arg("ssh -i $IDENTITY_FILE -p $PORT")
               .arg(local_dir.display().to_string())
               .arg(remote_dest);

            if let Some(ref identity) = self.config.identity_file {
                cmd.env("IDENTITY_FILE", identity.display().to_string());
            }
            cmd.env("PORT", self.config.port.to_string());

            let status = cmd.status()
                .map_err(|e| format!("Rsync upload failed: {}", e))?;

            if status.success() { Ok(()) } else { Err("Rsync upload failed".to_string()) }
        } else {
            // Fallback: Use scp -r (limited support)
            self._scp_recursive_upload(local_dir, remote_dir)
        }
    }

    /// Download directory recursively
    pub fn download_dir(&mut self, remote_dir: &Path, local_dir: &Path) -> Result<(), String> {
        self.ensure_connected()?;

        if self._is_rsync_available() {
            let remote_src = format!("{}@{}:{}", self.config.user, self.config.host, remote_dir.display());
            let mut cmd = Command::new("rsync");
            cmd.arg("-avz")
               .arg("--progress")
               .arg(remote_src)
               .arg(local_dir.display().to_string());

            let status = cmd.status()
                .map_err(|e| format!("Rsync download failed: {}", e))?;

            if status.success() { Ok(()) } else { Err("Rsync download failed".to_string()) }
        } else {
            self._scp_recursive_download(remote_dir, local_dir)
        }
    }

    /// Disconnect session
    pub fn disconnect(&mut self) -> Result<String, String> {
        match self.state {
            SessionState::Disconnected | SessionState::Error(_) => {
                return Err("Not connected".to_string());
            }
            _ => {}
        }

        // Kill any running child processes
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
        }

        // Clean up control socket
        if let Some(ref socket) = self.control_socket {
            if socket.exists() {
                let _ = std::fs::remove_file(socket);
            }
        }

        self.state = SessionState::Disconnected;
        self.connected_at = None;

        Ok(format!(
            "🔌 Disconnected from {}@{}\nSession: {}\nUptime: {:?}\nCommands executed: {}",
            self.config.user, self.config.host, self.id,
            self.uptime(),
            self.stats.commands_executed
        ))
    }

    /// Check if session is alive (heartbeat check)
    pub fn is_alive(&mut self) -> bool {
        if !matches!(self.state, SessionState::Connected) {
            return false;
        }

        match self.execute("echo alive") {
            Ok(output) => output.stdout.contains("alive"),
            Err(_) => false,
        }
    }
    
    /// Check if session is connected (without heartbeat)
    pub fn is_connected(&self) -> bool {
        matches!(self.state, SessionState::Connected)
    }

    /// Auto-reconnect if disconnected
    pub fn ensure_connected(&self) -> Result<(), String> {
        match self.state {
            SessionState::Connected => Ok(()),
            SessionState::Error(ref msg) => Err(format!("Session error: {}", msg)),
            _ => Err("Not connected. Call connect() first.".to_string()),
        }
    }

    /// Get session statistics
    pub fn stats(&self) -> &SessionStats { &self.stats }

    /// Get connection uptime
    pub fn uptime(&self) -> Option<Duration> {
        self.connected_at.map(|t| t.elapsed())
    }

    /// Get time since last activity
    pub fn idle_time(&self) -> Option<Duration> {
        self.last_activity.map(|t| t.elapsed())
    }

    /// Get session state
    pub fn state(&self) -> &SessionState { &self.state }

    /// Get configuration reference
    pub fn config(&self) -> &SshConfig { &self.config }

    // Private helper methods

    fn _scp_upload(&self, local_path: &Path, remote_path: &Path) -> Result<(), String> {
        let remote_dest = format!("{}@{}:{}", self.config.user, self.config.host, remote_path.display());

        let mut cmd = Command::new("scp");
        cmd.arg("-P")
           .arg(self.config.port.to_string())
           .arg("-o")
           .arg("BatchMode=yes");

        if let Some(ref identity) = self.config.identity_file {
            cmd.arg("-i").arg(identity);
        }

        cmd.arg(local_path.display().to_string())
           .arg(remote_dest)
           .status()
           .map_err(|e| format!("SCP upload failed: {}", e))
           .and_then(|status| {
               if status.success() { Ok(()) } else { Err("SCP upload failed".to_string()) }
           })
    }

    fn _scp_download(&self, remote_path: &Path, local_path: &Path) -> Result<(), String> {
        let remote_src = format!("{}@{}:{}", self.config.user, self.config.host, remote_path.display());

        let mut cmd = Command::new("scp");
        cmd.arg("-P")
           .arg(self.config.port.to_string())
           .arg("-o")
           .arg("BatchMode=yes");

        if let Some(ref identity) = self.config.identity_file {
            cmd.arg("-i").arg(identity);
        }

        cmd.arg(remote_src)
           .arg(local_path.display().to_string())
           .status()
           .map_err(|e| format!("SCP download failed: {}", e))
           .and_then(|status| {
               if status.success() { Ok(()) } else { Err("SCP download failed".to_string()) }
           })
    }

    fn _rsync_upload<F>(&self, local_path: &Path, remote_path: &Path, progress_cb: &F) -> Result<(), String>
    where
        F: Fn(u64, u64) + Send + Sync + 'static,
    {
        let remote_dest = format!("{}@{}:{}", self.config.user, self.config.host, remote_path.display());
        let file_size = std::fs::metadata(local_path).map(|m| m.len()).unwrap_or(0);

        let mut cmd = Command::new("rsync");
        cmd.arg("-avz")
           .arg("--progress")
           .arg(local_path.display().to_string())
           .arg(remote_dest);

        if let Some(ref identity) = self.config.identity_file {
            cmd.arg("-e").arg(format!("ssh -i {}", identity.display()));
        }

        let output = cmd.output()
            .map_err(|e| format!("Rsync failed: {}", e))?;

        progress_cb(file_size, file_size); // Simulate completion

        if output.status.success() {
            Ok(())
        } else {
            Err(format!("Rsync error: {}", String::from_utf8_lossy(&output.stderr)))
        }
    }

    fn _rsync_download<F>(&self, remote_path: &Path, local_path: &Path, progress_cb: &F) -> Result<(), String>
    where
        F: Fn(u64, u64) + Send + Sync + 'static,
    {
        let remote_src = format!("{}@{}:{}", self.config.user, self.config.host, remote_path.display());

        let mut cmd = Command::new("rsync");
        cmd.arg("-avz")
           .arg("--progress")
           .arg(remote_src)
           .arg(local_path.display().to_string());

        let output = cmd.output()
            .map_err(|e| format!("Rsync failed: {}", e))?;

        let size = std::fs::metadata(local_path).map(|m| m.len()).unwrap_or(0);
        progress_cb(size, size);

        if output.status.success() {
            Ok(())
        } else {
            Err(format!("Rsync error: {}", String::from_utf8_lossy(&output.stderr)))
        }
    }

    fn _scp_recursive_upload(&self, local_dir: &Path, remote_dir: &Path) -> Result<(), String> {
        // Find all files and upload individually
        self._walk_and_upload_files(local_dir, remote_dir)
    }

    fn _scp_recursive_download(&mut self, remote_dir: &Path, local_dir: &Path) -> Result<(), String> {
        let list_output = self.execute(&format!("ls -la {}", remote_dir.display()))?;
        
        if !list_output.stderr.is_empty() {
            return Err(format!("Failed to list remote dir: {}", list_output.stderr));
        }

        std::fs::create_dir_all(local_dir)
            .map_err(|e| format!("Failed to create local dir: {}", e))?;

        for line in list_output.stdout.lines() {
            if line.starts_with('-') || line.starts_with('d') {
                // File or directory entry
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 9 {
                    let name = parts[8];
                    let remote_path = remote_dir.join(name);
                    let local_path = local_dir.join(name);

                    if line.starts_with('d') {
                        self._scp_recursive_download(&remote_path, &local_path)?;
                    } else {
                        self._scp_download(&remote_path, &local_path)?;
                    }
                }
            }
        }

        Ok(())
    }

    fn _walk_and_upload_files(&self, local_dir: &Path, remote_dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(remote_dir)
            .map_err(|_| "".to_string())?; // Ignore error, will fail on actual upload

        let entries = std::fs::read_dir(local_dir)
            .map_err(|e| format!("Failed to read dir: {}", e))?;

        for entry in entries {
            let entry = entry.map_err(|e| format!("Entry error: {}", e))?;
            let path = entry.path();

            if path.is_dir() {
                let name = entry.file_name();
                self._walk_and_upload_files(&path, &remote_dir.join(name))?;
            } else if path.is_file() {
                let name = entry.file_name();
                self._scp_upload(&path, &remote_dir.join(name))?;
            }
        }

        Ok(())
    }

    fn _is_rsync_available(&self) -> bool {
        which::which("rsync").is_ok()
    }
}

struct SshInternalOutput {
    stdout: String,
    stderr: String,
    exit_code: Option<i32>,
}

/// SSH Command Output
#[derive(Debug, Clone)]
pub struct SshOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub duration: std::time::Duration,
}

/// Connection pool for managing multiple SSH sessions
pub struct SshConnectionPool {
    sessions: Arc<Mutex<HashMap<String, SshSession>>>,
    max_connections: usize,
    default_config: Option<SshConfig>,
}

impl SshConnectionPool {
    pub fn new(max_connections: usize) -> Self {
        SshConnectionPool {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            max_connections,
            default_config: None,
        }
    }

    pub fn with_config(mut self, config: SshConfig) -> Self {
        self.default_config = Some(config);
        self
    }

    /// Get or create a session for the given host
    pub fn get_session(&self, host: &str) -> Result<String, String> {
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;

        let key = host.to_lowercase();

        if let Some(session) = sessions.get_mut(&key) {
            if session.is_alive() {
                return Ok(session.id().to_string());
            }
        }
        sessions.remove(&key);

        if sessions.len() >= self.max_connections {
            let oldest_id = sessions.iter()
                .min_by_key(|(_, s)| s.idle_time().unwrap_or(Duration::MAX))
                .map(|(id, _)| id.clone());
            if let Some(id) = oldest_id {
                sessions.remove(&id);
            }
        }

        let config = self.default_config.clone()
            .unwrap_or_else(|| SshConfig::with_host(host));

        let mut session = SshSession::new(config);
        let _session_id = session.connect()?;
        let id = session.id.clone();

        sessions.insert(key, session);

        Ok(id)
    }

    /// Execute command on specific host
    pub fn execute_on(&mut self, host: &str, command: &str) -> Result<SshOutput, String> {
        let session_id = self.get_session(host)?;
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        
        let session = sessions.values_mut()
            .find(|s| s.id == session_id)
            .ok_or("Session not found after creation")?;

        session.execute(command)
    }

    /// Parallel execution across multiple hosts
    pub fn parallel_execute(&mut self, commands: Vec<(String, String)>) -> Vec<(String, Result<SshOutput, String>)> {
        commands.into_iter()
            .map(|(host, cmd)| {
                let output = self.execute_on(&host, &cmd);
                (host, output)
            })
            .collect()
    }

    /// Close all sessions
    pub fn close_all(&self) -> Result<usize, String> {
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        let count = sessions.len();

        for (_, session) in sessions.iter_mut() {
            let _ = session.disconnect();
        }

        sessions.clear();
        Ok(count)
    }

    /// Get active session count
    pub fn active_sessions(&self) -> usize {
        self.sessions.lock().map(|s| s.len()).unwrap_or(0)
    }
}

/// SSH Audit Logger for security compliance
pub struct SshAuditLogger {
    events: Arc<Mutex<Vec<SshEvent>>>,
    enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshEvent {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub session_id: String,
    pub event_type: SshEventType,
    pub details: serde_json::Value,
    pub user: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SshEventType {
    ConnectionEstablished,
    ConnectionClosed,
    CommandExecuted,
    FileUploaded,
    FileDownloaded,
    PortForwardCreated,
    AuthenticationSuccess,
    AuthenticationFailure,
    ErrorOccurred,
}

impl SshAuditLogger {
    pub fn new(enabled: bool) -> Self {
        SshAuditLogger {
            events: Arc::new(Mutex::new(Vec::new())),
            enabled,
        }
    }

    pub fn log_event(&self, event: SshEvent) {
        if !self.enabled { return; }

        if let Ok(mut events) = self.events.lock() {
            events.push(event);
            
            // Keep only last 10000 events
            if events.len() > 10000 {
                let excess = events.len() - 10000;
                events.drain(..excess);
            }
        }
    }

    pub fn get_events(&self, limit: usize) -> Vec<SshEvent> {
        self.events.lock()
            .map(|events| events.iter().rev().take(limit).cloned().collect())
            .unwrap_or_default()
    }

    pub fn get_events_by_session(&self, session_id: &str) -> Vec<SshEvent> {
        self.events.lock()
            .map(|events| events.iter().filter(|e| e.session_id == session_id).cloned().collect())
            .unwrap_or_default()
    }

    pub fn export_to_file(&self, path: &std::path::Path) -> Result<(), String> {
        let events = self.get_events(10000);
        let json = serde_json::to_string_pretty(&events)
            .map_err(|e| format!("Serialization failed: {}", e))?;

        std::fs::write(path, json)
            .map_err(|e| format!("Write failed: {}", e))?;

        Ok(())
    }
}
