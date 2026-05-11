//! 项目管理处理器
//!
//! 提供项目构建和测试功能：
//! - 构建项目
//! - 运行测试
//! - 执行脚本/命令

use crate::ws::protocol::{WsRequest, WsResponse, MessageType};
use crate::ws::session::SessionManager;
use anyhow::Result;
use tokio::process::Command;
use tracing::{info};

/// 处理项目构建请求
pub async fn handle_build(
    command_request: &WsRequest,
    session_id: &str,
    _session_manager: &SessionManager,
) -> Result<WsResponse> {
    let project_path = command_request.params.get("path")
        .and_then(|v| v.as_str())
        .unwrap_or(".");
    
    let build_command = command_request.params.get("command")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| detect_build_command(project_path));
    
    let args: Option<Vec<String>> = command_request.params.get("args")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect());

    let release_mode: bool = command_request.params.get("release")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    info!(
        session_id = %session_id,
        path = %project_path,
        command = %build_command,
        release = release_mode,
        args = ?args,
        "Build requested"
    );

    // 构建完整的命令参数
    let mut cmd_args: Vec<String> = if let Some(ref custom_args) = args {
        custom_args.clone()
    } else {
        Vec::new()
    };
    
    if release_mode && !cmd_args.contains(&"--release".to_string()) {
        cmd_args.push("--release".to_string());
    }

    // 执行构建命令
    let output = Command::new(build_command)
        .args(&cmd_args)
        .current_dir(project_path)
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to execute build command: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let success = output.status.success();

    if success {
        info!(command = %build_command, "Build succeeded");
    } else {
        warn!(command = %build_command, stderr = %stderr, "Build failed");
    }

    Ok(WsResponse::new(&command_request.id, MessageType::Response, serde_json::json!({
        "success": success,
        "exit_code": output.status.code(),
        "stdout": stdout,
        "stderr": stderr,
        "command": build_command,
        "args": cmd_args,
        "duration_ms": 0, // TODO: 计算实际耗时
        "built_at": chrono::Utc::now().to_rfc3339()
    })))
}

/// 处理运行测试请求
pub async fn handle_test(
    request: &WsRequest,
    session_id: &str,
    _session_manager: &SessionManager,
) -> Result<WsResponse> {
    let project_path = request.params.get("path")
        .and_then(|v| v.as_str())
        .unwrap_or(".");
    
    let test_command = request.params.get("command")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| detect_test_command(project_path));
    
    let test_filter: Option<String> = request.params.get("filter")
        .and_then(|v| v.as_str())
        .map(String::from);
    
    let verbose: bool = request.params.get("verbose")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    info!(
        session_id = %session_id,
        path = %project_path,
        command = %test_command,
        filter = ?test_filter,
        verbose = verbose,
        "Test requested"
    );

    // 构建测试命令参数
    let mut args = Vec::new();
    
    match test_command {
        "cargo" => {
            args.push("test".to_string());
            if verbose { args.push("--verbose".to_string()); }
            if let Some(filter) = test_filter {
                args.push(format!("-- {}", filter)); // cargo test -- <filter>
            }
            args.push("--no-fail-fast".to_string());
            args.push("--".to_string());
        },
        "npm" => {
            args.push("test".to_string());
            if verbose { args.push("--verbose".to_string()); }
            if let Some(filter) = test_filter {
                args.push(format!("--grep={}", filter));
            }
        },
        _ => {
            args.push(test_command.to_string());
        }
    }

    // 执行测试命令
    let output = Command::new(test_command)
        .args(&args)
        .current_dir(project_path)
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to execute test command: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let success = output.status.success();

    // 解析测试结果（简化版）
    let (passed, failed, total) = parse_test_results(&stdout, &stderr);

    info!(
        total = total,
        passed = passed,
        failed = failed,
        success = success,
        "Test completed"
    );

    Ok(WsResponse::new(&request.id, MessageType::Response, serde_json::json!({
        "success": success,
        "exit_code": output.status.code(),
        "stdout": stdout,
        "stderr": stderr,
        "summary": {
            "total": total,
            "passed": passed,
            "failed": failed,
            "skipped": total.saturating_sub(passed + failed),
        },
        "command": test_command,
        "duration_ms": 0, // TODO: 实际计时
        "completed_at": chrono::Utc::now().to_rfc3339()
    })))
}

