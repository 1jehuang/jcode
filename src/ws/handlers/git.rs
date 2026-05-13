//! Git 操作处理器
//!
//! 提供 Git 工作流集成：
//! - 状态查看
//! - Diff 对比
//! - 提交操作
/// - 分支管理
/// - 日志查看

use crate::ws::protocol::{WsRequest, WsResponse, GitStatusInfo, GitFileStatus, MessageType};
use crate::ws::session::SessionManager;
use anyhow::Result;
use tokio::process::Command;
use tracing::{info};

/// 处理 Git 状态请求
pub async fn handle_status(
    request: &WsRequest,
    session_id: &str,
    _session_manager: &SessionManager,
) -> Result<WsResponse> {
    let repo_path = request.params.get("path")
        .and_then(|v| v.as_str())
        .unwrap_or(".");

    info!(
        session_id = %session_id,
        path = %repo_path,
        "Getting git status"
    );

    // 执行 git status 命令
    let output = Command::new("git")
        .args(["status", "--porcelain=v2"])
        .current_dir(repo_path)
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to execute git status: {}", e))?;

    let output_str = String::from_utf8_lossy(&output.stdout);
    
    // 解析当前分支
    let branch_output = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(repo_path)
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to get branch: {}", e))?;
    
    let branch = String::from_utf8_lossy(&branch_output.stdout).trim().to_string();

    // 解析状态信息
    let (mut staged, mut modified, mut untracked) = (Vec::new(), Vec::new(), Vec::new());
    
    for line in output_str.lines() {
        if line.starts_with('1') || line.starts_with('2') {
            // 已暂存或已修改的文件
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                let path = parts.last().unwrap_or(&"").to_string();
                let status = parts.get(1).unwrap_or(&"?").chars().next().unwrap_or('?');
                
                match status {
                    'A' | 'M' | 'D' | 'R' | 'C' => {
                        staged.push(GitFileStatus {
                            path: path.clone(),
                            status: status.to_string(),
                        });
                    }
                    _ => {
                        modified.push(GitFileStatus {
                            path,
                            status: status.to_string(),
                        });
                    }
                }
            }
        } else if line.starts_with('?') {
            // 未跟踪文件
            let path = line.trim_start_matches("? ").to_string();
            untracked.push(path);
        }
    }

    let has_changes = !staged.is_empty() || !modified.is_empty() || !untracked.is_empty();

    let status_info = GitStatusInfo {
        branch,
        staged,
        modified,
        untracked,
        has_changes,
    };

    Ok(WsResponse::new(&request.id, MessageType::Response, serde_json::json!({
        "status": status_info,
        "repo_path": repo_path
    })))
}

/// 处理 Git diff 请求
pub async fn handle_diff(
    request: &WsRequest,
    session_id: &str,
    _session_manager: &SessionManager,
) -> Result<WsResponse> {
    let repo_path = request.params.get("path")
        .and_then(|v| v.as_str())
        .unwrap_or(".");
    
    let file_path = request.params.get("file_path").and_then(|v| v.as_str());
    let staged: bool = request.params.get("staged")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    info!(
        session_id = %session_id,
        repo = %repo_path,
        file = ?file_path,
        staged = staged,
        "Getting git diff"
    );

    let mut args = vec!["diff"];
    if staged {
        args.push("--staged");
    }
    if let Some(file) = file_path {
        args.push("--");
        args.push(file);
    }

    let output = Command::new("git")
        .args(&args)
        .current_dir(repo_path)
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to execute git diff: {}", e))?;

    let diff_output = String::from_utf8_lossy(&output.stdout).to_string();

    Ok(WsResponse::new(&request.id, MessageType::Response, serde_json::json!({
        "diff": diff_output,
        "repo_path": repo_path,
        "file_path": file_path,
        "staged": staged,
        "has_changes": !diff_output.is_empty()
    })))
}

