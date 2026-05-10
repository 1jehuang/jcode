// ════════════════════════════════════════════════════════════════
// Windows PowerShell 集成桥接
//
// 提供:
//   - PowerShell 命令执行 (通过 pwsh.exe 或 powershell.exe)
//   - 输出对象解析 (JSON 序列化)
//   - 模块管理 (Import-Module)
//   - 跨平台支持 (pwsh = PowerShell Core 7+)
//
// 与 BashTool 的关系:
//   BashTool → Linux/WSL/Git Bash
//   PowerShellBridge → Windows 原生 cmdlet / .NET 对象
// ════════════════════════════════════════════════════════════════

use serde::{Deserialize, Serialize};

/// PowerShell 执行结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PsResult {
    pub success: bool,
    /// stdout 文本输出
    pub output: String,
    /// stderr / 错误流
    pub error: Option<String>,
    /// 解析后的对象 (如果使用了 ConvertTo-Json)
    pub objects: Vec<serde_json::Value>,
    /// 退出码
    pub exit_code: i32,
    /// 耗时 (ms)
    pub duration_ms: u64,
}

/// PowerShell 配置
#[derive(Debug, Clone)]
pub struct PsConfig {
    /// 使用 PowerShell Core (跨平台) 还是 Windows PowerShell
    pub use_pwsh: bool,
    
    /// 额外模块路径
    pub module_paths: Vec<String>,

    /// 初始化脚本 (每次执行前运行)
    pub init_script: Option<String>,

    /// 执行策略
    pub execution_policy: ExecutionPolicy,

    /// 超时 (秒), 0=无限制
    pub timeout_secs: u64,
}

