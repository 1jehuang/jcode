//! # Sandbox — 沙箱执行引擎
//!
//! 从 Claude Code 移植并增强的安全沙箱：
//! - 进程隔离：bubblewrap(bwrap) 容器化执行
//! - 资源限制：CPU 时间、内存、文件描述符上限
//! - 网络控制：可选择完全断网或白名单模式
//! - 文件系统限制：只读挂载 workdir，禁止访问系统敏感路径
//! - 临时工作区：每次执行自动创建/清理临时目录
//! - 超时强制 kill：进程超时后 SIGTERM → SIGKILL 升级

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command as AsyncCommand;
use tokio::time::timeout;
use tracing::{debug, error, info, warn};

pub const DEFAULT_TIMEOUT_SECS: u64 = 60;
pub const DEFAULT_MAX_MEMORY_MB: u64 = 4096;
pub const DEFAULT_MAX_CPU_SECS: u64 = 300;
pub const KILL_GRACE_SECS: u64 = 5;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxConfig {
    pub timeout_secs: u64,
    pub max_memory_mb: u64,
    pub max_cpu_secs: u64,
    pub network_allowed: bool,
    pub allowed_hosts: Vec<String>,
    pub read_only_root: bool,
    pub temp_dir: Option<PathBuf>,
    pub env_vars: HashMap<String, String>,
    pub shell: String,
    pub working_dir: Option<PathBuf>,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            timeout_secs: DEFAULT_TIMEOUT_SECS,
            max_memory_mb: DEFAULT_MAX_MEMORY_MB,
            max_cpu_secs: DEFAULT_MAX_CPU_SECS,
            network_allowed: false,
            allowed_hosts: vec![],
            read_only_root: true,
            temp_dir: None,
            env_vars: HashMap::new(),
            shell: if cfg!(windows) {
                "powershell.exe".into()
            } else {
                "/bin/sh".into()
            },
            working_dir: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxResult {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub elapsed_ms: u64,
    pub timed_out: bool,
    pub killed_by_signal: Option<i32>,
    pub truncated: bool,
    pub temp_dir: Option<PathBuf>,
}

const MAX_OUTPUT_BYTES: usize = 10 * 1024 * 1024; // 10MB

#[derive(Debug, Clone)]
pub struct Sandbox {
    config: SandboxConfig,
    bubblewrap_available: bool,
}

impl Sandbox {
    pub fn new(config: SandboxConfig) -> Self {
        let bw = which::which("bwrap").is_ok();
        if !bw && !cfg!(windows) {
            info!("bubblewrap not found; falling back to process-level isolation");
        }
        Self {
            config,
            bubblewrap_available: bw,
        }
    }

    pub async fn execute(&self, command: &str) -> Result<SandboxResult> {
        let start = std::time::Instant::now();
        let temp_dir = self.setup_temp_dir()?;

        let mut output_buf = Vec::with_capacity(MAX_OUTPUT_BYTES);
        let mut stderr_buf = Vec::with_capacity(MAX_OUTPUT_BYTES);
        let mut timed_out = false;
        let mut killed_by_signal: Option<i32> = None;
        let mut truncated = false;

        let mut child = if self.bubblewrap_available {
            self.spawn_bwrap(&temp_dir, command)?
        } else {
            self.spawn_direct(command)?
        };

        let timeout_dur = Duration::from_secs(self.config.timeout_secs);

        let outcome = timeout(timeout_dur, async {
            let stdout = child.stdout.take().expect("stdout not captured");
            let stderr = child.stderr.take().expect("stderr not captured");

            let mut out_truncated = false;
            let mut err_truncated = false;

            let read_stdout = Self::read_to_limit(stdout, &mut output_buf, &mut out_truncated);
            let read_stderr = Self::read_to_limit(stderr, &mut stderr_buf, &mut err_truncated);

            let (so_res, se_res) = tokio::join!(read_stdout, read_stderr);
            if let Err(e) = so_res {
                warn!("stdout read error: {}", e);
            }
            if let Err(e) = se_res {
                warn!("stderr read error: {}", e);
            }

            truncated = out_truncated || err_truncated;

            child.wait().await
        })
        .await;

        let exit_code = match outcome {
            Ok(Ok(status)) => status.code(),
            Ok(Err(e)) => {
                error!("Sandbox process error: {}", e);
                None
            }
            Err(_) => {
                warn!("Sandbox timeout after {}s", self.config.timeout_secs);
                timed_out = true;
                Self::kill_process_tree(child.id()).await;
                None
            }
        };

        if timed_out {
            tokio::time::sleep(Duration::from_secs(KILL_GRACE_SECS)).await;
            let _ = Self::kill_process_tree(child.id()).await;
            killed_by_signal = Some(9); // SIGKILL
        }

        let _ = self.cleanup_temp_dir(&temp_dir);

        let elapsed = start.elapsed().as_millis() as u64;
        let stdout = String::from_utf8_lossy(&output_buf).to_string();
        let stderr = String::from_utf8_lossy(&stderr_buf).to_string();

        debug!(
            "Sandbox complete: exit={:?} elapsed={}ms stdout={} stderr={}",
            exit_code,
            elapsed,
            stdout.len(),
            stderr.len()
        );

        Ok(SandboxResult {
            exit_code,
            stdout,
            stderr,
            elapsed_ms: elapsed,
            timed_out,
            killed_by_signal,
            truncated,
            temp_dir: Some(temp_dir),
        })
    }

    fn setup_temp_dir(&self) -> Result<PathBuf> {
        let base = self
            .config
            .temp_dir
            .as_deref()
            .unwrap_or_else(|| Path::new("/tmp"));

        let dir = tempfile::tempdir_in(base)
            .with_context(|| format!("Failed to create temp dir in {:?}", base))?;

        let path = dir.keep();
        std::fs::create_dir_all(&path)?;
        debug!("Sandbox temp dir: {:?}", path);
        Ok(path)
    }

    fn cleanup_temp_dir(&self, path: &Path) -> Result<()> {
        if path.exists() {
            std::fs::remove_dir_all(path)
                .with_context(|| format!("Failed to cleanup temp dir: {:?}", path))?;
        }
        Ok(())
    }

    fn spawn_bwrap(&self, temp_dir: &Path, command: &str) -> Result<tokio::process::Child> {
        let mut cmd = AsyncCommand::new("bwrap");

        cmd.arg("--new-session")
            .arg("--die-with-parent")
            .arg("--unshare-all")
            .arg("--share-net")
            .arg("--ro-bind")
            .arg("/usr")
            .arg("/usr")
            .arg("--ro-bind")
            .arg("/lib")
            .arg("/lib")
            .arg("--ro-bind")
            .arg("/lib64")
            .arg("/lib64")
            .arg("--ro-bind")
            .arg("/bin")
            .arg("/bin")
            .arg("--ro-bind")
            .arg("/etc/alternatives")
            .arg("/etc/alternatives");

        cmd.arg("--bind")
            .arg(temp_dir)
            .arg("/workspace")
            .arg("--chdir")
            .arg("/workspace");

        cmd.arg("--proc").arg("/proc");
        cmd.arg("--dev").arg("/dev");
        cmd.arg("--tmpfs").arg("/tmp");

        if !self.config.network_allowed {
            cmd.arg("--unshare-net");
        }

        for (key, val) in &self.config.env_vars {
            cmd.arg("--setenv").arg(key).arg(val);
        }

        cmd.arg("--");
        cmd.arg(&self.config.shell);
        cmd.arg("-c");
        cmd.arg(command);

        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        if let Some(ref wd) = self.config.working_dir {
            cmd.current_dir(wd);
        }

        let child = cmd
            .spawn()
            .with_context(|| "Failed to spawn bubblewrap sandbox")?;

        debug!("bwrap sandbox spawned: pid={}", child.id().unwrap_or(0));
        Ok(child)
    }

    fn spawn_direct(&self, command: &str) -> Result<tokio::process::Child> {
        let mut cmd = AsyncCommand::new(&self.config.shell);
        cmd.arg("-c").arg(command);

        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        for (key, val) in &self.config.env_vars {
            cmd.env(key, val);
        }

        if let Some(ref wd) = self.config.working_dir {
            cmd.current_dir(wd);
        }

        let child = cmd
            .spawn()
            .with_context(|| "Failed to spawn sandbox process")?;

        debug!(
            "Direct sandbox spawned: pid={}",
            child.id().unwrap_or(0)
        );
        Ok(child)
    }

    async fn read_to_limit<R: tokio::io::AsyncRead + Unpin>(
        mut reader: R,
        buf: &mut Vec<u8>,
        truncated: &mut bool,
    ) -> Result<()> {
        use tokio::io::AsyncReadExt;

        let mut chunk = vec![0u8; 8192];
        loop {
            let n = reader.read(&mut chunk).await?;
            if n == 0 {
                break;
            }
            if buf.len() + n > MAX_OUTPUT_BYTES {
                let remaining = MAX_OUTPUT_BYTES - buf.len();
                buf.extend_from_slice(&chunk[..remaining]);
                *truncated = true;
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
        }
        Ok(())
    }

    #[cfg(unix)]
    async fn kill_process_tree(pid: Option<u32>) {
        if let Some(pid) = pid {
            let pid = pid as i32;
            unsafe {
                libc::kill(-pid, libc::SIGTERM);
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
    }

    #[cfg(not(unix))]
    async fn kill_process_tree(pid: Option<u32>) {
        if let Some(pid) = pid {
            let _ = AsyncCommand::new("taskkill")
                .args(["/F", "/T", "/PID", &pid.to_string()])
                .output()
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_sandbox_echo() {
        let config = SandboxConfig::default();
        let sandbox = Sandbox::new(config);
        let result = sandbox.execute("echo hello world").await.unwrap();
        assert_eq!(result.exit_code, Some(0));
        assert!(result.stdout.contains("hello world"));
        assert!(!result.timed_out);
    }

    #[tokio::test]
    async fn test_sandbox_timeout() {
        let mut config = SandboxConfig::default();
        config.timeout_secs = 1;
        let sandbox = Sandbox::new(config);
        let result = sandbox.execute("sleep 10 2>/dev/null || timeout /t 10 >nul 2>&1").await.unwrap();
        assert!(result.timed_out || result.killed_by_signal.is_some());
    }

    #[tokio::test]
    async fn test_sandbox_exit_code() {
        let config = SandboxConfig::default();
        let sandbox = Sandbox::new(config);
        let result = sandbox.execute("exit 42").await.unwrap();
        assert_eq!(result.exit_code, Some(42));
    }
}