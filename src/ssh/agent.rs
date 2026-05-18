use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::env;
use std::time::Duration;

/// SSH Agent Forwarding Manager
/// 
/// Provides comprehensive SSH agent support including:
/// - Automatic SSH_AUTH_SOCK detection
/// - Agent identity querying and management
/// - Key forwarding configuration
/// - Agent request signing (delegation)
/// - Security audit logging

pub struct SshAgentManager {
    agent_socket: Option<PathBuf>,
    identities: Vec<AgentIdentity>,
    forwarding_enabled: bool,
    lifetime: Option<Duration>,
    confirm_before_use: bool,
}

#[derive(Debug, Clone)]
pub struct AgentIdentity {
    pub key_type: String,        // rsa, ed25519, ecdsa-sha2-nistp256, etc.
    pub fingerprint: String,     // SHA256 fingerprint
    pub comment: String,        // User-provided comment
    pub expires_at: Option<std::time::SystemTime>,
}

#[derive(Debug, Clone)]
pub enum AgentError {
    AgentNotFound {
        message: String,
    },
    ConnectionFailed {
        socket_path: PathBuf,
        message: String,
    },
    RequestFailed {
        operation: String,
        message: String,
    },
    NoIdentitiesAvailable,
    SigningFailed {
        key_fingerprint: String,
        message: String,
    },
    PermissionDenied {
        operation: String,
    },
    Timeout {
        operation: String,
    },
    InvalidResponse {
        message: String,
    },
}

impl std::fmt::Display for AgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentError::AgentNotFound { message } => {
                write!(f, "SSH agent not found: {}", message)
            }
            AgentError::ConnectionFailed { socket_path, message } => {
                write!(f, "Failed to connect to SSH agent at {}: {}", 
                    socket_path.display(), message)
            }
            AgentError::RequestFailed { operation, message } => {
                write!(f, "Agent request '{}' failed: {}", operation, message)
            }
            AgentError::NoIdentitiesAvailable => {
                write!(f, "No identities available in SSH agent")
            }
            AgentError::SigningFailed { key_fingerprint, message } => {
                write!(f, "Signing failed for key {}: {}", key_fingerprint, message)
            }
            AgentError::PermissionDenied { operation } => {
                write!(f, "Permission denied for agent operation: {}", operation)
            }
            AgentError::Timeout { operation } => {
                write!(f, "Agent operation timed out: {}", operation)
            }
            AgentError::InvalidResponse { message } => {
                write!(f, "Invalid agent response: {}", message)
            }
        }
    }
}

impl std::error::Error for AgentError {}

impl SshAgentManager {
    /// Create new SSH agent manager with automatic detection
    pub fn new() -> Self {
        let mut manager = SshAgentManager {
            agent_socket: None,
            identities: vec![],
            forwarding_enabled: true,
            lifetime: None,
            confirm_before_use: false,
        };

        let _ = manager.detect_agent();

        manager
    }

    /// Create with specific agent socket path
    pub fn with_socket(socket_path: PathBuf) -> Self {
        SshAgentManager {
            agent_socket: Some(socket_path),
            identities: vec![],
            forwarding_enabled: true,
            lifetime: None,
            confirm_before_use: false,
        }
    }

    /// Enable or disable agent forwarding globally
    pub fn set_forwarding(&mut self, enabled: bool) {
        self.forwarding_enabled = enabled;
    }

    /// Set session lifetime for forwarded agents
    pub fn set_lifetime(&mut self, lifetime: Duration) {
        self.lifetime = Some(lifetime);
    }

    /// Require user confirmation before using each identity
    pub fn set_confirm_required(&mut self, required: bool) {
        self.confirm_before_use = required;
    }

    // ==========================================
    // Agent Detection & Connection
    // ==========================================

