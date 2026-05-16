use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::fs;
use serde::{Serialize, Deserialize};

/// SSH Host Configuration from ~/.ssh/config
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshHostConfig {
    pub host: String,
    pub host_name: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Vec<PathBuf>,
    pub proxy_command: Option<String>,
    pub local_forward: Vec<PortForwardRule>,
    pub remote_forward: Vec<PortForwardRule>,
    pub dynamic_forward: Option<u16>,
    pub compression: Option<bool>,
    pub connect_timeout: Option<u64>,
    pub server_alive_interval: Option<u64>,
    pub strict_host_key_checking: Option<bool>,
    pub forward_agent: Option<bool>,
    pub forward_x11: Option<bool>,
    pub control_master: Option<ControlMasterMode>,
    pub control_path: Option<PathBuf>,
    pub environment: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ControlMasterMode {
    Auto,
    AutoAsk,
    Yes,
    No,
    Ask,
}

impl Default for ControlMasterMode {
    fn default() -> Self { ControlMasterMode::Auto }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortForwardRule {
    pub bind_address: Option<String>,
    pub local_port: u16,
    pub remote_host: String,
    pub remote_port: u16,
}

/// SSH Config Parser for ~/.ssh/config files
pub struct ConfigParser;

impl ConfigParser {
    /// Parse ~/.ssh/config file and return all host configurations
    pub fn parse() -> Result<Vec<SshHostConfig>, String> {
        let config_path = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".ssh")
            .join("config");

        if !config_path.exists() {
            return Ok(vec![]);
        }

        let content = fs::read_to_string(&config_path)
            .map_err(|e| format!("Failed to read config file: {}", e))?;

        Self::parse_content(&content)
    }

    /// Parse config file content
    pub fn parse_content(content: &str) -> Result<Vec<SshHostConfig>, String> {
        let mut hosts = vec![];
        let mut current_host: Option<SshHostConfig> = None;
        let mut in_match_block = false;
        let mut match_depth = 0;

        for line in content.lines() {
            let line = line.trim();
            
            // Skip comments and empty lines
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            // Handle Match blocks (simplified)
            if line.starts_with("Match ") || line.starts_with("match ") {
                match_depth += 1;
                if match_depth == 1 { in_match_block = true; }
                continue;
            }
            if line.starts_with("EndMatch") || line.starts_with("endmatch") {
                match_depth -= 1;
                if match_depth == 0 { 
                    in_match_block = false; 
                    if let Some(host) = current_host.take() {
                        hosts.push(host);
                    }
                }
                continue;
            }

            // Skip lines inside Match blocks for now
            if in_match_block && match_depth > 0 {
                continue;
            }

            // Parse Host directive
            if line.starts_with("Host ") {
                if let Some(host) = current_host.take() {
                    hosts.push(host);
                }

                let patterns: Vec<&str> = line[5..].split_whitespace().collect();
                
                // Create config for each pattern (usually just one)
                for pattern in patterns {
                    current_host = Some(SshHostConfig {
                        host: pattern.to_string(),
                        ..Default::default()
                    });
                    break; // Take first pattern as primary
                }
                continue;
            }

            // Parse other directives within Host block
            if let Some(ref mut host) = current_host {
                Self::parse_directive(line, host);
            }
        }

        // Don't forget the last host block
        if let Some(host) = current_host {
            hosts.push(host);
        }

        Ok(hosts)
    }

    fn parse_directive(line: &str, host: &mut SshHostConfig) {
        if let Some(value) = Self::extract_value(line, "HostName") {
            host.host_name = Some(value.to_string());
        } else if let Some(value) = Self::extract_value(line, "User") {
            host.user = Some(value.to_string());
        } else if let Some(value) = Self::extract_value(line, "Port") {
            host.port = value.parse().ok();
        } else if let Some(value) = Self::extract_value(line, "IdentityFile") {
            let path = PathBuf::from(Self::expand_tilde(value));
            if !host.identity_file.contains(&path) {
                host.identity_file.push(path);
            }
        } else if let Some(value) = Self::extract_value(line, "ProxyCommand") {
            host.proxy_command = Some(value.to_string());
        } else if line.starts_with("LocalForward ") {
            if let Some(rule) = Self::parse_port_forward(&line[13..]) {
                host.local_forward.push(rule);
            }
        } else if line.starts_with("RemoteForward ") {
            if let Some(rule) = Self::parse_port_forward(&line[14..]) {
                host.remote_forward.push(rule);
            }
        } else if let Some(value) = Self::extract_value(line, "DynamicForward") {
            host.dynamic_forward = value.parse().ok();
        } else if let Some(value) = Self::extract_value(line, "Compression") {
            host.compression = Some(value.eq_ignore_ascii_case("yes"));
        } else if let Some(value) = Self::extract_value(line, "ConnectTimeout") {
            host.connect_timeout = value.parse().ok();
        } else if let Some(value) = Self::extract_value(line, "ServerAliveInterval") {
            host.server_alive_interval = value.parse().ok();
        } else if let Some(value) = Self::extract_value(line, "StrictHostKeyChecking") {
            host.strict_host_key_checking = Some(
                value.eq_ignore_ascii_case("yes") || value.eq_ignore_ascii_case("accept-new")
            );
        } else if let Some(value) = Self::extract_value(line, "ForwardAgent") {
            host.forward_agent = Some(value.eq_ignore_ascii_case("yes"));
        } else if let Some(value) = Self::extract_value(line, "ForwardX11") {
            host.forward_x11 = Some(value.eq_ignore_ascii_case("yes"));
        } else if let Some(value) = Self::extract_value(line, "ControlMaster") {
            host.control_master = Some(match value.to_lowercase().as_str() {
                "auto" => ControlMasterMode::Auto,
                "autoask" => ControlMasterMode::AutoAsk,
                "yes" | "true" => ControlMasterMode::Yes,
                "no" | "false" => ControlMasterMode::No,
                "ask" => ControlMasterMode::Ask,
                _ => ControlMasterMode::Auto,
            });
        } else if let Some(value) = Self::extract_value(line, "ControlPath") {
            host.control_path = Some(PathBuf::from(Self::expand_tilde(value)));
        } else if line.starts_with("SetEnv ") {
            // Parse SetEnv KEY=VALUE or SetEnv KEY VALUE
            let env_str = &line[7..].trim();
            if let Some(eq_pos) = env_str.find('=') {
                let key = env_str[..eq_pos].trim().to_string();
                let value = env_str[eq_pos+1..].trim().to_string();
                host.environment.insert(key, value);
            } else if let Some(space_pos) = env_str.find(' ') {
                let key = env_str[..space_pos].trim().to_string();
                let value = env_str[space_pos+1..].trim().to_string();
                host.environment.insert(key, value);
            }
        }
    }

    fn extract_value<'a>(line: &'a str, keyword: &str) -> Option<&'a str> {
        if line.starts_with(keyword) && line.len() > keyword.len() {
            let rest = line[keyword.len()..].trim();
            if !rest.is_empty() {
                return Some(rest);
            }
        }
        None
    }

    fn parse_port_forward(spec: &str) -> Option<PortForwardRule> {
        let parts: Vec<&str> = spec.split_whitespace().collect();
        
        match parts.len() {
            2 => {
                // [bind:]port host:hostport format
                let first = parts[0];
                let second = parts[1];
                
                if first.contains(':') {
                    // bind:port format
                    let bind_parts: Vec<&str> = first.split(':').collect();
                    let bind_addr = if bind_parts[0].is_empty() { None } else { Some(bind_parts[0].to_string()) };
                    let port: u16 = bind_parts.get(1)?.parse().ok()?;
                    
                    if second.contains(':') {
                        let host_parts: Vec<&str> = second.split(':').collect();
                        Some(PortForwardRule {
                            bind_address: bind_addr,
                            local_port: port,
                            remote_host: host_parts[0].to_string(),
                            remote_port: host_parts.get(1)?.parse().ok()?,
                        })
                    } else { None }
                } else {
                    // Simple port host:hostport
                    let port: u16 = first.parse().ok()?;
                    if second.contains(':') {
                        let host_parts: Vec<&str> = second.split(':').collect();
                        Some(PortForwardRule {
                            bind_address: None,
                            local_port: port,
                            remote_host: host_parts[0].to_string(),
                            remote_port: host_parts.get(1)?.parse().ok()?,
                        })
                    } else { None }
                }
            }
            _ => None,
        }
    }

    fn expand_tilde(path: &str) -> String {
        if path.starts_with("~/") || path == "~" {
            if let Some(home) = dirs::home_dir() {
                return path.replacen("~", &home.display().to_string(), 1);
            }
        }
        path.to_string()
    }

    /// Find configuration for a specific host by name or alias
    pub fn find_host(host_name: &str) -> Result<Option<SshHostConfig>, String> {
        let configs = Self::parse()?;
        
        for config in &configs {
            // Check exact match
            if config.host == host_name {
                return Ok(Some(config.clone()));
            }
            
            // Check hostname match
            if let Some(ref hostname) = config.host_name {
                if hostname == host_name {
                    return Ok(Some(config.clone()));
                }
            }
            
            // Check wildcard patterns (simplified)
            if config.host.contains('*') || config.host.contains('?') {
                if Self::pattern_matches(&config.host, host_name) {
                    return Ok(Some(config.clone()));
                }
            }
        }

        Ok(None)
    }

    /// Simple glob-like pattern matching
    fn pattern_matches(pattern: &str, text: &str) -> bool {
        let pattern_chars: Vec<char> = pattern.chars().collect();
        let text_chars: Vec<char> = text.chars().collect();
        
        Self::_match_helper(&pattern_chars, &text_chars, 0, 0)
    }

    fn _match_helper(pattern: &[char], text: &[usize], p_idx: usize, t_idx: usize) -> bool {
        if p_idx == pattern.len() {
            return t_idx == text.len();
        }

        match pattern[p_idx] {
            '*' => {
                // Try matching zero or more characters
                for i in t_idx..=text.len() {
                    if Self::_match_helper(pattern, text, p_idx + 1, i) {
                        return true;
                    }
                }
                false
            }
            '?' => {
                if t_idx < text.len() {
                    Self::_match_helper(pattern, text, p_idx + 1, t_idx + 1)
                } else {
                    false
                }
            }
            c => {
                if t_idx < text.len() && text[t_idx] == c {
                    Self::_match_helper(pattern, text, p_idx + 1, t_idx + 1)
                } else {
                    false
                }
            }
        }
    }

    /// List all configured hosts
    pub fn list_hosts() -> Result<Vec<String>, String> {
        let configs = Self::parse()?;
        Ok(configs.into_iter().map(|c| c.host).collect())
    }

    /// Validate SSH configuration file
    pub fn validate_config() -> Result<Vec<String>, String> {
        let mut warnings = vec![];
        let mut errors = vec![];

        let config_path = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".ssh")
            .join("config");

        if !config_path.exists() {
            warnings.push("No ~/.ssh/config file found".to_string());
            return Ok(warnings);
        }

        let configs = Self::parse()?;
        
        if configs.is_empty() {
            warnings.push("No Host blocks defined in config".to_string());
        }

        for config in &configs {
            // Check for required fields
            if config.host_name.is_none() && !config.host.contains('*') {
                warnings.push(format!("Host '{}' has no HostName specified", config.host));
            }

            if config.user.is_none() {
                warnings.push(format!("Host '{}' has no User specified (will use system default)", config.host));
            }

            // Check identity files exist
            for identity in &config.identity_file {
                if !identity.exists() {
                    warnings.push(format!("IdentityFile '{}' does not exist", identity.display()));
                }
            }

            // Check for potential issues
            if config.port == Some(0) {
                errors.push(format!("Host '{}' has invalid port 0", config.host));
            }
        }

        if !errors.is_empty() {
            return Err(errors.join("\n"));
        }

        Ok(warnings)
    }
}

impl Default for SshHostConfig {
    fn default() -> Self {
        SshHostConfig {
            host: String::new(),
            host_name: None,
            user: None,
            port: None,
            identity_file: vec![],
            proxy_command: None,
            local_forward: vec![],
            remote_forward: vec![],
            dynamic_forward: None,
            compression: None,
            connect_timeout: None,
            server_alive_interval: None,
            strict_host_key_checking: None,
            forward_agent: None,
            forward_x11: None,
            control_master: None,
            control_path: None,
            environment: HashMap::new(),
        }
    }
}