/// 处理 Git 提交请求
pub async fn handle_commit(
    request: &WsRequest,
    session_id: &str,
    _session_manager: &SessionManager,
) -> Result<WsResponse> {
    let repo_path = request.params.get("path")
        .and_then(|v| v.as_str())
        .unwrap_or(".");
    
    let message = request.params.get("message")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'message' parameter"))?;

    let amend: bool = request.params.get("amend")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    
    let files: Option<Vec<String>> = request.params.get("files")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect());

    info!(
        session_id = %session_id,
        repo = %repo_path,
        message = %message,
        amend = amend,
        files_count = files.as_ref().map(|f| f.len()),
        "Creating commit"
    );

    // 如果指定了文件，先 add 这些文件
    if let Some(ref file_list) = files {
        let mut args = vec!["add".to_string()];
        args.extend(file_list.iter().cloned());
        
        Command::new("git")
            .args(&args)
            .current_dir(repo_path)
            .output()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to stage files: {}", e))?;
    }

    // 创建提交
    let mut args = vec!["commit", "-m", message];
    if amend {
        args.push("--amend");
    }

    let output = Command::new("git")
        .args(&args)
        .current_dir(repo_path)
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to create commit: {}", e))?;

    if !output.status.success() {
        let error_msg = String::from_utf8_lossy(&output.stderr);
        return Ok(WsResponse::error(&request.id, &format!("Commit failed: {}", error_msg)));
    }

    // 获取新的 commit hash
    let hash_output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo_path)
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to get commit hash: {}", e))?;

    let commit_hash = String::from_utf8_lossy(&hash_output.stdout).trim().to_string();

    info!(
        commit_hash = %commit_hash,
        "Commit created successfully"
    );

    Ok(WsResponse::new(&request.id, MessageType::Response, serde_json::json!({
        "success": true,
        "commit_hash": commit_hash,
        "message": message,
        "committed_at": chrono::Utc::now().to_rfc3339()
    })))
}