    /// Detect SSH agent from environment variables
    pub fn detect_agent(&mut self) -> Result<PathBuf, AgentError> {
        // Check SSH_AUTH_SOCK first (standard location)
        if let Ok(sock_path) = env::var("SSH_AUTH_SOCK") {
            let path = PathBuf::from(sock_path);
            
            if self._is_agent_running(&path) {
                self.agent_socket = Some(path.clone());
                
                // Load available identities
                if let Err(e) = self.refresh_identities() {
                    eprintln!("[SSH-AGENT] Warning: Failed to load identities: {}", e);
                }

                return Ok(path);
            }
        }

        // Check common alternative locations
        let alternatives = [
            PathBuf::from("/tmp/ssh-XXXXXXXXXX/agent.XXXXXXXXXX"),  // Pattern
            PathBuf::from("/run/user/1000/agent"),  // Linux systemd
            PathBuf::from("/tmp/ssh-"),  // macOS pattern
        ];

        for pattern in alternatives.iter() {
            if let Some(found) = self._find_agent_socket(pattern) {
                self.agent_socket = Some(found.clone());
                let _ = self.refresh_identities();
                return Ok(found);
            }
        }

        Err(AgentError::AgentNotFound {
            message: "No running SSH agent found. Start ssh-agent first.".to_string(),
        })
    }

    /// Check if agent is responding at given socket
    pub fn is_agent_available(&self) -> bool {
        if let Some(ref socket) = self.agent_socket {
            return self._is_agent_running(socket);
        }
        false
    }

    /// Get current agent socket path
    pub fn socket_path(&self) -> Option<&PathBuf> {
        self.agent_socket.as_ref()
    }

    // ==========================================
    // Identity Management
    // ==========================================

    /// Query agent for all loaded identities
    pub fn refresh_identities(&mut self) -> Result<Vec<AgentIdentity>, AgentError> {
        let socket = self.agent_socket.as_ref()
            .ok_or_else(|| AgentError::AgentNotFound {
                message: "No agent socket configured".to_string()
            })?;

        let output = self._execute_ssh_add_command("-l", socket)?;

        if !output.status.success() && !output.stdout.is_empty() {
            // ssh-add -l returns non-zero when no identities, but outputs nothing
            // Check if it's actually an error or just "no identities"
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("error") || stderr.contains("failed") {
                return Err(AgentError::RequestFailed {
                    operation: "list_identities".to_string(),
                    message: stderr.to_string(),
                });
            }
        }

        // Parse identity list output
        let mut identities = vec![];
        let stdout = String::from_utf8_lossy(&output.stdout);

        for line in stdout.lines() {
            // Format: "256 SHA256:<fingerprint> <comment> (RSA)"
            // Or: "<bits> <fingerprint> <comment> (<type>)"
            if let Some(identity) = self._parse_identity_line(line.trim()) {
                identities.push(identity);
            }
        }

        self.identities = identities.clone();
        Ok(identities)
    }

    /// Get cached identities without refreshing
    pub fn get_identities(&self) -> &[AgentIdentity] {
        &self.identities
    }

