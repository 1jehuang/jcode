use std::process::{Command, Child, Stdio};
use std::path::PathBuf;

use std::sync::{Arc, Mutex};

/// Port Forwarding Types
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ForwardType {
    Local,      // -L local_port:remote_host:remote_port
    Remote,     // -R remote_port:local_host:local_port
    Dynamic,    // -D local_port (SOCKS5)
}

/// Port Forwarder for managing SSH tunnels
pub struct PortForwarder {
    id: String,
    forward_type: ForwardType,
    local_port: u16,
    remote_host: Option<String>,
    remote_port: Option<u16>,
    bind_address: Option<String>,
    ssh_target: String,
    child: Option<Child>,
    config: PortForwardConfig,
}

#[derive(Debug, Clone)]
pub struct PortForwardConfig {
    pub identity_file: Option<PathBuf>,
    pub port: u16,
    pub user: String,
    pub host: String,
    pub exit_on_close: bool,
    pub verbose: bool,
}

impl Default for PortForwardConfig {
    fn default() -> Self {
        PortForwardConfig {
            identity_file: None,
            port: 22,
            user: std::env::var("USER").unwrap_or_else(|_| "user".to_string()),
            host: "localhost".to_string(),
            exit_on_close: true,
            verbose: false,
        }
    }
}

impl PortForwarder {
    /// Create new port forwarder
    pub fn new(forward_type: ForwardType, config: PortForwardConfig) -> Self {
        PortForwarder {
            id: uuid::Uuid::new_v4().to_string(),
            forward_type,
            local_port: 0,
            remote_host: None,
            remote_port: None,
            bind_address: None,
            ssh_target: format!("{}@{}", config.user, config.host),
            child: None,
            config,
        }
    }

    /// Configure local port forwarding (-L)
    pub fn local_forward(local_port: u16, remote_host: &str, remote_port: u16) -> Self {
        let mut forwarder = Self::new(ForwardType::Local, PortForwardConfig::default());
        forwarder.local_port = local_port;
        forwarder.remote_host = Some(remote_host.to_string());
        forwarder.remote_port = Some(remote_port);
        forwarder
    }

    /// Configure remote port forwarding (-R)
    pub fn remote_forward(remote_port: u16, local_host: &str, local_port: u16) -> Self {
        let mut forwarder = Self::new(ForwardType::Remote, PortForwardConfig::default());
        forwarder.local_port = remote_port;
        forwarder.remote_host = Some(local_host.to_string());
        forwarder.remote_port = Some(local_port);
        forwarder
    }

    /// Configure dynamic/SOCKS forwarding (-D)
    pub fn dynamic_forward(local_port: u16) -> Self {
        let mut forwarder = Self::new(ForwardType::Dynamic, PortForwardConfig::default());
        forwarder.local_port = local_port;
        forwarder
    }

    /// Set bind address (optional)
    pub fn bind_address(mut self, addr: &str) -> Self {
        self.bind_address = Some(addr.to_string());
        self
    }

    /// Set SSH target (user@host)
    pub fn target(mut self, user: &str, host: &str) -> Self {
        self.config.user = user.to_string();
        self.config.host = host.to_string();
        self.ssh_target = format!("{}@{}", user, host);
        self
    }

    /// Set SSH port
    pub fn ssh_port(mut self, port: u16) -> Self {
        self.config.port = port;
        self
    }

    /// Set identity file
    pub fn identity_file(mut self, path: &PathBuf) -> Self {
        self.config.identity_file = Some(path.clone());
        self
    }