/// 处理 Git 分支操作请求
pub async fn handle_branch(
    request: &WsRequest,
    session_id: &str,
    _session_manager: &SessionManager,
) -> Result<WsResponse> {
    let repo_path = request.params.get("path")
        .and_then(|v| v.as_str())
        .unwrap_or(".");
    
    let action = request.params.get("action")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'action' parameter"))?;
    
    let name = request.params.get("name").and_then(|v| v.as_str());
    let target = request.params.get("target").and_then(|v| v.as_str());

    info!(
        session_id = %session_id,
        action = %action,
        name = ?name,
        target = ?target,
        "Branch operation"
    );

    match action {
        "list" => {
            // 列出所有分支
            let output = Command::new("git")
                .args(["branch", "-a", "--no-color"])
                .current_dir(repo_path)
                .output()
                .await
                .map_err(|e| anyhow::anyhow!("Failed to list branches: {}", e))?;

            let branches: Vec<String> = String::from_utf8_lossy(&output.stdout)
                .lines()
                .map(|line| line.trim().to_string())
                .collect();

            // 获取当前分支
            let current_output = Command::new("git")
                .args(["rev-parse", "--abbrev-ref", "HEAD"])
                .current_dir(repo_path)
                .output()
                .await
                .map_err(|e| anyhow::anyhow!("Failed to get current branch: {}", e))?;
            
            let current_branch = String::from_utf8_lossy(&current_output.stdout).trim().to_string();

            Ok(WsResponse::new(&request.id, MessageType::Response, serde_json::json!({
                "branches": branches,
                "current_branch": current_branch
            })))
        },
        
        "create" => {
            let branch_name = name.ok_or_else(|| anyhow::anyhow!("Missing 'name' for create action"))?;
            
            let mut args = vec!["checkout", "-b", branch_name];
            if let Some(base) = target {
                args.push(base);
            }

            let output = Command::new("git")
                .args(&args)
                .current_dir(repo_path)
                .output()
                .await
                .map_err(|e| anyhow::anyhow!("Failed to create branch: {}", e))?;

            if !output.status.success() {
                let error_msg = String::from_utf8_lossy(&output.stderr);
                return Ok(WsResponse::error(&request.id, &format!("Failed to create branch: {}", error_msg)));
            }

            Ok(WsResponse::new(&request.id, MessageType::Response, serde_json::json!({
                "success": true,
                "branch": branch_name,
                "message": format!("Created and switched to branch: {}", branch_name)
            })))
        },
        
        "switch" => {
            let branch_name = name.ok_or_else(|| anyhow::anyhow!("Missing 'name' for switch action"))?;
            
            let output = Command::new("git")
                .args(["checkout", branch_name])
                .current_dir(repo_path)
                .output()
                .await
                .map_err(|e| anyhow::anyhow!("Failed to switch branch: {}", e))?;

            if !output.status.success() {
                let error_msg = String::from_utf8_lossy(&output.stderr);
                return Ok(WsResponse::error(&request.id, &format!("Failed to switch branch: {}", error_msg)));
            }

            Ok(WsResponse::new(&request.id, MessageType::Response, serde_json::json!({
                "success": true,
                "branch": branch_name,
                "message": format!("Switched to branch: {}", branch_name)
            })))
        },
        
        "delete" => {
            let branch_name = name.ok_or_else(|| anyhow::anyhow!("Missing 'name' for delete action"))?;
            
            let force: bool = request.params.get("force")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let mut args = vec!["branch"];
            if force {
                args.push("-D");
            } else {
                args.push("-d");
            }
            args.push(branch_name);

            let output = Command::new("git")
                .args(&args)
                .current_dir(repo_path)
                .output()
                .await
                .map_err(|e| anyhow::anyhow!("Failed to delete branch: {}", e))?;

            if !output.status.success() {
                let error_msg = String::from_utf8_lossy(&output.stderr);
                return Ok(WsResponse::error(&request.id, &format!("Failed to delete branch: {}", error_msg)));
            }

            Ok(WsResponse::new(&request.id, MessageType::Response, serde_json::json!({
                "success": true,
                "branch": branch_name,
                "message": format!("Deleted branch: {}", branch_name)
            })))
        },
        
        _ => Ok(WsResponse::error(&request.id, &format!("Unknown branch action: {}", action))),
    }
}

/// 处理 Git 日志请求
pub async fn handle_log(
    request: &WsRequest,
    session_id: &str,
    _session_manager: &SessionManager,
) -> Result<WsResponse> {
    let repo_path = request.params.get("path")
        .and_then(|v| v.as_str())
        .unwrap_or(".");
    
    let limit: usize = request.params.get("limit")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
        .unwrap_or(20);
    
    let skip: usize = request.params.get("skip")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
        .unwrap_or(0);

    info!(
        session_id = %session_id,
        repo = %repo_path,
        limit = limit,
        skip = skip,
        "Getting git log"
    );

    // 使用 --pretty=format 自定义输出格式
    let format_str = "%H|%an|%ae|%aI|%s";
    
    let output = Command::new("git")
        .args([
            "log",
            &format!("-{}", limit),
            &format!("--skip={skip}"),
            "--pretty=format",
            format_str,
        ])
        .current_dir(repo_path)
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to get git log: {}", e))?;

    let log_output = String::from_utf8_lossy(&output.stdout);
    
    let commits: Vec<serde_json::Value> = log_output
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let parts: Vec<&str> = line.splitn(5, '|').collect();
            serde_json::json!({
                "hash": parts.get(0).unwrap_or(&""),
                "author": {
                    "name": parts.get(1).unwrap_or(&""),
                    "email": parts.get(2).unwrap_or(&""),
                },
                "date": parts.get(3).unwrap_or(&""),
                "message": parts.get(4).unwrap_or(&""),
            })
        })
        .collect();

    Ok(WsResponse::new(&request.id, MessageType::Response, serde_json::json!({
        "commits": commits,
        "count": commits.len(),
        "repo_path": repo_path
    })))
}