    /// Add private key to agent
    pub fn add_identity(&self, key_path: &Path, lifetime: Option<Duration>) -> Result<(), AgentError> {
        let socket = self.agent_socket.as_ref()
            .ok_or_else(|| AgentError::AgentNotFound {
                message: "No agent configured".to_string()
            })?;

        let mut args = vec![];
        
        if let Some(lt) = lifetime.or(self.lifetime) {
            args.push(format!("-t{}", lt.as_secs()));
        }

        if self.confirm_before_use {
            args.push("-c".to_string());  // Confirm before use
        }

        let output = self._execute_ssh_add_command_with_args(
            key_path.display().to_string(), 
            &args, 
            socket
        )?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(AgentError::RequestFailed {
                operation: "add_identity".to_string(),
                message: format!("Failed to add key '{}': {}", 
                    key_path.display(), stderr),
            })
        } else {
            // Refresh identities after adding
            // Note: This requires &mut self, so caller should call refresh separately
            Ok(())
        }
    }

    /// Remove specific identity from agent by fingerprint
    pub fn remove_identity(&self, fingerprint: &str) -> Result<(), AgentError> {
        let socket = self.agent_socket.as_ref()
            .ok_or_else(|| AgentError::AgentNotFound {
                message: "No agent configured".to_string()
            })?;

        let output = self._execute_ssh_add_command(
            &format!("-d {}", fingerprint), 
            socket
        )?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(AgentError::RequestFailed {
                operation: "remove_identity".to_string(),
                message: stderr.to_string(),
            })
        } else {
            Ok(())
        }
    }

    /// Remove all identities from agent
    pub fn remove_all_identities(&self) -> Result<(), AgentError> {
        let socket = self.agent_socket.as_ref()
            .ok_or_else(|| AgentError::AgentNotFound {
                message: "No agent configured".to_string()
            })?;

        let output = self._execute_ssh_add_command("-D", socket)?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(AgentError::RequestFailed {
                operation: "remove_all_identities".to_string(),
                message: stderr.to_string(),
            })
        } else {
            Ok(())
        }
    }

    /// Lock agent with password (requires confirmation for future use)
    pub fn lock_agent(&self, password: &str, lifetime: Option<Duration>) -> Result<(), AgentError> {
        let socket = self.agent_socket.as_ref()
            .ok_or_else(|| AgentError::AgentNotFound {
                message: "No agent configured".to_string()
            })?;

        let mut cmd_args = format!("{} -x", password);

        if let Some(lt) = lifetime.or(self.lifetime) {
            cmd_args.push_str(&format!(" -t{}", lt.as_secs()));
        }

        let output = self._execute_ssh_add_command(&cmd_args, socket)?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(AgentError::RequestFailed {
                operation: "lock_agent".to_string(),
                message: stderr.to_string(),
            })
        } else {
            Ok(())
        }
    }

    // ==========================================
    // Agent Signing Operations
    // ==========================================

    /// Sign data with a specific key from the agent
    pub fn sign_data(&self, key_fingerprint: &str, data: &[u8]) -> Result<Vec<u8>, AgentError> {
        // This would require direct communication with the agent protocol
        // For now, we use ssh-keygen as a workaround
        let temp_file = env::temp_dir().join("carpai_ssh_sign_input");
        
        // Write data to temp file
        std::fs::write(&temp_file, data)
            .map_err(|e| AgentError::SigningFailed {
                key_fingerprint: key_fingerprint.to_string(),
                message: format!("Failed to write temp file: {}", e),
            })?;

        let output = Command::new("ssh-keygen")
            .arg("-Y")
            .arg("sign")
            .arg("-f")
            .arg(key_fingerprint)
            .arg("-n")
            .arg("carpai-sign-request")
            .arg(&temp_file)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| AgentError::SigningFailed {
                key_fingerprint: key_fingerprint.to_string(),
                message: format!("ssh-keygen execution failed: {}", e),
            })?;

        // Cleanup temp file
        let _ = std::fs::remove_file(&temp_file);

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(AgentError::SigningFailed {
                key_fingerprint: key_fingerprint.to_string(),
                message: stderr.to_string(),
            })
        } else {
            // Parse signature output (would be base64 encoded)
            let signature = String::from_utf8_lossy(&output.stdout);
            // In real implementation, this would decode the signature format
            Ok(signature.into_bytes())
        }
    }

    // ==========================================
    // Integration with SSH Session
    // ==========================================

    /// Configure SSH command to use agent forwarding
    pub fn configure_ssh_command_for_forwarding<'a>(
        &self,
        cmd: &'a mut Command,
    ) -> &'a mut Command {
        if self.forwarding_enabled {
            if let Some(ref socket) = self.agent_socket {
                cmd.env("SSH_AUTH_SOCK", socket.display().to_string());
                cmd.arg("-A");  // Enable agent forwarding
                cmd.arg("-o").arg("ForwardAgent=yes");
            }
        }

        cmd
    }

    /// Build SSH arguments string for agent forwarding
    pub fn build_agent_forward_args(&self) -> Vec<String> {
        let mut args = vec![];

        if self.forwarding_enabled {
            if let Some(ref _socket) = self.agent_socket {
                args.push("-A".to_string());  // Agent forwarding
                args.push("-o".to_string());
                args.push("ForwardAgent=yes".to_string());
                
                // Optionally set IdentityAgent if needed
                // args.push("-o".to_string());
                // args.push(format!("IdentityAgent={}", socket.display()));
            }
        }

        args
    }

    /// Verify agent is working correctly
    pub fn health_check(&self) -> AgentHealthStatus {
        match &self.agent_socket {
            None => AgentHealthStatus::NotConfigured,
            Some(socket) => {
                if self._is_agent_running(socket) {
                    match self.get_identities().len() {
                        0 => AgentHealthStatus::RunningNoIdentities,
                        n => AgentHealthStatus::Healthy { identity_count: n },
                    }
                } else {
                    AgentHealthStatus::Disconnected {
                        socket_path: socket.clone(),
                    }
                }
            }
        }
    }

    // ==========================================
    // Private Helper Methods
    // ==========================================

    fn _is_agent_running(&self, socket_path: &Path) -> bool {
        // Try to connect to the Unix domain socket
        #[cfg(unix)]
        {
            use std::os::unix::net::UnixStream;
            UnixStream::connect(socket_path).is_ok()
        }

        // Fallback for Windows/non-Unix: check if file exists and looks like socket
        #[cfg(not(unix))]
        {
            socket_path.exists() && socket_path.file_name()
                .map(|name| name.to_string_lossy().starts_with("agent."))
                .unwrap_or(false)
        }
    }

    fn _find_agent_socket(&self, pattern: &Path) -> Option<PathBuf> {
        // Simple glob-like search for agent sockets
        let parent = pattern.parent().unwrap_or(Path::new("/tmp"));
        
        if let Ok(entries) = std::fs::read_dir(parent) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                
                if name_str.starts_with("agent.") || name_str.contains("agent.") {
                    let full_path = entry.path();
                    if self._is_agent_running(&full_path) {
                        return Some(full_path);
                    }
                }
            }
        }

        None
    }

    fn _execute_ssh_add_command(
        &self, 
        args: &str, 
        socket: &Path
    ) -> Result<std::process::Output, AgentError> {
        self._execute_ssh_add_command_with_args(args, &[], socket)
    }

    fn _execute_ssh_add_command_with_args(
        &self,
        main_arg: &str,
        extra_args: &[String],
        socket: &Path,
    ) -> Result<std::process::Output, AgentError> {
        let mut cmd = Command::new("ssh-add");
        
        for arg in extra_args {
            cmd.arg(arg);
        }
        
        cmd.arg(main_arg)
           .env("SSH_AUTH_SOCK", socket.display().to_string())
           .stdout(Stdio::piped())
           .stderr(Stdio::piped());

        cmd.output().map_err(|e| AgentError::ConnectionFailed {
            socket_path: socket.to_path_buf(),
            message: format!("Failed to execute ssh-add: {}", e),
        })
    }

    fn _parse_identity_line(&self, line: &str) -> Option<AgentIdentity> {
        // Parse formats:
        // "256 SHA256:abcdef123456... user@host (ED25519)"
        // "4096 SHA256:abcdef... /path/to/key (RSA)"
        
        if line.is_empty() || line.contains("The agent has no identities") {
            return None;
        }

        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 4 {
            return None;
        }

        let bits = parts[0];         // Key size (1024, 2048, 256, etc.)
        let hash_type_and_value = parts[1];  // SHA256:value or MD5:value
        let comment_parts: Vec<&str> = parts[2..parts.len()-1].join(" ").split('(').collect();
        let comment = comment_parts.first().unwrap_or(&"").trim().to_string();
        let key_type_raw = parts.last()?.trim_end_matches(')');
        let key_type = key_type_raw.trim_start_matches('[').trim_end_matches(']');

        // Extract fingerprint from hash_type:value
        let fingerprint = hash_type_and_value.split(':')
            .nth(1)
            .unwrap_or(hash_type_and_value)
            .to_string();

        // Determine actual key type from bits
        let actual_key_type = match bits {
            "256" | "25519" => "ed25519".to_string(),
            "521" => "ecdsa-sha2-nistp521".to_string(),
            "384" => "ecdsa-sha2-nistp384".to_string(),
            "256" if key_type.contains("ECDSA") => "ecdsa-sha2-nistp256".to_string(),
            _ if key_type.to_lowercase().contains("rsa") => "rsa".to_string(),
            _ => key_type.to_lowercase(),
        };

        Some(AgentIdentity {
            key_type: actual_key_type.to_string(),
            fingerprint,
            comment,
            expires_at: None,  // Would need ssh-add -l -E to get expiry
        })
    }
}

