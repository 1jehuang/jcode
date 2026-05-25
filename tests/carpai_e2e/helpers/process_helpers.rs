//! Process management helpers for E2E tests
//!
//! Provides utilities for starting/stopping server and CLI processes,
//! sending input, and reading output with timeout protection.

use anyhow::{Context, Result};
use std::process::{Child, Command, Stdio};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::time::timeout;
use std::sync::Arc;
use std::sync::Mutex;

const TEST_TIMEOUT_SECS: u64 = 60;
const PROCESS_START_TIMEOUT_SECS: u64 = 30;
const OUTPUT_READ_TIMEOUT_SECS: u64 = 10;

/// Manages a child process with automatic cleanup on drop
pub struct ManagedProcess {
    pub child: Child,
    pub name: String,
    start_time: Instant,
}

impl ManagedProcess {
    pub fn new(mut child: Child, name: String) -> Self {
        Self {
            child,
            name,
            start_time: Instant::now(),
        }
    }

    /// Check if process is still running
    pub fn is_running(&mut self) -> bool {
        match self.child.try_wait() {
            Ok(None) => true,
            _ => false,
        }
    }

    /// Get process uptime
    pub fn uptime(&self) -> Duration {
        self.start_time.elapsed()
    }

    /// Force kill the process
    pub fn kill(&mut self) -> Result<()> {
        self.child.kill()
            .with_context(|| format!("Failed to kill {} process", self.name))?;
        let status = self.child.wait()
            .with_context(|| format!("Failed to wait for {} process", self.name))?;
        tracing::info!("{} process exited with status: {}", self.name, status);
        Ok(())
    }
}

impl Drop for ManagedProcess {
    fn drop(&mut self) {
        if self.is_running() {
            let _ = self.kill();
        }
    }
}

/// Start the CarpAI server in background mode
///
/// # Arguments
/// * `config_path` - Path to server configuration file
/// * `port` - Port to listen on (0 for random available port)
///
/// # Returns
/// * `ManagedProcess` wrapping the server child process
pub async fn start_server(config_path: &PathBuf, port: u16) -> Result<ManagedProcess> {
    let mut cmd = Command::new("carpai-server");
    cmd.arg("--config")
       .arg(config_path)
       .arg("--port")
       .arg(port.to_string())
       .stdout(Stdio::piped())
       .stderr(Stdio::piped())
       .stdin(Stdio::null());

    let child = cmd.spawn()
        .context("Failed to spawn server process")?;

    let managed = ManagedProcess::new(child, "carpai-server".to_string());

    // Wait for server to be ready
    tokio::time::sleep(Duration::from_secs(2)).await;

    Ok(managed)
}

/// Stop a running server process
pub async fn stop_server(process: &mut ManagedProcess) -> Result<()> {
    if process.is_running() {
        process.kill()?;
    }
    Ok(())
}

/// Start the CarpAI CLI in local mode (interactive)
///
/// # Arguments
/// * `config_path` - Path to CLI configuration file
/// * `working_dir` - Working directory for the session
///
/// # Returns
/// * `ManagedProcess` with piped stdin/stdout for interaction
pub async fn start_cli_local(config_path: &PathBuf, working_dir: &PathBuf) -> Result<ManagedProcess> {
    let mut cmd = Command::new("carpai");
    cmd.arg("chat")
       .arg("--local")
       .arg("--config")
       .arg(config_path)
       .arg("--working-dir")
       .arg(working_dir)
       .env("TERM", "dumb")  // Disable TUI rendering for testing
       .env("CARPAI_TEST_MODE", "1")
       .stdout(Stdio::piped())
       .stderr(Stdio::piped())
       .stdin(Stdio::piped());

    let child = cmd.spawn()
        .context("Failed to spawn CLI process in local mode")?;

    Ok(ManagedProcess::new(child, "carpai-cli-local".to_string()))
}

/// Start the CarpAI CLI in remote mode (connecting to server)
///
/// # Arguments
/// * `server_addr` - Server address (e.g., "127.0.0.1:8080")
/// * `config_path` - Path to CLI configuration file
///
/// # Returns
/// * `ManagedProcess` with piped stdin/stdout for interaction
pub async fn start_cli_remote(server_addr: &str, config_path: &PathBuf) -> Result<ManagedProcess> {
    let mut cmd = Command::new("carpai");
    cmd.arg("chat")
       .arg("--remote")
       .arg("--server")
       .arg(server_addr)
       .arg("--config")
       .arg(config_path)
       .env("TERM", "dumb")
       .env("CARPAI_TEST_MODE", "1")
       .stdout(Stdio::piped())
       .stderr(Stdio::piped())
       .stdin(Stdio::piped());

    let child = cmd.spawn()
        .context("Failed to spawn CLI process in remote mode")?;

    Ok(ManagedProcess::new(child, "carpai-cli-remote".to_string()))
}

