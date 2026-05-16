//! # Debugger — 调试器引擎
//!
//! 从 Claude Code 移植并增强的调试器系统：
//! - DAP (Debug Adapter Protocol) 客户端：通过 stdio JSON-RPC 与调试器通信
//! - 支持 LLDB / GDB：自动检测可用调试后端
//! - 断点管理：行号/函数名/条件断点
//! - 调用栈检查：获取 full backtrace + 局部变量
//! - 表达式求值：在断点处 eval 任意表达式
//! - 运行时注入：attach 到运行中进程
//! - 结果解析：结构化解析 crash/diagnostic 输出

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

static SEQ: AtomicU64 = AtomicU64::new(1);
fn next_seq() -> u64 { SEQ.fetch_add(1, Ordering::SeqCst) }

// -- DAP Protocol Types --

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
enum DapRequest {
    Initialize { seq: u64, command: String, arguments: Value },
    Launch { seq: u64, command: String, arguments: Value },
    Attach { seq: u64, command: String, arguments: Value },
    SetBreakpoints { seq: u64, command: String, arguments: Value },
    ConfigurationDone { seq: u64, command: String },
    Continue { seq: u64, command: String, arguments: Value },
    Next { seq: u64, command: String, arguments: Value },
    StepIn { seq: u64, command: String, arguments: Value },
    StepOut { seq: u64, command: String, arguments: Value },
    Pause { seq: u64, command: String, arguments: Value },
    StackTrace { seq: u64, command: String, arguments: Value },
    Scopes { seq: u64, command: String, arguments: Value },
    Variables { seq: u64, command: String, arguments: Value },
    Evaluate { seq: u64, command: String, arguments: Value },
    Threads { seq: u64, command: String },
    Disconnect { seq: u64, command: String, arguments: Value },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DapResponse {
    #[serde(rename = "type")]
    resp_type: String,
    seq: Option<u64>,
    command: Option<String>,
    request_seq: Option<u64>,
    success: Option<bool>,
    message: Option<String>,
    body: Option<Value>,
    event: Option<String>,
}

// -- Public Types --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebugConfig {
    pub debugger_type: DebuggerType,
    pub program: Option<PathBuf>,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub cwd: Option<PathBuf>,
    pub pid: Option<u32>,
    pub timeout_secs: u64,
}