impl Default for PsConfig {
    fn default() -> Self {
        // 自动检测: 优先使用 pwsh (PowerShell Core)
        let use_pwsh = which::which("pwsh").is_ok();
        
        Self {
            use_pwsh,
            module_paths: vec![],
            init_script: None,
            execution_policy: ExecutionPolicy::RemoteSigned,
            timeout_secs: 120,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionPolicy {
    Restricted,
    AllSigned,
    RemoteSigned,
    Unrestricted,
    Bypass,
}

/// PowerShell Bridge — 安全的 PS 命令执行入口
pub struct PowerShellBridge {
    config: PsConfig,
}

impl Default for PowerShellBridge {
    fn default() -> Self { Self::new() }
}

impl PowerShellBridge {
    pub fn new() -> Self {
        Self { config: PsConfig::default() }
    }

    pub fn with_config(config: PsConfig) -> Self { Self { config } }

    /// 执行 PowerShell 命令
    ///
    /// # 示例
    ///
    /// ```ignore
    /// let bridge = PowerShellBridge::new();
    /// let result = bridge.execute("Get-Process | Select-Object -First 5 | ConvertTo-Json").await?;
    /// ```
    pub async fn execute(&self, script: &str) -> Result<PsResult, String> {
        let start = std::time::Instant::now();

        // 确定可执行文件
        let exe = if self.config.use_pwsh { "pwsh" } else { "powershell" };

        // 构建完整命令 (包装在 JSON 序列化中以便解析输出)
        let full_script = format!(
            "$ProgressPreference='SilentlyContinue'; {}; $OutputEncoding=[System.Text.Encoding]::UTF8; {}",
            if let Some(init) = &self.config.init_script {
                init.as_str()
            } else {
                ""
            },
            script
        );

        // 将命令包装为 JSON-safe 输出格式
        let wrapped = format!(
            "try {{ {} | ConvertTo-Json -Depth 10 -Compress }} catch {{ Write-Error $_.Exception.Message; exit 1 }}",
            full_script
        );

        let mut cmd = tokio::process::Command::new(exe);
        cmd.arg("-NoProfile")
           .arg("-NonInteractive")
           .arg("-Command")
           .arg(&wrapped)
           .stdout(std::process::Stdio::piped())
           .stderr(std::process::Stdio::piped())
           .kill_on_drop(true);

        // 设置超时
        let result = if self.config.timeout_secs > 0 {
            tokio::time::timeout(std::time::Duration::from_secs(self.config.timeout_secs), cmd.output()).await
        } else {
            Ok(cmd.output().await)
        };

        match result {
            Ok(output) => {
                let output = output.map_err(|e| e.to_string())?;
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                let exit_code_raw = output.status.code();

                // 尝试解析 stdout 为 JSON 数组
                let objects: Vec<serde_json::Value> = if !stdout.trim().is_empty() {
                    match serde_json::from_str(&stdout) {
                        Ok(serde_json::Value::Array(arr)) => arr.iter().cloned().collect(),
                        Ok(serde_json::Value::Object(obj)) => vec![serde_json::json!(obj)],
                        Ok(_) | Err(_) => vec![], // 其他JSON类型或解析错误, 作为纯文本处理
                    }
                } else { vec![] };

                Ok(PsResult {
                    success: exit_code_raw == Some(0),
                    output: if objects.is_empty() { stdout.clone() } else { String::new() },
                    error: if !stderr.trim().is_empty() { Some(stderr) } else { None },
                    objects,
                    exit_code: exit_code_raw.unwrap_or(-1),
                    duration_ms: start.elapsed().as_millis() as u64,
                })
            }
            Err(e) => {
                // 超时或执行失败
                if e.to_string().contains("timed out") || e.to_string().contains("deadline") {
                    Ok(PsResult {
                        success: false,
                        output: String::new(),
                        error: Some(format!("PowerShell execution timed out after {}s", self.config.timeout_secs)),
                        objects: vec![],
                        exit_code: -1,
                        duration_ms: start.elapsed().as_millis() as u64,
                    })
                } else {
                    Err(format!("Failed to spawn {}: {}", exe, e))
                }
            }
        }
    }

    /// 执行并获取单个标量值
    pub async fn execute_scalar<T: serde::de::DeserializeOwned>(&self, script: &str) -> Result<T, String> {
        let result = self.execute(script).await?;

        if result.objects.len() >= 1 {
            let val: T = serde_json::from_value(result.objects.into_iter().next().unwrap())
                .map_err(|e| format!("Parse error: {}", e))?;
            Ok(val)
        } else {
            // 尝试从 output 字符串反序列化
            serde_json::from_str(&result.output)
                .map_err(|_| format!("No objects returned and text parse failed"))
        }
    }

    /// 快捷方法: 获取系统信息
    pub async fn get_system_info(&self) -> Result<SystemInfo, String> {
        self.execute_scalar(
            "[PSCustomObject]@{ \
                OS = [System.Environment]::OSVersion.ToString(); \
                ComputerName = $env:COMPUTERNAME; \
                UserName = $env:USERNAME; \
                PSVersion = $PSVersionTable.PSVersion.ToString(); \
                DotNetVersion = (Get-ItemProperty 'HKLM:\\SOFTWARE\\Microsoft\\NET Framework\\Setup\\NDP\\v4\\Full' -ErrorAction SilentlyContinue).GetValue('Release')?.ToString(); \
            } | ConvertTo-Json"
        ).await
    }

    /// 快捷方法: 列出进程
    pub async fn list_processes(&self, name_filter: Option<&str>) -> Result<Vec<ProcessInfo>, String> {
        let filter = name_filter.unwrap_or("*");
        let script = format!(
            "Get-Process '{}' -ErrorAction SilentlyContinue \
             | Select-Object Id, ProcessName, CPU, WorkingSet64, StartTime \
             | Sort-Object -Property CPU -Descending \
             | Select-Object -First 20 \
             | ConvertTo-Json -Depth 3",
            filter.replace("'", "''")
        );

        self.execute_scalar::<Vec<ProcessInfo>>(&script).await
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SystemInfo {
    #[serde(rename = "OS")]
    pub os: String,
    pub computer_name: String,
    pub user_name: String,
    #[serde(rename = "PSVersion")]
    pub ps_version: String,
    #[serde(rename = "DotNetVersion")]
    pub dotnet_version: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProcessInfo {
    pub id: u32,
    #[serde(rename = "ProcessName")]
    pub process_name: String,
    pub cpu: f64,
    pub working_set_64: u64,
    #[serde(rename = "StartTime")]
    pub start_time: Option<String>,
}