    /// Start the port forwarding tunnel
    pub fn start(&mut self) -> Result<String, String> {
        if self.child.is_some() {
            return Err("Tunnel already running".to_string());
        }

        let mut cmd = Command::new("ssh");
        
        // Common SSH options
        cmd.arg("-o").arg("BatchMode=yes")
           .arg("-o").arg("ServerAliveInterval=30")
           .arg("-N")  // Don't execute remote command
           .arg("-f"); // Go to background

        // Add verbose mode if requested
        if self.config.verbose {
            cmd.arg("-v");
        }

        // Add port
        cmd.arg("-p").arg(self.config.port.to_string());

        // Add identity file
        if let Some(ref identity) = self.config.identity_file {
            cmd.arg("-i").arg(identity.display().to_string());
        }

        // Add port forwarding argument based on type
        match self.forward_type {
            ForwardType::Local => {
                let spec = match (&self.bind_address, &self.remote_host, &self.remote_port) {
                    (Some(addr), Some(host), Some(port)) => 
                        format!("{}:{}:{}:{}", addr, self.local_port, host, port),
                    (None, Some(host), Some(port)) => 
                        format!("{}:{}:{}", self.local_port, host, port),
                    _ => return Err("Incomplete local forward configuration".to_string()),
                };
                cmd.arg("-L").arg(spec);
            }
            ForwardType::Remote => {
                let spec = match (&self.bind_address, &self.remote_host, &self.remote_port) {
                    (Some(addr), Some(host), Some(port)) => 
                        format!("{}:{}:{}:{}", addr, self.local_port, host, port),
                    (None, Some(host), Some(port)) => 
                        format!("{}:{}:{}", self.local_port, host, port),
                    _ => return Err("Incomplete remote forward configuration".to_string()),
                };
                cmd.arg("-R").arg(spec);
            }
            ForwardType::Dynamic => {
                cmd.arg("-D").arg(self.local_port.to_string());
            }
        }

        // Add exit on close option
        if self.config.exit_on_close {
            cmd.arg("-o").arg("ExitOnForwardFailure=yes");
        }

        // Add target
        cmd.arg(&self.ssh_target);

        // Spawn the process
        let child = cmd.spawn()
            .map_err(|e| format!("Failed to start tunnel: {}", e))?;

        self.child = Some(child);

        Ok(format!(
            "✅ {} tunnel started\nID: {}\nType: {:?}\nTarget: {}\nPort: {}",
            match self.forward_type {
                ForwardType::Local => "Local",
                ForwardType::Remote => "Remote",
                ForwardType::Dynamic => "Dynamic/SOCKS",
            },
            self.id,
            self.forward_type,
            self.ssh_target,
            self.local_port
        ))
    }

    /// Stop the port forwarding tunnel
    pub fn stop(&mut self) -> Result<String, String> {
        if let Some(mut child) = self.child.take() {
            child.kill()
                .map_err(|e| format!("Failed to kill tunnel process: {}", e))?;
            
            let _status = child.wait();

            Ok(format!(
                "🔌 Tunnel stopped\nID: {}\nType: {:?}\nPort: {}",
                self.id,
                self.forward_type,
                self.local_port
            ))
        } else {
            Err("Tunnel not running".to_string())
        }
    }

    /// Check if tunnel is running
    pub fn is_running(&mut self) -> bool {
        self.child.as_mut().map_or(false, |c| {
            c.try_wait().map_or(true, |s| s.is_none())
        })
    }

    /// Get tunnel ID
    pub fn id(&self) -> &str { &self.id }

    /// Get forward type
    pub fn forward_type(&self) -> ForwardType { self.forward_type }

    /// Get local port
    pub fn local_port(&self) -> u16 { self.local_port }

    /// Get configuration reference
    pub fn config(&self) -> &PortForwardConfig { &self.config }
}

/// Tunnel Manager for managing multiple port forwards
pub struct TunnelManager {
    tunnels: Arc<Mutex<Vec<PortForwarder>>>,
}

