# CarpAI SSH Remote Connection System - Complete API Documentation

## 📚 Table of Contents

1. [Quick Start Guide](#quick-start-guide)
2. [Configuration Management](#configuration-management)
3. [Session Management](#session-management)
4. [Command Execution](#command-execution)
5. [File Transfer](#file-transfer)
6. [Port Forwarding & Tunneling](#port-forwarding--tunneling)
7. [Connection Pooling](#connection-pooling)
8. [Audit Logging](#audit-logging)
9. [Error Recovery & Resilience](#error-recovery--resilience)
10. [Advanced Features](#advanced-features)

---

## Quick Start Guide

### Basic SSH Connection

```rust
use carpai::ssh::{SshSession, SshConfig};

// Create configuration
let config = SshConfig::with_host("user@example.com");

// Create and connect session
let mut session = SshSession::new(config);
match session.connect() {
    Ok(msg) => println!("Connected: {}", msg),
    Err(e) => eprintln!("Failed to connect: {}", e),
}

// Execute command
let output = session.execute("ls -la")?;
println!("Output:\n{}", output.stdout);

// Disconnect
session.disconnect()?;
```

### Using ~/.ssh/config

```rust
use carpai::ssh::{SshConfig, ConfigParser};

// Load from SSH config file
let config = SshConfig::from_host_config("myserver")?;
let mut session = SshSession::new(config);
session.connect()?;
```

---

## Configuration Management

### SshConfig Structure

```rust
pub struct SshConfig {
    pub host: String,                    // Remote hostname or IP
    pub port: u16,                       // SSH port (default: 22)
    pub user: String,                    // Username
    pub identity_file: Option<PathBuf>,  // Path to private key
    pub connect_timeout: Duration,        // Connection timeout
    pub keepalive_interval: Duration,     // Keepalive interval
    pub server_alive_interval: u64,       // Server alive interval
    pub server_alive_count_max: u32,      // Max server alive failures
    pub compression: bool,                // Enable compression
    pub control_master: bool,             // Use connection multiplexing
    pub control_path: Option<PathBuf>,    // Control socket path
    pub strict_host_key_checking: bool,   // Host key verification
    pub known_hosts_file: Option<PathBuf>, // Known hosts file path
    pub proxy_command: Option<String>,     // Proxy command
    pub jump_hosts: Vec<JumpHost>,         // Jump host chain
    pub local_forwards: Vec<PortForward>,  // Local port forwards
    pub remote_forwards: Vec<PortForward>, // Remote port forwards
    pub dynamic_forward: Option<u16>,      // SOCKS5 proxy port
    pub environment: HashMap<String, String>, // Environment variables
    pub request_pty: bool,                 // Request PTY allocation
}
```

### Configuration Methods

```rust
// Default configuration
let config = SshConfig::default();

// Quick creation with host
let config = SshConfig::with_host("example.com");
let config = SshConfig::with_host("user@example.com:2222");

// From ~/.ssh/config
let config = SshConfig::from_host_config("myalias")?;

// Build SSH arguments for custom use
let args = config.build_ssh_args();
```

### Advanced Configuration Examples

```rust
use carpai::ssh::{SshConfig, JumpHost, PortForward};
use std::time::Duration;

// Complex configuration with jump hosts and port forwarding
let mut config = SshConfig {
    host: "internal-server.example.com".to_string(),
    port: 22,
    user: "deploy".to_string(),
    connect_timeout: Duration::from_secs(60),
    compression: true,
    ..SshConfig::default()
};

// Add jump host (bastion)
config.jump_hosts.push(JumpHost {
    host: "bastion.example.com".to_string(),
    user: "jump-user".to_string(),
    port: 22,
    identity_file: Some(PathBuf::from("~/.ssh/bastion_key")),
});

// Add local port forwarding
config.local_forwards.push(PortForward {
    local_port: 8080,
    remote_host: "localhost".to_string(),
    remote_port: 3000,
    bind_address: Some("127.0.0.1".to_string()),
});

// Set environment variables
config.environment.insert("DEPLOY_ENV".to_string(), "production".to_string());
```

---

## Session Management

### Basic Operations

```rust
let mut session = SshSession::new(config);

// Connect to remote host
let result = session.connect();
match result {
    Ok(msg) => println!("✅ {}", msg),
    Err(e) => println!("❌ {}", e),
}

// Check session status
println!("Connected: {}", session.is_alive());
println!("Uptime: {:?}", session.uptime());
println!("Idle time: {:?}", session.idle_time());
println!("State: {:?}", session.state());

// Get statistics
let stats = session.stats();
println!("Commands executed: {}", stats.commands_executed);
println!("Bytes transferred: {}", stats.bytes_transferred);

// Disconnect
session.disconnect()?;
```

### Session Lifecycle Example

```rust
fn manage_ssh_session() -> Result<(), String> {
    let config = SshConfig::with_host("production-server.com");
    
    {
        let mut session = SshSession::new(config);
        
        // Connect with error handling
        session.connect().map_err(|e| format!("Connection failed: {}", e))?;
        
        // Perform operations
        let output = session.execute("df -h")?;
        println!("Disk usage:\n{}", output.stdout);
        
        // Upload deployment artifact
        session.upload(
            &PathBuf::from("./artifact.tar.gz"),
            &PathBuf::from("/tmp/artifact.tar.gz")
        )?;
        
        // Session automatically disconnected when dropped
    } // <- session goes out of scope here
    
    Ok(())
}
```

---

## Command Execution

### Synchronous Execution

```rust
// Simple command execution
let output = session.execute("ls -la /home")?;

if output.exit_code == Some(0) {
    println!("Success:\n{}", output.stdout);
} else {
    eprintln!("Error (exit code {:?}):\n{}", output.exit_code, output.stderr);
}
```

### Streaming Output (Real-time)

```rust
// Process output line by line in real-time
session.execute_streaming("tail -f /var/log/syslog", |line| {
    println!("[LOG] {}", line);
})?;
```

### Interactive Commands (PTY)

```rust
// Handle interactive commands like sudo
session.execute_interactive("sudo -s", |prompt| -> Option<String> {
    if prompt.contains("[sudo] password for") {
        Some("my_secret_password\n".to_string())
    } else if prompt.contains("# ") || prompt.contains("$ ") {
        None // Exit interaction loop
    } else {
        None // No response needed
    }
})?;
```

### Asynchronous Execution

```rust
// Run command in background
#[tokio::main]
async fn async_example() -> Result<(), String> {
    let exit_code = session.execute_async("long_running_task.sh").await?;
    println!("Background task exited with code: {}", exit_code);
    Ok(())
}
```

### Command Builder Pattern

```rust
use carpai::ssh::enhanced::SshCommandBuilder;

let command = SshCommandBuilder::new("target", "make deploy")
    .arg("--verbose")
    .arg("--dry-run")
    .env("DEPLOY_USER", "ci-bot")
    .env("BRANCH", "main")
    .timeout(Duration::from_secs(300))
    .working_dir(Path::new("/app"))
    .build();

let output = session.execute(&command)?;
```

---

## File Transfer

### Basic File Upload/Download

```rust
// Upload single file
session.upload(
    &PathBuf::from("./local_file.txt"),
    &PathBuf::from("/remote/path/file.txt")
)?;

// Download single file
session.download(
    &PathBuf::from("/remote/path/file.txt"),
    &PathBuf::from("./downloaded_file.txt")
)?;
```

### File Transfer with Progress

```rust
// Upload with progress callback
session.upload_with_progress(
    &PathBuf::from("./large_file.tar.gz"),
    &PathBuf::from("/tmp/large_file.tar.gz"),
    |current, total| {
        let percent = (current as f64 / total as f64) * 100.0;
        print!("\rProgress: {:.1}% ({}/{})", percent, current, total);
    },
)?;
println!("\nUpload complete!");
```

### Directory Operations

```rust
// Upload entire directory recursively
session.upload_dir(
    &PathBuf::from("./project"),
    &PathBuf::from("/opt/app/project")
)?;

// Download directory
session.download_dir(
    &PathBuf::from("/var/logs"),
    &PathBuf::from("./logs_backup")
)?;
```

### Enhanced File Transfer (Rsync Integration)

```rust
use carpai::ssh::transfer::FileTransfer;

let transfer = FileTransfer::new("user", "host.com", 22)
    .with_identity(&PathBuf::from("~/.ssh/id_rsa"))
    .compress(true)
    .limit_bandwidth(10 * 1024 * 1024); // 10 MB/s limit

// Single file transfer
let result = transfer.upload_file(
    &PathBuf::from("./release.zip"),
    &PathBuf::from("/tmp/release.zip")
)?;

if result.success {
    println!("Uploaded {} bytes at {:.2} MB/s", 
        result.bytes_transferred,
        result.speed_bytes_per_sec / (1024.0 * 1024.0));
}

// Directory synchronization (rsync --delete)
let sync_result = transfer.sync_to_remote(
    &PathBuf::from("./dist"),
    &PathBuf::from("/var/www/html")
)?;
```

---

## Port Forwarding & Tunneling

### Local Port Forwarding (-L)

```rust
use carpai::ssh::tunnel::PortForwarder;

// Forward local port 8080 to remote localhost:80
let mut tunnel = PortForwarder::local_forward(8080, "localhost", 80)
    .bind_address("127.0.0.1")
    .target("user", "remote-server.com")
    .start()?;

println!("Tunnel started: {}", tunnel.id());

// Now access http://localhost:8080 -> remote:80

// Stop tunnel
tunnel.stop()?;
```

### Remote Port Forwarding (-R)

```rust
// Forward remote port 9000 to local localhost:3000
let tunnel = PortForwarder::remote_forward(9000, "localhost", 3000)
    .target("user", "public-server.com")
    .start()?;
```

### Dynamic/SOCKS Proxy (-D)

```rust
// Create SOCKS5 proxy on local port 1080
let tunnel = PortForwarder::dynamic_forward(1080)
    .target("user", "proxy-server.com")
    .start()?;

// Configure applications to use SOCKS5 proxy at localhost:1080
```

### Tunnel Manager

```rust
use carpai::ssh::tunnel::TunnelManager;
use std::sync::{Arc, Mutex};

let manager = Arc::new(Mutex::new(TunnelManager::new()));

{
    let mut mgr = manager.lock().unwrap();
    
    // Add multiple tunnels
    mgr.add_tunnel(PortForwarder::local_forward(8080, "localhost", 80).target("u", "h"))?;
    mgr.add_tunnel(PortForwarder::dynamic_forward(1080).target("u", "h"))?;
    
    // List active tunnels
    for tunnel in mgr.list_tunnels() {
        println!("Tunnel {}: {:?}", tunnel.id, tunnel.forward_type);
    }
}

// Cleanup all tunnels
manager.lock().unwrap().stop_all()?;
```

### Jump Host Chain (Bastion Support)

```rust
use carpai::ssh::tunnel::JumpHostChain;

// Create jump host chain through bastion servers
let chain = JumpHostChain::new()
    .add_jump("bastion-user", "bastion.company.com", 22)
    .add_jump("internal-jump", "10.0.0.5", 2222);

// Get SSH -J argument
let jump_arg = chain.build_jump_arg();
println!("Jump argument: {}", jump_arg);

// Or create full tunnel chain
let tunnels = chain.create_tunnel_chain(
    "final-user",
    "target-internal.company.com",
    22
)?;
```

---

## Connection Pooling

### Basic Pool Usage

```rust
use carpai::ssh::pool::SshConnectionPool;

// Create pool with max 10 connections
let pool = Arc::new(SshConnectionPool::new(10));

// Set default configuration
let pool = SshConnectionPool::new(10)
    .with_default_config(SshConfig::with_host("cluster-node"));

// Get or create session for a host
let session_id = pool.get_session("node1.example.com")?;

// Execute command using pool (auto-manages lifecycle)
let output = pool.execute_on("node1.example.com", "hostname")?;

// Return session to pool when done
pool.return_session(&session_id)?;
```

### Parallel Execution

```rust
// Execute on multiple hosts in parallel
let hosts = vec![
    ("node1".to_string(), "uptime".to_string()),
    ("node2".to_string(), "uptime".to_string()),
    ("node3".to_string(), "uptime".to_string()),
];

let results = pool.parallel_execute(hosts);

for (host, result) in &results {
    match result {
        Ok(output) => println!("{}: {}", host, output.stdout.trim()),
        Err(e) => eprintln!("{} failed: {}", host, e),
    }
}
```

### Pool Configuration and Monitoring

```rust
use carpai::ssh::pool::PoolConfig;
use std::time::Duration;

let pool = SshConnectionPool::with_config(PoolConfig {
    max_connections: 20,
    min_idle_connections: 3,
    max_idle_time: Duration::from_secs(600), // 10 minutes
    connection_timeout: Duration::from_secs(15),
    health_check_interval: Duration::from_secs(30),
    enable_auto_reconnect: true,
});

// Monitor pool status
println!("Active connections: {}", pool.active_count());
println!("Idle connections: {}", pool.idle_count());

// Evict idle sessions
let evicted = pool.evict_idle()?;
println!("Evicted {} idle sessions", evicted);

// Pool statistics
let stats = pool.stats();
println!("Total created: {}, Total destroyed: {}", 
    stats.total_created, stats.total_destroyed);
```

---

## Audit Logging

### Basic Audit Logger Setup

```rust
use carpai::ssh::audit::{SshAuditLogger, AuditConfig, LogFormat};

// Create logger with custom configuration
let logger = SshAuditLogger::with_config(AuditConfig {
    enabled: true,
    max_events_in_memory: 50000,
    log_to_file: true,
    log_file_path: Some(PathBuf::from("/var/log/carpai_ssh_audit.log")),
    log_format: LogFormat::Json,
    include_command_output: false,
    retention_days: 90,
});
```

### Logging Events

```rust
// Log connection events
logger.log_connection_attempt("admin", "prod-server", "session-123");
logger.log_connection_success("admin", "prod-server", "session-123");

// Log command execution
logger.log_command_execution(
    "session-123",
    "cat /etc/passwd",
    Some(0),  // exit code
    150,      // duration in ms
    Some("root:x:0:0:root:/root:/bin/bash\n...")  // truncated output
);

// Log file transfers
logger.log_file_transfer(
    "session-123",
    true,  // is_upload
    &PathBuf::from("/local/deploy.tar.gz"),
    10485760,  // 10MB
    true,  // success
    None   // no error
);

// Log security violations
logger.log_security_violation(
    "session-123",
    "UnauthorizedRootAccess",
    "User attempted sudo without authorization"
);
```

### Querying Events

```rust
// Get recent events
let recent = logger.get_recent_events(100);

// Filter by session
let session_events = logger.get_events_by_session("session-123");

// Filter by type
let errors = logger.get_events_by_type(&SshEventType::AuthenticationFailure);

// Filter by severity (Error and above)
let critical_events = logger.get_events_by_severity(EventSeverity::Error);

// Get statistics
let stats = logger.get_statistics();
println!("Total events: {}", stats.total_events);
println!("Security violations: {}", stats.security_violations);
println!("Critical events: {}", stats.critical_count);
```

### Export and Reporting

```rust
// Export to JSON
logger.export_to_json(&PathBuf::from("audit_report_2024.json"))?;

// Export to CSV for spreadsheet analysis
logger.export_to_csv(&PathBuf::from("audit_report_2024.csv"))?;

// Clear old events after export
logger.clear_events();
```

---

## Error Recovery & Resilience

### Smart Retry Handler

```rust
use carpai::ssh::resilience::{
    SmartRetryHandler, RetryPolicy, ReconnectStrategy, ErrorClassification
};
use std::time::Duration;

// Custom retry policy with exponential backoff
let retry_policy = RetryPolicy {
    max_attempts: 5,
    strategy: ReconnectStrategy::ExponentialBackoff {
        initial_delay: Duration::from_millis(200),
        max_delay: Duration::from_secs(30),
        multiplier: 2.0,
        jitter: true,  // Randomize delays to avoid thundering herd
    },
    retryable_errors: vec![
        ErrorClassification::Transient("Connection reset".to_string()),
        ErrorClassification::Timeout(Duration::from_secs(60)),
        ErrorClassification::NetworkUnreachable("Host unreachable".to_string()),
    ],
};

let mut retry_handler = SmartRetryHandler::new(retry_policy);

// Simulate failure handling
for attempt in 1..=retry_policy.max_attempts {
    let should_retry = retry_handler.should_retry(attempt);
    
    match should_retry {
        Ok(delay) => {
            eprintln!("[RETRY] Attempt {}/{}, waiting {:?}...", 
                attempt, retry_policy.max_attempts, delay);
            
            // Simulate delay (in real code, you'd actually try the operation here)
            std::thread::sleep(delay);
            
            // Record attempt result
            retry_handler.record_attempt(attempt, "Simulated transient error");
        }
        Err(e) => {
            eprintln!("[ABORT] {}", e);
            break;
        }
    }

    // Check circuit breaker state
    let stats = retry_handler.stats();
    println!("Circuit state: {:?}, Total attempts: {}", 
        stats.circuit_state, stats.total_attempts);
}
```

### Circuit Breaker Pattern

```rust
use carpai::ssh::resilience::{CircuitBreaker, CircuitBreakerConfig};
use std::time::Duration;

let mut breaker = CircuitBreaker::new(CircuitBreakerConfig {
    failure_threshold: 3,      // Open after 3 consecutive failures
    success_threshold: 2,      // Close after 2 successes in half-open
    timeout: Duration::from_secs(30),  // Stay open for 30 seconds
    half_open_max_calls: 1,    // Only allow 1 test call in half-open
});

// Simulate operations
for i in 0..10 {
    if breaker.allow_request() {
        println!("Operation {} allowed (state: {:?})", i, breaker.state());
        
        // Simulate success/failure
        if i % 3 == 0 {
            breaker.record_failure();  // Record failure
            println!("  → Failed");
        } else {
            breaker.record_success();  // Record success
            println!("  → Success");
        }
    } else {
        println!("Operation {} blocked (circuit open)", i);
    }

    let stats = breaker.stats();
    println!("  Stats: state={:?}, failures={}, successes={}", 
        stats.state, stats.failure_count, stats.success_count);
}
```

### Resilient SSH Session

```rust
use carpai::ssh::resilience::{ResilientSshSession, RetryPolicy};

// Create resilient session with automatic reconnection
let resilient_policy = RetryPolicy {
    max_attempts: 10,
    ..RetryPolicy::default()
};

let mut resilient_session = ResilientSshSession::new(
    SshConfig::with_host("unstable-server.com"),
    Some(resilient_policy)
);

// Connect with automatic retries
match resilient_session.connect_with_retry() {
    Ok(msg) => println!("Connected (after retries): {}", msg),
    Err(e) => eprintln!("All attempts failed: {}", e),
}

// Execute commands with auto-reconnection on failure
let output = resilient_session.execute_resilient("important_command.sh")?;
println!("Command output: {}", output.stdout);

// Periodic health checks
let health = resilient_session.health_check();
match health {
    HealthStatus::Healthy => println!("Session healthy"),
    HealthStatus::Unhealthy { uptime, consecutive_failures } => {
        eprintln!("Warning: Session unhealthy (failures: {}, uptime: {:?})", 
            consecutive_failures, uptime);
    }
    HealthStatus::Skipped => println!("Health check skipped (too recent)"),
}

// Get resilience metrics
let metrics = resilient_session.resilience_metrics();
println!("Consecutive failures: {}", metrics.consecutive_failures);
println!("Auto reconnect enabled: {}", metrics.auto_reconnect_enabled);
```

### Resilient Connection Pool

```rust
use carpai::ssh::resilience::{ResilientConnectionPool, RetryPolicy};

// Create pool with built-in resilience
let mut pool = ResilientConnectionPool::new(
    20,  // max sessions
    RetryPolicy::default()
);

// Get or create session (handles reconnection automatically)
let _session = pool.get_or_create_session(
    "web-server-1",
    SshConfig::with_host("web1.example.com")
)?;

// Execute with automatic failover
let output = pool.execute_on("web-server-1", "systemctl status nginx")?;

// Health check all sessions
let health_results = pool.health_check_all();
for (key, status) in health_results {
    match status {
        HealthStatus::Healthy => println!("{}: ✓ Healthy", key),
        HealthStatus::Unreachable { .. } => println!("{}: ✗ Unhealthy", key),
        HealthStatus::Skipped => {}
    }
}

// Remove dead sessions
let dead = pool.evict_dead_sessions();
if !dead.is_empty() {
    println!("Evicted dead sessions: {:?}", dead);
}

// Pool statistics
let stats = pool.stats();
println!("Pool: {}/{} sessions ({} healthy)", 
    stats.total_sessions, stats.max_capacity, stats.healthy_sessions);
```

---

## Advanced Features

### Batch Operations

```rust
use carpai::ssh::enhanced::{BatchExecutor, SshManager};
use std::sync::Arc;

let manager = Arc::new(SshManager::new());
let batch = BatchExecutor::new(manager.clone());

// Sequential execution across hosts
let hosts = ["server1", "server2", "server3"];
let results = batch.execute_sequential(
    &hosts.map(|&h| h.to_string()).to_vec(),
    "uptime"
);

let summary = batch.get_summary(&results);
println!(
    "Completed: {}/{}, Success rate: {:.1}%, Avg duration: {:?}",
    summary.successful,
    summary.total_commands,
    summary.success_rate * 100.0,
    summary.average_duration
);

// Parallel execution (faster but uses more resources)
let parallel_results = batch.execute_parallel(
    hosts.iter().map(|&h| h.to_string()).collect(),
    "df -h | grep /dev/sda1"
);
```

### SSH Manager (Unified Interface)

```rust
use carpai::ssh::enhanced::SshManager;

let manager = SshManager::new();

// Connect using cached config from ~/.ssh/config
let session_id = manager.connect("production-server")?;

// Execute command (auto-manages connection pooling)
let output = manager.execute("production-server", "ps aux | grep node")?;

// File operations
manager.upload_file("production-server",
    &PathBuf::from("./build.tar.gz"),
    &PathBuf::from("/tmp/build.tar.gz")
)?;

// List configured hosts
let hosts = manager.list_hosts()?;
println!("Available hosts: {:?}", hosts);

// Validate SSH configuration
let warnings = manager.validate_config()?;
for warning in warnings {
    println!("⚠️  {}", warning);
}

// Export audit log
manager.export_audit_log(&PathBuf::from("ssh_operations.json"))?;

// Shutdown everything gracefully
manager.shutdown()?;
```

### Configuration Parser Deep Dive

```rust
use carpai::ssh::config::{ConfigParser, SshHostConfig};

// Parse entire ~/.ssh/config
let configs = ConfigParser::parse()?;

// Find specific host configuration
if let Some(host_config) = ConfigParser::find_host("my-alias")? {
    println!("Host: {}", host_config.host);
    println!("Hostname: {:?}", host_config.host_name);
    println!("User: {:?}", host_config.user);
    println!("Port: {:?}", host_config.port);
    println!("Identity files: {:?}", host_config.identity_file);
    println!("Local forwards: {:?}", host_config.local_forward);
    println!("Remote forwards: {:?}", host_config.remote_forward);
    println!("Dynamic forward: {:?}", host_config.dynamic_forward);
    println!("Proxy command: {:?}", host_config.proxy_command);
}

// Parse config content directly
let content = r#"
Host production
    HostName prod.example.com
    User deployer
    Port 2222
    IdentityFile ~/.ssh/prod_key
    LocalForward 8080 localhost:80
    ServerAliveInterval 60
"#;

let parsed_configs = ConfigParser::parse_content(content)?;
assert_eq!(parsed_configs.len(), 1);

// Validate configuration
let validation_warnings = ConfigParser::validate_config()?;
for warning in validation_warnings {
    println!("Config warning: {}", warning);
}
```

---

## Best Practices & Patterns

### 1. Resource Management (RAII Pattern)

```rust
fn ssh_operation() -> Result<(), String> {
    let config = SshConfig::with_host("server.com");
    let mut session = SshSession::new(config);
    
    session.connect()?;  // Early return on failure
    
    // ... perform operations ...
    
    session.disconnect()?;  // Explicit cleanup
    Ok(())
}
```

### 2. Error Handling Strategy

```rust
fn robust_execute(session: &mut SshSession, cmd: &str) -> Result<String, String> {
    match session.execute(cmd) {
        Ok(output) => {
            if output.exit_code == Some(0) {
                Ok(output.stdout)
            } else {
                Err(format!("Command failed (exit {:?}): {}", 
                    output.exit_code, output.stderr))
            }
        }
        Err(e) => {
            if !session.is_alive() {
                // Try to reconnect
                session.connect()
                    .and_then(|_| session.execute(cmd))
                    .map(|o| o.stdout)
            } else {
                Err(e)
            }
        }
    }
}
```

### 3. Performance Optimization Tips

- **Use connection pooling** for frequent short-lived operations
- **Enable compression** for high-latency connections
- **Use rsync** for large file transfers instead of SCP
- **Batch commands** using `&&` to reduce round-trips
- **Set appropriate timeouts** based on network conditions
- **Monitor pool utilization** to size correctly

### 4. Security Best Practices

- Always enable `strict_host_key_checking` in production
- Use **SSH keys** instead of passwords
- Implement **audit logging** for compliance
- Regularly rotate credentials
- Use **jump hosts** (bastions) for internal network access
- Enable **circuit breakers** to prevent cascading failures
- Monitor for **security violations** in audit logs

---

## Troubleshooting

### Common Issues

| Issue | Solution |
|-------|----------|
| "Connection refused" | Check SSH service is running, correct port |
| "Permission denied" | Verify key permissions (600) and ownership |
| "Host key verification failed" | Update known_hosts or disable strict checking (dev only) |
| "Connection timeout" | Increase connect_timeout, check network/firewall |
| "Too many authentication failures" | Check for correct identity file |
| "Broken pipe" | Use resilient session with auto-reconnect |

### Debug Mode

```rust
// Enable verbose SSH output for debugging
let mut config = SshConfig::with_host("debug-target");
// Add -v flag manually in build_ssh_args or use verbose mode

// Check connectivity
if !session.is_alive() {
    eprintln!("Session not alive, attempting reconnect...");
}
```

---

## API Reference Summary

### Core Types

- `SshConfig` - SSH connection configuration
- `SshSession` - Individual SSH session management
- `SshOutput` - Command execution result
- `SshConnectionPool` - Multi-session connection pooling

### Advanced Types

- `ResilientSshSession` - Auto-reconnecting session
- `SmartRetryHandler` - Intelligent retry logic
- `CircuitBreaker` - Failure cascade prevention
- `ResilientConnectionPool` - Resilient connection pool

### Utility Types

- `FileTransfer` - SCP/Rsync file operations
- `PortForwarder` - Port forwarding/tunneling
- `TunnelManager` - Multiple tunnel lifecycle
- `ConfigParser` - ~/.ssh/config parsing
- `SshAuditLogger` - Comprehensive audit logging
- `SshManager` - Unified high-level interface

### Enums & Constants

- `SessionState` - Connection state machine
- `ForwardType` - Port forwarding type
- `EventSeverity` - Audit event severity levels
- `SshEventType` - Audit event type classification
- `ReconnectStrategy` - Backoff strategies
- `ErrorClassification` - Error categorization

---

*Last updated: 2026-05-14*
*CarpAI SSH Module v1.0*