#[derive(Debug, Clone)]
pub enum AgentHealthStatus {
    NotConfigured,
    Disconnected {
        socket_path: PathBuf,
    },
    RunningNoIdentities,
    Healthy {
        identity_count: usize,
    },
}

impl std::fmt::Display for AgentHealthStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentHealthStatus::NotConfigured => {
                write!(f, "Agent: Not configured")
            }
            AgentHealthStatus::Disconnected { socket_path } => {
                write!(f, "Agent: Disconnected (socket: {})", socket_path.display())
            }
            AgentHealthStatus::RunningNoIdentities => {
                write!(f, "Agent: Running but no identities loaded")
            }
            AgentHealthStatus::Healthy { identity_count } => {
                write!(f, "Agent: Healthy ({} identities)", identity_count)
            }
        }
    }
}

/// Utility function to start ssh-agent if not running
pub fn ensure_ssh_agent_running() -> Result<SshAgentManager, AgentError> {
    let mut manager = SshAgentManager::new();
    
    match manager.detect_agent() {
        Ok(_) => Ok(manager),
        Err(_) => {
            // Try to start ssh-agent
            eprintln!("[SSH-AGENT] No running agent found, attempting to start ssh-agent...");
            
            let output = Command::new("ssh-agent")
                .arg("-s")  // Shell-friendly output
                .arg("-t")  // Lifetime (optional)
                .arg("3600")  // 1 hour default
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
                .map_err(|e| AgentError::AgentNotFound {
                    message: format!("Failed to start ssh-agent: {}", e),
                })?;

            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                
                // Parse SSH_AUTH_SOCK and SSH_AGENT_PID from output
                for line in stdout.lines() {
                    if line.contains("SSH_AUTH_SOCK") {
                        if let Some(sock_path) = line.split('=').nth(1) {
                            let trimmed = sock_path.trim_end_matches(';').trim();
                            unsafe { env::set_var("SSH_AUTH_SOCK", trimmed) };
                            eprintln!("[SSH-AGENT] Started new agent at: {}", trimmed);
                            
                            return Ok(SshAgentManager::with_socket(
                                PathBuf::from(trimmed)
                            ));
                        }
                    }
                    
                    if line.contains("SSH_AGENT_PID") {
                        if let Some(pid) = line.split('=').nth(1) {
                            let pid = pid.trim_end_matches(';').trim();
                            unsafe { env::set_var("SSH_AGENT_PID", pid) };
                        }
                    }
                }

                // Retry detection after setting env var
                manager.detect_agent()?;
                Ok(manager)
            } else {
                Err(AgentError::AgentNotFound {
                    message: "Failed to start ssh-agent".to_string(),
                })
            }
        }
    }
}