impl TunnelManager {
    pub fn new() -> Self {
        TunnelManager {
            tunnels: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Add and start a new tunnel
    pub fn add_tunnel(&mut self, mut tunnel: PortForwarder) -> Result<String, String> {
        let result = tunnel.start()?;
        
        if let Ok(mut tunnels) = self.tunnels.lock() {
            tunnels.push(tunnel);
        }

        Ok(result)
    }

    /// Stop and remove a tunnel by ID
    pub fn remove_tunnel(&mut self, id: &str) -> Result<(), String> {
        if let Ok(mut tunnels) = self.tunnels.lock() {
            if let Some(pos) = tunnels.iter().position(|t| t.id() == id) {
                let mut tunnel = tunnels.remove(pos);
                tunnel.stop()?;
                return Ok(());
            }
        }
        Err(format!("Tunnel '{}' not found", id))
    }

    /// Stop all tunnels
    pub fn stop_all(&mut self) -> Result<usize, String> {
        if let Ok(mut tunnels) = self.tunnels.lock() {
            let count = tunnels.len();
            
            for tunnel in tunnels.iter_mut() {
                let _ = tunnel.stop();
            }
            
            tunnels.clear();
            return Ok(count);
        }
        Ok(0)
    }

    /// List active tunnels
    pub fn list_tunnels(&self) -> Vec<TunnelInfo> {
        let tunnels_guard = match self.tunnels.lock() {
            Ok(g) => g,
            Err(_) => return Vec::new(),
        };
        tunnels_guard
            .iter()
            .filter_map(|t| {
                let is_running = t.child.as_mut().map_or(false, |c| {
                    c.try_wait().map_or(true, |s| s.is_none())
                });
                if is_running {
                    Some(TunnelInfo {
                        id: t.id().to_string(),
                        forward_type: t.forward_type(),
                        local_port: t.local_port(),
                        target: t.ssh_target.clone(),
                        running: true,
                    })
                } else {
                    None
                }
            })
            .collect()
    }

    /// Get tunnel by ID
    pub fn get_tunnel(&self, id: &str) -> Option<PortForwarder> {
        self.tunnels.lock()
            .ok()
            .and_then(|tunnels| {
                tunnels.iter()
                    .find(|t| t.id() == id)
                    .cloned()
            })
    }
}

#[derive(Debug, Clone)]
pub struct TunnelInfo {
    pub id: String,
    pub forward_type: ForwardType,
    pub local_port: u16,
    pub target: String,
    pub running: bool,
}

/// Jump Host / Bastion Host support
pub struct JumpHostChain {
    hosts: Vec<JumpHostEntry>,
}

#[derive(Debug, Clone)]
pub struct JumpHostEntry {
    pub user: String,
    pub host: String,
    pub port: u16,
    pub identity_file: Option<PathBuf>,
}

impl JumpHostChain {
    pub fn new() -> Self {
        JumpHostChain { hosts: vec![] }
    }

    /// Add a jump host to the chain
    pub fn add_jump(mut self, user: &str, host: &str, port: u16) -> Self {
        self.hosts.push(JumpHostEntry {
            user: user.to_string(),
            host: host.to_string(),
            port,
            identity_file: None,
        });
        self
    }

    /// Build SSH -J argument string
    pub fn build_jump_arg(&self) -> String {
        self.hosts.iter()
            .map(|h| format!("{}@{}:{}", h.user, h.host, h.port))
            .collect::<Vec<_>>()
            .join(",")
    }

    /// Create intermediate tunnel through jump hosts
    pub fn create_tunnel_chain(&self, _final_user: &str, final_host: &str, final_port: u16) -> Result<Vec<PortForwarder>, String> {
        let mut tunnels = vec![];
        let mut prev_port = final_port;

        // Work backwards through jump hosts
        for (i, jump) in self.hosts.iter().rev().enumerate() {
            let local_port = 20000 + i as u16; // Use high ports
            
            let mut tunnel = if i == 0 {
                // Final hop to destination
                PortForwarder::local_forward(local_port, final_host, prev_port)
            } else {
                // Intermediate hop to next jump host
                let next_host = &self.hosts[self.hosts.len() - i];
                PortForwarder::local_forward(local_port, &next_host.host, prev_port)
            };

            tunnel = tunnel.target(&jump.user, &jump.host)
                          .ssh_port(jump.port);

            tunnels.push(tunnel);
            prev_port = local_port;
        }

        Ok(tunnels.into_iter().rev().collect())
    }
}
