//! 会话恢复实现 (-c 和 -r flags)
//!
//! Claude Code兼容:
//! - `claude -c` : 继续当前目录最近的会话
//! - `claude -c -p "query"` : 继续会话并通过SDK查询
//! - `claude -r "session-name" "query"` : 按名称恢复会话
//!
//! ## 功能特性
//! 1. 自动检测最近会话
//! 2. 按名称或ID恢复历史会话
//! 3. 会话列表和搜索
//! 4. 会话元数据管理

use anyhow::Result;
use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 会话信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    /// 会话ID (UUID)
    pub id: String,
    
    /// 会话名称 (用户自定义或自动生成)
    pub name: Option<String>,
    
    /// 项目路径
    pub project_path: PathBuf,
    
    /// 创建时间
    pub created_at: DateTime<Utc>,
    
    /// 最后访问时间
    pub last_accessed: DateTime<Local>,
    
    /// 消息数量
    pub message_count: usize,
    
    /// Token使用量
    pub tokens_used: u64,
    
    /// 会话状态
    pub status: SessionStatus,
    
    /// 模型名称
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SessionStatus {
    Active,
    Completed,
    Interrupted,
    Archived,
}

impl std::fmt::Display for SessionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Active => write!(f, "🟢 活跃"),
            Self::Completed => write!(f, "✅ 已完成"),
            Self::Interrupted => write!(f, "⚠️ 中断"),
            Self::Archived => write!(f, "📦 已归档"),
        }
    }
}

/// 继续最近会话 (-c flag)
pub async fn run_continue_session() -> Result<SessionInfo> {
    // 1. 获取当前工作目录
    let cwd = std::env::current_dir()?;
    
    // 2. 查找该目录下最近的会话
    let recent_session = find_most_recent_session(&cwd)?;
    
    // 3. 加载会话数据
    load_session(&recent_session.id).await?;
    
    // 4. 更新最后访问时间
    update_session_access_time(&recent_session.id)?;
    
    eprintln!("🔄 继续最近会话:");
    print_session_info(&recent_session);
    
    Ok(recent_session)
}

/// 按名称/ID恢复会话 (-r flag)
pub async fn run_resume_session(session_identifier: &str) -> Result<SessionInfo> {
    // 1. 尝试按名称查找
    if let Some(session) = find_session_by_name(session_identifier)? {
        load_session(&session.id).await?;
        update_session_access_time(&session.id)?;
        
        eprintln!("🔄 恢复会话 '{}':", session_identifier);
        print_session_info(&session);
        
        return Ok(session);
    }
    
    // 2. 尝试按ID查找
    if let Some(session) = find_session_by_id(session_identifier)? {
        load_session(&session.id).await?;
        update_session_access_time(&session.id)?;
        
        eprintln!("🔄 恢复会话 (ID: {}):", session_identifier);
        print_session_info(&session);
        
        return Ok(session);
    }
    
    // 3. 未找到
    anyhow::bail!(
        "未找到会话 '{}'。\n\
         使用 'carpai --resume' 列出所有可用会话，\n\
         或使用 'carpai -c' 继续最近的会话。",
        session_identifier
    )
}

/// 列出所有会话
pub async fn list_sessions(project_path: Option<&str>) -> Result<Vec<SessionInfo>> {
    let sessions_dir = get_sessions_directory();
    
    if !sessions_dir.exists() {
        return Ok(Vec::new());
    }
    
    let mut sessions = Vec::new();
    
    for entry in std::fs::read_dir(sessions_dir)? {
        let entry = entry?;
        let path = entry.path();
        
        if path.extension().map(|e| e == "json").unwrap_or(false) {
            if let Ok(session) = load_session_from_file(&path) {
                // 过滤项目路径 (如果指定)
                if let Some(proj_path) = project_path {
                    if session.project_path == PathBuf::from(proj_path) {
                        sessions.push(session);
                    }
                } else {
                    sessions.push(session);
                }
            }
        }
    }
    
    // 按最后访问时间降序排序
    sessions.sort_by(|a, b| b.last_accessed.cmp(&a.last_accessed));
    
    Ok(sessions)
}

// --- Internal Methods ----------------------

fn find_most_recent_session(cwd: &std::path::Path) -> Result<SessionInfo> {
    let sessions = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(list_sessions(Some(&cwd.to_string_lossy())))
    })?;
    
    sessions.into_iter().next().ok_or_else(|| {
        anyhow::anyhow!(
            "未找到最近的会话。\n\
             请先启动一个新会话: carpai"
        )
    })
}

fn find_session_by_name(name: &str) -> Result<Option<SessionInfo>> {
    let sessions = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(list_sessions(None))
    })?;
    
    Ok(sessions.into_iter()
        .find(|s| s.name.as_deref() == Some(name)))
}

fn find_session_by_id(id: &str) -> Result<Option<SessionInfo>> {
    let sessions = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(list_sessions(None))
    })?;
    
    Ok(sessions.into_iter()
        .find(|s| s.id.starts_with(id))) // 支持部分ID匹配
}

async fn load_session(session_id: &str) -> Result<()> {
    let session_file = get_session_file_path(session_id);
    
    if !session_file.exists() {
        anyhow::bail!("会话文件不存在: {}", session_id);
    }
    
    // TODO: 实际加载会话数据到Agent中
    // 这里应该恢复消息历史、上下文、工具状态等
    
    tracing::info!(session_id, "Session loaded");
    
    Ok(())
}

fn load_session_from_file(path: &std::path::Path) -> Result<SessionInfo> {
    let content = std::fs::read_to_string(path)?;
    let session: SessionInfo = serde_json::from_str(&content)?;
    Ok(session)
}

fn update_session_access_time(session_id: &str) -> Result<()> {
    let session_file = get_session_file_path(session_id);
    
    if session_file.exists() {
        let mut session = load_session_from_file(&session_file)?;
        session.last_accessed = Local::now();
        
        let content = serde_json::to_string_pretty(&session)?;
        std::fs::write(&session_file, content)?;
    }
    
    Ok(())
}

fn get_sessions_directory() -> PathBuf {
    let data_dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."));
    
    data_dir.join("carpai").join("sessions")
}

fn get_session_file_path(session_id: &str) -> PathBuf {
    get_sessions_directory().join(format!("{}.json", session_id))
}

fn print_session_info(session: &SessionInfo) {
    println!("   ID: {}", &session.id[..8.min(session.id.len())]);
    if let Some(name) = &session.name {
        println!("   名称: {}", name);
    }
    println!("   状态: {}", session.status);
    println!("   模型: {}", session.model.as_deref().unwrap_or("默认"));
    println!("   消息数: {}", session.message_count);
    println!("   Token使用: {}", format_tokens(session.tokens_used));
    println!("   最后访问: {}", session.last_accessed.format("%Y-%m-%d %H:%M:%S"));
    println!("   创建时间: {}", session.created_at.format("%Y-%m-%d %H:%M:%S UTC"));
}

fn format_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.2}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.2}K", tokens as f64 / 1_000.0)
    } else {
        format!("{}", tokens)
    }
}
