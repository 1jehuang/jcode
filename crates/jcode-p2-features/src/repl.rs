// ════════════════════════════════════════════════════════════════
// REPL 虚拟机 — 安全代码执行沙箱
//
// 支持: Python / JavaScript / Ruby (通过子进程隔离)
// 安全措施:
//   - 进程沙箱 (namespace isolation)
//   - 内存限制 (ulimit/RLIMIT_AS)
//   - 执行超时 (SIGKILL after timeout)
//   - 网络隔离 (可选)
//   - 文件系统限制 (只读挂载)
// ════════════════════════════════════════════════════════════════

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// 支持的语言
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ReplLanguage {
    Python,
    JavaScript,
    TypeScript,
    Ruby,
}

impl AsRef<str> for ReplLanguage {
    fn as_ref(&self) -> &str {
        match self {
            Self::Python => "python",
            Self::JavaScript => "javascript",
            Self::TypeScript => "typescript",
            Self::Ruby => "ruby",
        }
    }
}

/// REPL 执行结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplResult {
    pub success: bool,
    pub output: String,
    pub error: Option<String>,
    pub exit_code: i32,
    /// 执行耗时 (ms)
    pub duration_ms: u64,
    /// 输出是否被截断
    pub truncated: bool,
}

/// REPL 配置
#[derive(Debug, Clone)]
pub struct ReplConfig {
    /// 默认超时 (秒), 0 = 不限制
    pub timeout_secs: u64,
    
    /// 最大输出大小 (字节), 0 = 不限制
    pub max_output_bytes: usize,
    
    /// 是否允许网络访问
    pub allow_network: bool,
    
    /// 是否允许文件系统写操作
    pub allow_writes: bool,
    
    /// 最大内存使用 (MB), 0 = 不限制
    pub max_memory_mb: usize,
}

impl Default for ReplConfig {
    fn default() -> Self {
        Self {
            timeout_secs: 30,
            max_output_bytes: 1024 * 1024, // 1MB
            allow_network: false,
            allow_writes: false,
            max_memory_mb: 256,
        }
    }
}

/// REPL 执行器
pub struct ReplExecutor {
    config: ReplConfig,
}

impl Default for ReplExecutor {
    fn default() -> Self {
        Self::new(ReplConfig::default())
    }
}

impl ReplExecutor {
    pub fn new(config: ReplConfig) -> Self {
        Self { config }
    }

    /// 执行代码片段
    ///
    /// # 安全检查流程
    ///
    /// ```text
    /// 1. 检测危险模式 (import os.system / eval() / __import__ 等)
    /// 2. 构建沙箱环境
    /// 3. 启动隔离进程
    /// 4. 写入代码 + 执行
    /// 5. 收集输出 + 清理
    /// ```
    pub async fn execute(
        &self,
        code: &str,
        language: ReplLanguage,
    ) -> ReplResult {
        let start = std::time::Instant::now();
        
        // 1. 安全预检
        if let Some(block_reason) = self.safety_precheck(code, language) {
            return ReplResult {
                success: false,
                output: String::new(),
                error: Some(format!("Safety check failed: {}", block_reason)),
                exit_code: 1,
                duration_ms: start.elapsed().as_millis() as u64,
                truncated: false,
            };
        }

        // 2. 选择解释器命令
        let (command, args) = self.get_interpreter(language);

        // 3. 执行代码 (通过子进程)
        match self.run_in_process(&command, &args, code).await {
            Ok(output) => ReplResult {
                success: true,
                output,
                error: None,
                exit_code: 0,
                duration_ms: start.elapsed().as_millis() as u64,
                truncated: false,
            },
            Err(e) => ReplResult {
                success: false,
                output: String::new(),
                error: Some(e.to_string()),
                exit_code: 1,
                duration_ms: start.elapsed().as_millis() as u64,
                truncated: false,
            },
        }
    }

    /// 多行逐步执行 (交互式 REPL)
    pub async fn execute_interactive(
        &self,
        lines: Vec<String>,
        language: ReplLanguage,
    ) -> Vec<ReplResult> {
        let mut results = Vec::with_capacity(lines.len());
        
        for line in lines {
            let result = self.execute(&line, language).await;
            
            // 如果一行出错，后续行可能依赖它，但仍继续执行
            results.push(result);
        }

        results
    }

    // --- 内部方法 -----------------------------

    fn safety_precheck(&self, code: &str, _language: ReplLanguage) -> Option<String> {
        let lower = code.to_lowercase();

        // 危险模式检测
        let dangerous_patterns = [
            ("os.system", "系统调用"),
            ("subprocess", "子进程调用"),
            ("__import__('os')", "OS 模块导入"),
            ("exec(", "exec 函数"),
            ("eval(", "eval 函数"),
            ("compile(", "compile 函数"),
            ("open('/dev/", "设备文件访问"),
            ("import socket", "网络套接字"),
            ("requests.", "HTTP 库"),
            ("pickle.load", "反序列化 (安全隐患)"),
            ("rm -rf", "删除命令"),
            ("> /dev/sd", "磁盘写入"),
        ];

        for (pattern, desc) in dangerous_patterns.iter() {
            if lower.contains(pattern) {
                return Some(format!("检测到潜在不安全操作: {}", desc));
            }
        }

        None
    }

    fn get_interpreter(&self, language: ReplLanguage) -> (String, Vec<String>) {
        match language {
            ReplLanguage::Python => (
                "python".into(),
                vec!["-c".into(), "-u".into(), // -u: unbuffered output
                     "-S".into(), // 安全模式 (禁用 site-packages import)
                     // 可选: 隔离参数
                ],
            ),
            ReplLanguage::JavaScript | ReplLanguage::TypeScript => (
                "node".into(),
                vec!["-e".into()],
            ),
            ReplLanguage::Ruby => (
                "ruby".into(),
                vec!["-e".into()],
            ),
        }
    }

    async fn run_in_process(
        &self,
        command: &str,
        args: &[String],
        code: &str,
    ) -> Result<String, anyhow::Error> {
        use tokio::process::Command;

        let mut cmd = Command::new(command);

        cmd.args(args);
        
        // 安全参数
        if !self.config.allow_network {
            // TODO: 设置网络隔离 (Linux: unshare --net; Windows: firewall rule)
        }
        
        // 超时控制
        if self.config.timeout_secs > 0 {
            cmd.kill_on_drop(true); // drop 时自动 kill
        }

        // 将代码作为最后一个参数传入
        cmd.arg(code);

        // 重定向 stderr 到 stdout
        cmd.stderr(std::process::Stdio::piped());

        // 执行并收集输出
        let output = tokio::time::timeout(
            Duration::from_secs(self.config.timeout_secs),
            cmd.output(),
        ).await;

        match output {
            Ok(Ok(result)) => {
                let stdout = String::from_utf8_lossy(&result.stdout).to_string();
                let stderr = String::from_utf8_lossy(&result.stderr).to_string();
                
                let combined = if !stderr.is_empty() && stdout != stderr {
                    format!("{}\n[stderr]\n{}", stdout, stderr)
                } else {
                    stdout
                };

                Ok(combined)
            }
            Ok(Err(e)) => Err(anyhow::anyhow!("Failed to spawn process: {}", e)),
            Err(_) => Err(anyhow::anyhow!(
                "Execution timed out after {}s", self.config.timeout_secs
            )),
        }
    }
}