/// 处理运行项目/脚本请求
pub async fn handle_run(
    request: &WsRequest,
    session_id: &str,
    _session_manager: &SessionManager,
) -> Result<WsResponse> {
    let project_path = request.params.get("path")
        .and_then(|v| v.as_str())
        .unwrap_or(".");
    
    let run_command = request.params.get("command")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'command' parameter"))?;
    
    let args: Option<Vec<String>> = request.params.get("args")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect());

    let env_vars: Option<std::collections::HashMap<String, String>> = request.params.get("env")
        .and_then(|v| v.as_object())
        .map(|obj| {
            obj.iter()
                .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
                .collect()
        });

    info!(
        session_id = %session_id,
        path = %project_path,
        command = %run_command,
        args = ?args,
        env_count = env_vars.as_ref().map(|e| e.len()),
        "Run requested"
    );

    // 使用 spawn 以便可以长时间运行并实时输出
    // 注意：这里使用 output() 是为了简化，生产环境应该用 spawn + 流式输出
    
    let mut cmd = Command::new(run_command);
    
    if let Some(ref cmd_args) = args {
        cmd.args(cmd_args);
    }
    
    if let Some(ref env) = env_vars {
        for (key, value) in env {
            cmd.env(key, value);
        }
    }
    
    cmd.current_dir(project_path);

    let output = cmd.output()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to execute command: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let success = output.status.success();

    Ok(WsResponse::new(&request.id, MessageType::Response, serde_json::json!({
        "success": success,
        "exit_code": output.status.code(),
        "stdout": stdout,
        "stderr": stderr,
        "command": run_command,
        "pid": None, // 如果使用 spawn，这里会有 PID
        "started_at": chrono::Utc::now().to_rfc3339()
    })))
}

/// 检测项目的构建命令
fn detect_build_command(path: &str) -> &'static str {
    if std::path::Path::new(path).join("Cargo.toml").exists() {
        "cargo"
    } else if std::path::Path::new(path).join("package.json").exists() {
        "npm"
    } else if std::path::Path::new(path).join("Makefile").exists() {
        "make"
    } else if std::path::Path::new(path).join("pom.xml").exists() {
        "mvn"
    } else if std::path::Path::new(path).join("build.gradle").exists() || 
              std::path::Path::new(path).join("build.gradle.kts").exists() {
        "gradle"
    } else if cfg!(target_os = "windows") && std::path::Path::new(path).join("*.sln").exists() {
        "msbuild"
    } else {
        "make" // 默认使用 make
    }
}

/// 检测项目的测试命令
fn detect_test_command(path: &str) -> &'static str {
    if std::path::Path::new(path).join("Cargo.toml").exists() {
        "cargo"
    } else if std::path::Path::new(path).join("package.json").exists() {
        "npm"
    } else if std::path::Path::new(path).join("Makefile").exists() {
        "make"
    } else {
        "make"
    }
}

/// 解析测试输出以提取通过/失败数量（简化版）
fn parse_test_results(stdout: &str, stderr: &str) -> (usize, usize, usize) {
    let combined = format!("{} {}", stdout, stderr);
    
    // 尝试匹配各种格式
    // Cargo 格式: "test result: ok. X passed; Y failed"
    if let Some(caps) = regex::Regex::new(r"(\d+) passed(?:; (\d+) failed)?")
        .ok()
        .and_then(|re| re.captures(&combined))
    {
        let passed: usize = caps.get(1).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
        let failed: usize = caps.get(2).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
        return (passed, failed, passed + failed);
    }

    // npm/jest 格式: "Tests: X passed, Y failed"
    if let Some(caps) = regex::Regex::new(r"Tests?:\s*(\d+) passed(?:,?\s*(\d+) failed)?")
        .ok()
        .and_then(|re| re.captures(&combined))
    {
        let passed: usize = caps.get(1).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
        let failed: usize = caps.get(2).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
        return (passed, failed, passed + failed);
    }

    // 无法解析，返回默认值
    (0, 0, 0)
}