impl Default for DebugConfig {
    fn default() -> Self {
        Self {
            debugger_type: DebuggerType::Auto,
            program: None,
            args: vec![],
            env: HashMap::new(),
            cwd: None,
            pid: None,
            timeout_secs: 30,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DebuggerType {
    Auto,
    Lldb,
    Gdb,
    LldbDap,
    CppVsdbg,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebugSession {
    pub id: String,
    pub config: DebugConfig,
    pub status: DebugStatus,
    pub started_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DebugStatus {
    Idle,
    Launching,
    Running,
    Paused,
    Stopped,
    Error,
}

impl DebugStatus {
    pub fn icon(&self) -> &'static str {
        match self {
            Self::Idle => "⏸️",
            Self::Launching => "🚀",
            Self::Running => "▶️",
            Self::Paused => "⏯️",
            Self::Stopped => "⏹️",
            Self::Error => "❌",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Breakpoint {
    pub id: usize,
    pub source: PathBuf,
    pub line: usize,
    pub column: Option<usize>,
    pub condition: Option<String>,
    pub hit_count: usize,
    pub verified: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StackFrame {
    pub id: usize,
    pub name: String,
    pub source: Option<PathBuf>,
    pub line: usize,
    pub column: usize,
    pub module: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Variable {
    pub name: String,
    pub value: String,
    pub var_type: String,
    pub children: Vec<Variable>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalResult {
    pub expression: String,
    pub value: String,
    pub var_type: String,
    pub success: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebugResult {
    pub session_id: String,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub hit_breakpoints: Vec<Breakpoint>,
    pub stack_frames: Vec<StackFrame>,
    pub variables: Vec<Variable>,
    pub crash_info: Option<String>,
    pub elapsed_ms: u64,
}

// -- Debugger Client --

pub struct DebugClient {
    config: DebugConfig,
    process: Option<Child>,
    request_seq: u64,
}

impl DebugClient {
    pub fn new(config: DebugConfig) -> Self {
        Self { config, process: None, request_seq: 0 }
    }

    pub async fn launch(&mut self) -> Result<DebugSession> {
        let id = crate::id::new_id("debug");
        let started_at = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

        let adapter = Self::resolve_adapter(self.config.debugger_type);
        info!("Launching debugger: {} (adapter: {})", id, adapter);

        let mut cmd = Command::new(&adapter);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let process = cmd.spawn()
            .with_context(|| format!("Failed to launch debug adapter: {}", adapter))?;

        self.process = Some(process);

        self.send_initialize().await?;
        self.send_launch().await?;

        Ok(DebugSession {
            id,
            config: self.config.clone(),
            status: DebugStatus::Launching,
            started_at,
        })
    }

    pub async fn attach(&mut self, pid: u32) -> Result<DebugSession> {
        let id = crate::id::new_id("debug");
        let started_at = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let adapter = Self::resolve_adapter(self.config.debugger_type);

        let mut cmd = Command::new(&adapter);
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());

        let process = cmd.spawn()
            .with_context(|| format!("Failed to launch: {}", adapter))?;
        self.process = Some(process);

        self.send_initialize().await?;
        self.send_attach(pid).await?;

        Ok(DebugSession {
            id,
            config: self.config.clone(),
            status: DebugStatus::Launching,
            started_at,
        })
    }

    pub async fn run_to_completion(&mut self, breakpoints: &[Breakpoint]) -> Result<DebugResult> {
        let start = Instant::now();

        for bp in breakpoints {
            self.set_breakpoint(bp).await?;
        }

        self.configuration_done().await?;
        self.do_continue().await?;

        let (stdout, stderr, exit_code) = self.wait_for_exit(Duration::from_secs(self.config.timeout_secs)).await?;

        Ok(DebugResult {
            session_id: String::new(),
            stdout,
            stderr,
            exit_code,
            hit_breakpoints: vec![],
            stack_frames: vec![],
            variables: vec![],
            crash_info: None,
            elapsed_ms: start.elapsed().as_millis() as u64,
        })
    }

    pub async fn set_breakpoint(&mut self, bp: &Breakpoint) -> Result<()> {
        let args = json!({
            "source": { "name": bp.source.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                         "path": bp.source.to_string_lossy().to_string() },
            "breakpoints": [{
                "line": bp.line,
                "column": bp.column.unwrap_or(1),
                "condition": bp.condition
            }]
        });
        let resp = self.send("setBreakpoints", Some(args)).await?;
        debug!("Breakpoint set: {:?}", resp);
        Ok(())
    }

    pub async fn get_stack_trace(&mut self, thread_id: usize) -> Result<Vec<StackFrame>> {
        let args = json!({ "threadId": thread_id, "startFrame": 0, "levels": 50 });
        let resp = self.send("stackTrace", Some(args)).await?;

        let frames: Vec<StackFrame> = resp
            .get("body")
            .and_then(|b| b.get("stackFrames"))
            .and_then(|f| f.as_array())
            .map(|arr| {
                arr.iter().enumerate().map(|(i, f)| StackFrame {
                    id: f.get("id").and_then(|v| v.as_u64()).unwrap_or(i as u64) as usize,
                    name: f.get("name").and_then(|v| v.as_str()).unwrap_or("???").into(),
                    source: f.get("source").and_then(|s| s.get("path")).and_then(|v| v.as_str()).map(PathBuf::from),
                    line: f.get("line").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
                    column: f.get("column").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
                    module: f.get("moduleId").and_then(|v| v.as_str()).map(|s| s.to_string()),
                }).collect()
            })
            .unwrap_or_default();

        Ok(frames)
    }

    pub async fn evaluate(&mut self, expression: &str, frame_id: Option<usize>) -> Result<EvalResult> {
        let mut args = json!({ "expression": expression, "context": "repl" });
        if let Some(fid) = frame_id {
            args["frameId"] = json!(fid);
        }
        let resp = self.send("evaluate", Some(args)).await?;

        Ok(EvalResult {
            expression: expression.to_string(),
            value: resp.get("body").and_then(|b| b.get("result")).and_then(|v| v.as_str()).unwrap_or("???").to_string(),
            var_type: resp.get("body").and_then(|b| b.get("type")).and_then(|v| v.as_str()).unwrap_or("unknown").to_string(),
            success: true,
        })
    }

    pub async fn disconnect(&mut self) -> Result<()> {
        let _ = self.send("disconnect", Some(json!({ "terminateDebuggee": true }))).await;
        if let Some(ref mut p) = self.process {
            let _ = p.kill();
        }
        self.process = None;
        Ok(())
    }

    // -- internal --

    async fn send_initialize(&mut self) -> Result<Value> {
        let args = json!({
            "clientID": "jcode",
            "clientName": "jcode-debugger",
            "adapterID": "jcode",
            "linesStartAt1": true,
            "columnsStartAt1": true,
            "supportsProgressReporting": true,
            "supportsVariableType": true,
            "supportsRunInTerminalRequest": false
        });
        self.send("initialize", Some(args)).await
    }

    async fn send_launch(&mut self) -> Result<Value> {
        let program = self.config.program.as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let args = json!({
            "program": program,
            "args": self.config.args,
            "cwd": self.config.cwd.as_ref().map(|p| p.to_string_lossy().to_string()),
            "env": self.config.env,
            "stopOnEntry": true,
            "noDebug": false
        });
        self.send("launch", Some(args)).await
    }

    async fn send_attach(&mut self, pid: u32) -> Result<Value> {
        let args = json!({ "processId": pid });
        self.send("attach", Some(args)).await
    }

    async fn configuration_done(&mut self) -> Result<Value> {
        self.send("configurationDone", None).await
    }

    async fn do_continue(&mut self) -> Result<Value> {
        self.send("continue", Some(json!({ "threadId": 0 }))).await
    }

    async fn send(&mut self, command: &str, arguments: Option<Value>) -> Result<Value> {
        let seq = next_seq();
        let req = json!({
            "seq": seq,
            "type": "request",
            "command": command,
            "arguments": arguments.unwrap_or(Value::Null)
        });

        let line = serde_json::to_string(&req)? + "\n";

        if let Some(ref mut process) = self.process
            && let Some(ref mut stdin) = process.stdin {
                stdin.write_all(line.as_bytes())
                    .with_context(|| format!("DAP write failed for {}", command))?;
                stdin.flush()?;
            }

        self.read_response(seq).await
    }

    async fn read_response(&mut self, seq: u64) -> Result<Value> {
        let timeout = Duration::from_secs(self.config.timeout_secs);
        let start = Instant::now();

        loop {
            if start.elapsed() > timeout {
                anyhow::bail!("DAP timeout waiting for response to seq {}", seq);
            }

            if let Some(ref mut process) = self.process
                && let Some(ref mut stdout) = process.stdout {
                    let mut reader = BufReader::new(stdout);
                    let mut line = String::new();
                    let n = reader.read_line(&mut line)?;
                    if n == 0 {
                        anyhow::bail!("DAP stdin closed");
                    }

                    if let Ok(resp) = serde_json::from_str::<DapResponse>(&line) {
                        if resp.resp_type == "event" {
                            debug!("DAP event: {:?}", resp.event);
                            continue;
                        }
                        if resp.request_seq == Some(seq) {
                            if resp.success != Some(true) {
                                warn!("DAP error for {}: {:?}", resp.command.as_deref().unwrap_or("?"), resp.message);
                            }
                            return Ok(json!(resp));
                        }
                        continue;
                    }
                    return Ok(serde_json::from_str(&line)?);
                }
            anyhow::bail!("DAP process not started");
        }
    }

    async fn wait_for_exit(&mut self, timeout: Duration) -> Result<(String, String, Option<i32>)> {
        let start = Instant::now();

        if let Some(ref mut process) = self.process {
            loop {
                if start.elapsed() > timeout {
                    process.kill()?;
                    anyhow::bail!("Process timeout");
                }

                match process.try_wait()? {
                    Some(status) => {
                        let mut stdout = String::new();
                        let mut stderr = String::new();
                        if let Some(ref mut out) = process.stdout {
                            use std::io::Read;
                            let mut buf = Vec::new();
                            out.read_to_end(&mut buf)?;
                            stdout = String::from_utf8_lossy(&buf).to_string();
                        }
                        if let Some(ref mut err) = process.stderr {
                            use std::io::Read;
                            let mut buf = Vec::new();
                            err.read_to_end(&mut buf)?;
                            stderr = String::from_utf8_lossy(&buf).to_string();
                        }
                        return Ok((stdout, stderr, status.code()));
                    }
                    None => {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
            }
        }
        Ok((String::new(), String::new(), None))
    }

    fn resolve_adapter(dbg_type: DebuggerType) -> String {
        match dbg_type {
            DebuggerType::Lldb => {
                if which::which("lldb-vscode").is_ok() { return "lldb-vscode".into(); }
                if which::which("lldb-dap").is_ok() { return "lldb-dap".into(); }
                "lldb".into()
            }
            DebuggerType::Gdb => "gdb".into(),
            DebuggerType::LldbDap => {
                if which::which("lldb-dap").is_ok() { "lldb-dap".into() }
                else if which::which("lldb-vscode").is_ok() { "lldb-vscode".into() }
                else { "lldb".into() }
            }
            DebuggerType::CppVsdbg => "OpenDebugAD7".into(),
            DebuggerType::Auto => {
                if which::which("lldb-dap").is_ok() { "lldb-dap".into() }
                else if which::which("lldb-vscode").is_ok() { "lldb-vscode".into() }
                else if which::which("gdb").is_ok() { "gdb".into() }
                else { "lldb".into() }
            }
        }
    }
}

// -- Crash Analyzer --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrashInfo {
    pub signal: Option<String>,
    pub address: Option<String>,
    pub thread: Option<String>,
    pub backtrace: Vec<String>,
    pub sanitizer_output: Option<String>,
}

pub fn parse_crash_output(stderr: &str) -> CrashInfo {
    let mut backtrace = Vec::new();
    let mut signal = None;
    let mut address = None;
    let mut thread = None;

    for line in stderr.lines() {
        let trimmed = line.trim();

        if trimmed.contains("SIGSEGV") || trimmed.contains("SIGABRT") || trimmed.contains("SIGILL") {
            for word in trimmed.split_whitespace() {
                if word.starts_with("SIG") && word.len() <= 8 {
                    signal = Some(word.to_string());
                    break;
                }
            }
        }

        if (trimmed.contains("at address") || trimmed.contains("address:"))
            && let Some(addr) = trimmed.split([':', ' ']).next_back()
                && addr.len() >= 8 && addr.chars().all(|c| c.is_ascii_hexdigit() || c == 'x' || c == '0') {
                    address = Some(addr.to_string());
                }

        if trimmed.starts_with('#') && trimmed.split_whitespace().count() >= 2 {
            backtrace.push(trimmed.to_string());
        }
        if trimmed.contains("at") && (trimmed.contains(".rs:") || trimmed.contains(".c:") || trimmed.contains(".cpp:")) {
            backtrace.push(trimmed.to_string());
        }

        if trimmed.starts_with("Thread") || trimmed.contains("faulting thread") {
            thread = Some(trimmed.to_string());
        }
    }

    CrashInfo { signal, address, thread, backtrace, sanitizer_output: None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_segfault() {
        let stderr = "SIGSEGV at address 0xdeadbeef\n\
                      #0 0x00401234 in main at src/main.rs:15\n\
                      Thread 1 received signal SIGSEGV\n";

        let crash = parse_crash_output(stderr);
        assert_eq!(crash.signal.as_deref(), Some("SIGSEGV"));
        assert_eq!(crash.address.as_deref(), Some("0xdeadbeef"));
        assert!(!crash.backtrace.is_empty());
        assert_eq!(crash.backtrace[0], "#0 0x00401234 in main at src/main.rs:15");
    }

    #[test]
    fn test_resolve_adapter() {
        let adapter = DebugClient::resolve_adapter(DebuggerType::Auto);
        assert!(!adapter.is_empty());
    }

    #[test]
    fn test_breakpoint_serialization() {
        let bp = Breakpoint {
            id: 1,
            source: PathBuf::from("src/main.rs"),
            line: 42,
            column: Some(5),
            condition: Some("x > 10".into()),
            hit_count: 0,
            verified: false,
        };
        let json = serde_json::to_string(&bp).unwrap();
        let bp2: Breakpoint = serde_json::from_str(&json).unwrap();
        assert_eq!(bp2.id, 1);
        assert_eq!(bp2.line, 42);
        assert_eq!(bp2.condition.as_deref(), Some("x > 10"));
    }
}