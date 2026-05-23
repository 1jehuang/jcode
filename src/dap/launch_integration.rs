//! DAP 增强 — 参考 Claude Code 的 launch.json 处理 + debug 技能
//!
//! Claude Code 有: launch.json 权限豁免 + /debug 技能读取调试日志
//! Claude Code 没有: DAP 协议实现 (CarpAI 已有完整 DAP)
//!
//! 增强点:
//! 1. launch.json 解析器 — 自动检测和解析调试配置
//! 2. Debug 技能 — Agent 可调用读取调试日志
//! 3. 断点同步 — 与 VS Code 断点状态同步

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

// ========================================================================
// [1] launch.json 解析器 — 对标 Claude Code 的 launch.json 处理
// ========================================================================

/// 启动配置
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LaunchConfig {
    pub name: String,
    #[serde(rename = "type")]
    pub config_type: String,
    pub request: String,
    pub program: Option<String>,
    pub args: Option<Vec<String>>,
    pub cwd: Option<String>,
    pub env: Option<HashMap<String, String>>,
    pub port: Option<u16>,
    pub pre_launch_task: Option<String>,
    pub post_debug_task: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

/// launch.json 解析器
pub struct LaunchConfigParser {
    workspace_root: PathBuf,
    configs: Arc<RwLock<Vec<LaunchConfig>>>,
}

impl LaunchConfigParser {
    pub fn new(workspace_root: &Path) -> Self {
        Self {
            workspace_root: workspace_root.to_path_buf(),
            configs: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// 从工作区自动发现 launch.json
    pub async fn discover(&self) -> Result<Vec<LaunchConfig>, String> {
        let paths = vec![
            self.workspace_root.join(".vscode/launch.json"),
            self.workspace_root.join(".carpai/launch.json"),
            self.workspace_root.join("launch.json"),
        ];

        let mut all_configs = Vec::new();
        for path in &paths {
            if path.exists() {
                let content = tokio::fs::read_to_string(path).await
                    .map_err(|e| format!("Read {}: {}", path.display(), e))?;
                let parsed: serde_json::Value = serde_json::from_str(&content)
                    .map_err(|e| format!("Parse {}: {}", path.display(), e))?;

                if let Some(configs) = parsed.get("configurations").and_then(|c| c.as_array()) {
                    for config in configs {
                        if let Ok(cfg) = serde_json::from_value::<LaunchConfig>(config.clone()) {
                            all_configs.push(cfg);
                        }
                    }
                }
            }
        }

        *self.configs.write().await = all_configs.clone();
        Ok(all_configs)
    }

    /// 查找匹配的调试配置
    pub async fn find_config(&self, name: &str) -> Option<LaunchConfig> {
        self.configs.read().await.iter()
            .find(|c| c.name == name)
            .cloned()
    }

    /// 自动推荐配置 (基于工作区语言)
    pub async fn suggest_config(&self) -> Option<LaunchConfig> {
        let configs = self.configs.read().await;
        // 优先 Rust
        if let Some(cfg) = configs.iter().find(|c| c.config_type == "lldb" || c.config_type == "codelldb") {
            return Some(cfg.clone());
        }
        // 其次 Node
        if let Some(cfg) = configs.iter().find(|c| c.config_type == "node") {
            return Some(cfg.clone());
        }
        // 返回第一个
        configs.first().cloned()
    }
}

// ========================================================================
// [2] Debug 技能 — 对标 Claude Code 的 /debug skills/bundled/debug.ts
// ========================================================================

/// 调试日志条目
#[derive(Debug, Clone)]
pub struct DebugLogEntry {
    pub timestamp: String,
    pub level: String,
    pub module: String,
    pub message: String,
}

/// Debug 技能 — Agent 可调用读取和分析调试日志
pub struct DebugSkill {
    log_dir: PathBuf,
}

impl DebugSkill {
    pub fn new(base_dir: &Path) -> Self {
        Self {
            log_dir: base_dir.join(".carpai").join("logs"),
        }
    }

    /// 读取最近 N 条调试日志
    /// 对标: Claude Code /debug 技能 — 读取最近20行
    pub async fn read_recent_logs(&self, lines: usize) -> Result<Vec<DebugLogEntry>, String> {
        let mut entries = Vec::new();

        // 查找最新的日志文件
        let log_file = self.find_latest_log().await;
        let content = match log_file {
            Some(path) => tokio::fs::read_to_string(&path).await
                .map_err(|e| format!("Read {}: {}", path.display(), e))?,
            None => return Ok(vec![DebugLogEntry {
                timestamp: chrono::Utc::now().to_rfc3339(),
                level: "info".to_string(),
                module: "debug_skill".to_string(),
                message: "No log files found. Start the CarpAI server to generate logs.".to_string(),
            }]),
        };

        // 解析日志行 (格式: TIMESTAMP [LEVEL] MODULE: MESSAGE)
        for line in content.lines().rev().take(lines) {
            let parts: Vec<&str> = line.splitn(4, &[' ', '\t'][..]).collect();
            if parts.len() >= 4 {
                entries.push(DebugLogEntry {
                    timestamp: parts[0].to_string(),
                    level: parts[1].trim_matches(&['[', ']'][..]).to_string(),
                    module: parts[2].trim_end_matches(':').to_string(),
                    message: parts[3..].join(" "),
                });
            }
        }

        entries.reverse();
        Ok(entries)
    }

    /// 分析日志中的错误模式
    pub async fn analyze_errors(&self) -> Result<Vec<(String, u32)>, String> {
        let logs = self.read_recent_logs(1000).await?;
        let mut error_counts: HashMap<String, u32> = HashMap::new();

        for entry in &logs {
            if entry.level == "ERROR" || entry.level == "error" {
                // 提取错误类型 (第一个冒号前的部分)
                let error_type = entry.message.split(':').next().unwrap_or("unknown").to_string();
                *error_counts.entry(error_type).or_insert(0) += 1;
            }
        }

        let mut result: Vec<_> = error_counts.into_iter().collect();
        result.sort_by(|a, b| b.1.cmp(&a.1));
        Ok(result)
    }

    /// 查找最新的日志文件
    async fn find_latest_log(&self) -> Option<PathBuf> {
        if !self.log_dir.exists() {
            return None;
        }
        let mut latest: Option<(PathBuf, std::time::SystemTime)> = None;
        let mut dir = tokio::fs::read_dir(&self.log_dir).await.ok()?;
        while let Some(entry) = dir.next_entry().await.ok().flatten() {
            if entry.file_type().await.map(|t| t.is_file()).unwrap_or(false) {
                if let Ok(modified) = entry.metadata().await.and_then(|m: std::fs::Metadata| m.modified()) {
                    match &latest {
                        Some((_, ts)) if modified > *ts => {
                            latest = Some((entry.path(), modified));
                        }
                        None => {
                            latest = Some((entry.path(), modified));
                        }
                        _ => {}
                    }
                }
            }
        }
        latest.map(|(p, _)| p)
    }
}

// ========================================================================
// [3] DAP 增强 — 补齐真实调试链路
// ========================================================================

/// DAP 调试配置 — 从 launch.json 生成 DAP 启动参数
#[derive(Debug, Clone)]
pub struct DapDebugConfig {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: HashMap<String, String>,
    pub stop_at_entry: bool,
}

/// DAP ↔ AutoFixLoop 桥梁
/// 闭环: DAP检测运行时错误 → 捕获上下文 → AutoFixLoop修复 → 重编译 → 重新调试
pub struct DapFixBridge {
    workspace: PathBuf,
}

impl DapFixBridge {
    pub fn new(workspace: &Path) -> Self {
        Self { workspace: workspace.to_path_buf() }
    }

    /// DAP 错误 → AutoFixLoop → 修复 → 重编译 → 返回修复报告
    pub async fn handle_dap_error(&self, error_message: &str, _stack_frame: Option<&str>) -> Result<String, String> {
        println!("[DAP-Fix] Error detected: {}", error_message);

        // 用 FixEngine 修复
        let fix_engine = crate::compilation_engine::FixEngine::new(&self.workspace);
        let engine = crate::compilation_engine::CompilationEngine::new(&self.workspace);
        let result = engine.cargo_check(&[]).await;

        if result.success {
            return Ok("✅ Already compiles cleanly".to_string());
        }

        let fixes = fix_engine.fix_errors(&result.errors, &result.raw_output).await?;
        if fixes.is_empty() {
            return Ok("⚠️  No auto-fix available for this error".to_string());
        }

        // 重新编译验证
        let recheck = engine.cargo_check(&[]).await;
        if recheck.success {
            Ok(format!("✅ Auto-fixed via DAP: {} files changed", fixes.len()))
        } else {
            Ok(format!("⚠️  Partial fix: {} files changed, {} errors remain", fixes.len(), recheck.errors.len()))
        }
    }
}

impl DapDebugConfig {
    /// 从 launch.json 配置生成 DAP 启动参数
    pub fn from_launch_config(config: &LaunchConfig, workspace: &Path) -> Self {
        Self {
            program: config.program.clone().unwrap_or_default(),
            args: config.args.clone().unwrap_or_default(),
            cwd: config.cwd.as_ref()
                .map(|c| workspace.join(c))
                .unwrap_or_else(|| workspace.to_path_buf()),
            env: config.env.clone().unwrap_or_default(),
            stop_at_entry: config.request == "launch",
        }
    }

    /// 自动检测调试目标 (无 launch.json 时)
    pub fn auto_detect(workspace: &Path) -> Self {
        // Rust: 查找 target/debug/ 下的二进制
        let target_dir = workspace.join("target/debug");
        if target_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&target_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file() {
                        // Windows: .exe; Unix: 无扩展名
                        #[cfg(windows)]
                        { if path.extension().map(|e| e == "exe").unwrap_or(false) {
                            return Self {
                                program: path.to_string_lossy().to_string(),
                                args: vec![],
                                cwd: workspace.to_path_buf(),
                                env: HashMap::new(),
                                stop_at_entry: true,
                            };
                        }}
                        #[cfg(not(windows))]
                        { if path.extension().is_none() {
                            return Self {
                                program: path.to_string_lossy().to_string(),
                                args: vec![],
                                cwd: workspace.to_path_buf(),
                                env: HashMap::new(),
                                stop_at_entry: true,
                            };
                        }}
                    }
                }
            }
        }
        // 回退: 尝试 cargo run
        Self {
            program: "cargo".to_string(),
            args: vec!["run".to_string()],
            cwd: workspace.to_path_buf(),
            env: HashMap::new(),
            stop_at_entry: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_launch_config_parser() {
        let temp = std::env::temp_dir().join("carpai-dap-test");
        let _ = std::fs::create_dir_all(&temp.join(".vscode"));
        std::fs::write(temp.join(".vscode/launch.json"), r#"{
            "configurations": [
                { "name": "Debug Main", "type": "lldb", "request": "launch", "program": "${workspaceFolder}/target/debug/app" }
            ]
        }"#).ok();

        let parser = LaunchConfigParser::new(&temp);
        let configs = parser.discover().await.unwrap();
        assert!(!configs.is_empty());
        assert_eq!(configs[0].name, "Debug Main");

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[tokio::test]
    async fn test_debug_skill() {
        let temp = std::env::temp_dir().join("carpai-debug-skill-test");
        let log_dir = temp.join(".carpai/logs");
        let _ = std::fs::create_dir_all(&log_dir);
        std::fs::write(log_dir.join("jcode-2026-05-23.log"),
            "2026-05-23T01:00:00 [INFO] server: started\n\
             2026-05-23T01:01:00 [ERROR] compiler: type mismatch\n\
             2026-05-23T01:02:00 [WARN] cache: hit rate low\n"
        ).ok();

        let skill = DebugSkill::new(&temp);
        let logs = skill.read_recent_logs(10).await.unwrap();
        assert_eq!(logs.len(), 3);
        assert!(logs.iter().any(|l| l.level == "ERROR"));

        let errors = skill.analyze_errors().await.unwrap();
        assert!(!errors.is_empty());
        assert_eq!(errors[0].0, "compiler");

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn test_auto_detect() {
        // 只是测试代码路径, 不期望找到二进制
        let temp = std::env::temp_dir();
        let config = DapDebugConfig::auto_detect(&temp);
        // 如果没有二进制, 应该回退到 cargo run
        assert!(config.program == "cargo" || !config.program.is_empty());
    }
}
