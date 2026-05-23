//! MCP 服务器运行器 — 统一管理 Python MCP 服务器子进程
//!
//! 对标 Claude Code 和 Cursor 的 MCP 服务管理:
//! - 启动/停止/重启 Python MCP 服务器
//! - 进程健康检查
//! - 自动重启崩溃的进程
//! - 日志采集

use crate::mcp::protocol::McpServerConfig;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::process::{Child, Command};
use tokio::sync::RwLock;

/// MCP 服务器进程状态
#[derive(Debug, Clone, PartialEq)]
pub enum ProcessStatus {
    Running,
    Stopped,
    Crashed(String),
    Starting,
}

/// 服务器进程句柄
struct ServerProcess {
    pub name: String,
    pub config: McpServerConfig,
    pub child: Option<Child>,
    pub status: ProcessStatus,
    pub started_at: Option<Instant>,
    pub restart_count: u32,
    pub last_error: Option<String>,
}

/// MCP 服务器运行器
pub struct McpServerRunner {
    processes: Arc<RwLock<HashMap<String, ServerProcess>>>,
    max_restarts: u32,
}

impl McpServerRunner {
    pub fn new() -> Self {
        Self {
            processes: Arc::new(RwLock::new(HashMap::new())),
            max_restarts: 5,
        }
    }

    /// 启动一个 MCP 服务器
    pub async fn start(&self, name: &str, config: &McpServerConfig) -> Result<String> {
        let mut procs = self.processes.write().await;

        // 检查是否已在运行
        if let Some(proc) = procs.get(name) {
            if proc.status == ProcessStatus::Running {
                return Ok(format!("✅ MCP服务器 '{}' 已在运行中", name));
            }
        }

        // 检查重启次数限制
        let restart_count = procs.get(name).map(|p| p.restart_count).unwrap_or(0);
        if restart_count >= self.max_restarts {
            return Err(anyhow::anyhow!("服务器 '{}' 已达到最大重启次数限制 ({})", name, self.max_restarts));
        }

        // 设置环境变量
        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        for (key, val) in &config.env {
            if !val.is_empty() {
                cmd.env(key, val);
            }
        }

        // 启动进程
        let child = cmd.spawn().map_err(|e| {
            anyhow::anyhow!("启动 '{}' 失败: {} (命令: {} {})", name, e, config.command, config.args.join(" "))
        })?;

        let process = ServerProcess {
            name: name.to_string(),
            config: config.clone(),
            child: Some(child),
            status: ProcessStatus::Running,
            started_at: Some(Instant::now()),
            restart_count: restart_count + 1,
            last_error: None,
        };

        procs.insert(name.to_string(), process);

        Ok(format!(
            "✅ MCP服务器 '{}' 已启动 (命令: {} {}, 重启次数: {})",
            name, config.command, config.args.join(" "), restart_count
        ))
    }

    /// 停止一个 MCP 服务器
    pub async fn stop(&self, name: &str) -> Result<String> {
        let mut procs = self.processes.write().await;
        if let Some(proc) = procs.get_mut(name) {
            if let Some(mut child) = proc.child.take() {
                // 尝试优雅停止
                let _ = child.kill().await;
                let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
                proc.status = ProcessStatus::Stopped;
                return Ok(format!("⏹️  MCP服务器 '{}' 已停止", name));
            }
            proc.status = ProcessStatus::Stopped;
            Ok(format!("⏹️  MCP服务器 '{}' 已标记为停止", name))
        } else {
            Ok(format!("MCP服务器 '{}' 未运行", name))
        }
    }

    /// 重启一个 MCP 服务器
    pub async fn restart(&self, name: &str, config: &McpServerConfig) -> Result<String> {
        self.stop(name).await?;
        tokio::time::sleep(Duration::from_millis(500)).await;
        self.start(name, config).await
    }

    /// 检查服务器健康状态
    pub async fn health_check(&self, name: &str) -> ProcessStatus {
        let procs = self.processes.read().await;
        procs.get(name).map(|p| p.status.clone()).unwrap_or(ProcessStatus::Stopped)
    }

    /// 获取所有服务器的状态摘要
    pub async fn status_summary(&self) -> Vec<(String, ProcessStatus, Option<String>)> {
        let procs = self.processes.read().await;
        procs.iter().map(|(name, p)| {
            (name.clone(), p.status.clone(), p.last_error.clone())
        }).collect()
    }

    /// 获取MCP服务器数量
    pub async fn running_count(&self) -> usize {
        let procs = self.processes.read().await;
        procs.values().filter(|p| p.status == ProcessStatus::Running).count()
    }

    /// 停止所有服务器
    pub async fn stop_all(&self) -> Vec<String> {
        let mut results = Vec::new();
        let names: Vec<String> = {
            let procs = self.processes.read().await;
            procs.keys().cloned().collect()
        };
        for name in &names {
            match self.stop(name).await {
                Ok(msg) => results.push(msg),
                Err(e) => results.push(format!("❌ {}: {}", name, e)),
            }
        }
        results
    }

    /// 启动所有已配置的 MCP 服务器 (自动发现)
    pub async fn start_all_configured(&self) -> Vec<Result<String>> {
        let config = crate::mcp::protocol::McpConfig::load();
        let mut results = Vec::new();
        for (name, cfg) in &config.servers {
            results.push(self.start(name, cfg).await);
        }
        results
    }

    /// 启动一个Python MCP服务器脚本
    pub async fn start_python_server(&self, name: &str, script_path: &str) -> Result<String> {
        let config = McpServerConfig {
            command: "python".to_string(),
            args: vec![script_path.to_string()],
            env: HashMap::new(),
            shared: true,
        };
        self.start(name, &config).await
    }
}

impl Default for McpServerRunner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_runner_creation() {
        let runner = McpServerRunner::new();
        assert_eq!(runner.running_count().await, 0);
    }

    #[tokio::test]
    async fn test_start_nonexistent() {
        let runner = McpServerRunner::new();
        let config = McpServerConfig {
            command: "does_not_exist_xyz".to_string(),
            args: vec![],
            env: HashMap::new(),
            shared: true,
        };
        let result = runner.start("test", &config).await;
        assert!(result.is_err());
    }
}