/// Send input text to a CLI process via stdin
///
/// # Arguments
/// * `process` - The CLI process to send input to
/// * `input` - Text to send (will append newline automatically)
pub async fn send_input(process: &mut Child, input: &str) -> Result<()> {
    use std::io::Write;

    let stdin = process.stdin.as_mut()
        .context("Process stdin not available")?;

    stdin.write_all(format!("{}\n", input).as_bytes())
        .context("Failed to write to process stdin")?;
    stdin.flush()
        .context("Failed to flush process stdin")?;

    Ok(())
}

/// Read output from a process stdout with timeout
///
/// # Arguments
/// * `process` - The process to read from
/// * `timeout_duration` - Maximum time to wait for output
///
/// # Returns
/// * Output text collected so far
pub async fn read_output(process: &mut Child, timeout_duration: Duration) -> Result<String> {
    use std::io::Read;

    let deadline = Instant::now() + timeout_duration;
    let mut output = String::new();

    while Instant::now() < deadline {
        if let Some(ref mut stdout) = process.stdout {
            let mut buffer = [0u8; 4096];
            match stdout.read(&mut buffer) {
                Ok(0) => break,  // EOF
                Ok(n) => {
                    let text = String::from_utf8_lossy(&buffer[..n]);
                    output.push_str(&text);
                    // Return early if we have substantial output
                    if output.len() > 100 {
                        return Ok(output);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(e) => return Err(e).context("Failed to read process output"),
            }
        } else {
            break;
        }
    }

    Ok(output)
}

/// Wait for a specific string to appear in process output
///
/// # Arguments
/// * `process` - The process to monitor
/// * `expected` - String to wait for
/// * `timeout_duration` - Maximum wait time
pub async fn wait_for_output(
    process: &mut Child,
    expected: &str,
    timeout_duration: Duration,
) -> Result<bool> {
    let deadline = Instant::now() + timeout_duration;
    let mut collected = String::new();

    while Instant::now() < deadline {
        let chunk = read_output(process, Duration::from_millis(100)).await?;
        collected.push_str(&chunk);

        if collected.contains(expected) {
            return Ok(true);
        }

        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    Ok(false)
}

/// Reserve a random TCP port for testing
pub fn reserve_port() -> Result<u16> {
    use std::net::TcpListener;

    let listener = TcpListener::bind(("127.0.0.1", 0))
        .context("Failed to bind to random port")?;
    let port = listener.local_addr()
        .context("Failed to get local address")?
        .port();
    drop(listener);  // Release the port immediately

    Ok(port)
}

/// Wait for a TCP port to become available (server ready)
pub async fn wait_for_port(port: u16, timeout: Duration) -> Result<()> {
    use tokio::net::TcpStream;

    let deadline = Instant::now() + timeout;

    while Instant::now() < deadline {
        match TcpStream::connect(("127.0.0.1", port)).await {
            Ok(_) => return Ok(()),
            Err(_) => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }

    anyhow::bail!("Port {} did not become available within {:?}", port, timeout)
}

/// Create a temporary directory for test data
pub fn create_temp_dir(prefix: &str) -> Result<PathBuf> {
    let temp_dir = std::env::temp_dir();
    let test_dir = temp_dir.join(format!(
        "{}-{}",
        prefix,
        SystemTimeTimestamp::new()
    ));

    std::fs::create_dir_all(&test_dir)
        .with_context(|| format!("Failed to create temp dir: {}", test_dir.display()))?;

    Ok(test_dir)
}

/// Clean up temporary directory
pub fn cleanup_temp_dir(path: &PathBuf) -> Result<()> {
    if path.exists() {
        std::fs::remove_dir_all(path)
            .with_context(|| format!("Failed to cleanup temp dir: {}", path.display()))?;
    }
    Ok(())
}

/// Generate unique timestamp-based identifier
struct SystemTimeTimestamp;

impl SystemTimeTimestamp {
    fn new() -> String {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_reserve_port_returns_valid_port() {
        let port = reserve_port().unwrap();
        assert!(port > 0 && port <= 65535);
    }

    #[tokio::test]
    async fn test_create_temp_dir_creates_directory() {
        let dir = create_temp_dir("test-e2e").unwrap();
        assert!(dir.exists());
        assert!(dir.is_dir());
        cleanup_temp_dir(&dir).unwrap();
    }

    #[tokio::test]
    async fn test_cleanup_temp_dir_removes_directory() {
        let dir = create_temp_dir("test-cleanup").unwrap();
        assert!(dir.exists());
        cleanup_temp_dir(&dir).unwrap();
        assert!(!dir.exists());
    }
}
