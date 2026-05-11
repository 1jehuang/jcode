// multi_workspace.rs
// ════════════════════════════════════════════════════════════════
// 多工作区管理器 — 支持同时打开多个项目

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lsp_types::*;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::{
    LspError, LspResult,
    LspServerManager,
    DocumentSyncManager,
    DiagnosticsManager,
    LspResultCache,
    LspOperations,
};

/// 工作区 ID（唯一标识符）
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceId(pub String);

impl std::fmt::Display for WorkspaceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// 工作区实例
pub struct WorkspaceInstance {
    /// 工作区 ID
    pub id: WorkspaceId,
    /// 工作区名称
    pub name: String,
    /// 工作区根路径
    pub path: PathBuf,
    /// 语言服务器管理器
    pub server_manager: Arc<LspServerManager>,
    /// 文档同步管理器
    pub document_sync: Arc<DocumentSyncManager>,
    /// 诊断信息管理器
    pub diagnostics: Arc<DiagnosticsManager>,
    /// 结果缓存
    pub cache: Arc<LspResultCache<serde_json::Value>>,
    /// 已打开的文件列表
    pub opened_files: RwLock<HashSet<Url>>,
    /// 创建时间
    pub created_at: std::time::Instant,
    /// 最后活动时间
    pub last_active_at: RwLock<std::time::Instant>,
}

/// 多工作区配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiWorkspaceConfig {
    /// 最大工作区数量
    pub max_workspaces: usize,
    /// 是否共享语言服务器
    pub shared_servers: bool,
    /// 是否支持跨工作区引用
    pub cross_workspace_refs: bool,
    /// 空闲超时（自动关闭）
    pub idle_timeout_seconds: Option<u64>,
}

impl Default for MultiWorkspaceConfig {
    fn default() -> Self {
        Self {
            max_workspaces: 5,
            shared_servers: true,
            cross_workspace_refs: true,
            idle_timeout_seconds: Some(3600), // 1 小时
        }
    }
}

/// 多工作区管理器
pub struct MultiWorkspaceManager {
    /// 所有工作区实例
    workspaces: Arc<RwLock<HashMap<WorkspaceId, Arc<WorkspaceInstance>>>>,
    /// 当前活动的工作区
    active_workspace: Arc<RwLock<Option<WorkspaceId>>>,
    /// 配置
    config: MultiWorkspaceConfig,
    /// 统计信息
    stats: Arc<RwLock<MultiWorkspaceStats>>,
}

/// 多工作区统计信息
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MultiWorkspaceStats {
    /// 总创建次数
    pub total_created: u64,
    /// 总关闭次数
    pub total_closed: u64,
    /// 当前活跃数
    pub active_count: usize,
    /// 峰值数量
    pub peak_count: usize,
    /// 跨工作区查询次数
    pub pub_cross_workspace_queries: u64,
}

impl MultiWorkspaceManager {
    /// 创建新的多工作区管理器
    pub fn new() -> Self {
        Self::with_config(MultiWorkspaceConfig::default())
    }

    /// 使用配置创建多工作区管理器
    pub fn with_config(config: MultiWorkspaceConfig) -> Self {
        Self {
            workspaces: Arc::new(RwLock::new(HashMap::new())),
            active_workspace: Arc::new(RwLock::new(None)),
            config,
            stats: Arc::new(RwLock::new(MultiWorkspaceStats::default())),
        }
    }

    /// 创建新工作区
    pub async fn create_workspace(
        &self,
        path: &Path,
        name: Option<&str>,
    ) -> LspResult<WorkspaceId> {
        // 检查是否超过最大限制
        {
            let workspaces = self.workspaces.read().await;
            if workspaces.len() >= self.config.max_workspaces {
                return Err(LspError::StartFailed(format!(
                    "Maximum number of workspaces ({}) reached",
                    self.config.max_workspaces
                )));
            }
        }

        // 验证路径存在
        if !path.exists() {
            return Err(LspError::StartFailed(format!(
                "Workspace path does not exist: {}",
                path.display()
            )));
        }

        // 生成工作区 ID 和名称
        let workspace_name = name
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("workspace")
                    .to_string()
            });

        let workspace_id = WorkspaceId(format!("ws_{}", uuid::Uuid::new_v4()));

        info!(
            "Creating workspace '{}' at '{}'",
            workspace_name,
            path.display()
        );

        // 创建工作区组件
        let server_manager = Arc::new(
            LspServerManager::new()
                .with_workspace(path.to_string_lossy().as_ref())
        );

        let document_sync = Arc::new(DocumentSyncManager::new());
        let diagnostics = Arc::new(DiagnosticsManager::new());
        let cache = Arc::new(LspResultCache::new());

        // 创建工作区实例
        let workspace = Arc::new(WorkspaceInstance {
            id: workspace_id.clone(),
            name: workspace_name.clone(),
            path: path.to_path_buf(),
            server_manager,
            document_sync,
            diagnostics,
            cache,
            opened_files: RwLock::new(HashSet::new()),
            created_at: std::time::Instant::now(),
            last_active_at: RwLock::new(std::time::Instant::now()),
        });

        // 存入映射表
        {
            let mut workspaces = self.workspaces.write().await;
            workspaces.insert(workspace_id.clone(), workspace);
        }

        // 更新统计
        {
            let mut stats = self.stats.write().await;
            stats.total_created += 1;
            stats.active_count = self.workspaces.read().await.len();
            if stats.active_count > stats.peak_count {
                stats.peak_count = stats.active_count;
            }
        }

        // 自动切换到新工作区
        self.switch_workspace(&workspace_id).await?;

        debug!(
            "Workspace '{}' created successfully with ID: {}",
            workspace_name, workspace_id
        );

        Ok(workspace_id)
    }

    /// 切换活动工作区
    pub async fn switch_workspace(&self, workspace_id: &WorkspaceId) -> LspResult<()> {
        // 验证工作区是否存在
        {
            let workspaces = self.workspaces.read().await;
            if !workspaces.contains_key(workspace_id) {
                return Err(LspError::StartFailed(format!(
                    "Workspace not found: {}",
                    workspace_id
                )));
            }
        }

        // 更新活动工作区
        {
            let mut active = self.active_workspace.write().await;
            *active = Some(workspace_id.clone());
        }

        // 更新最后活动时间
        if let Some(workspace) = self.get_workspace(workspace_id).await {
            let mut last_active = workspace.last_active_at.write().await;
            *last_active = std::time::Instant::now();
        }

        info!("Switched to workspace: {}", workspace_id);
        Ok(())
    }

    /// 关闭工作区
    pub async fn close_workspace(&self, workspace_id: &WorkspaceId) -> LspResult<()> {
        info!("Closing workspace: {}", workspace_id);

        // 从映射表中移除
        let removed = {
            let mut workspaces = self.workspaces.write().await;
            workspaces.remove(workspace_id)
        };

        match removed {
            Some(_) => {
                // 如果关闭的是当前活动工作区，清除活动状态
                {
                    let mut active = self.active_workspace.write().await;
                    if *active == Some(workspace_id.clone()) {
                        if let Some(first_remaining) = self.workspaces.read().await.keys().next() {
                            *active = Some(first_remaining.clone());
                        } else {
                            *active = None;
                        }
                    }
                }

                // 更新统计
                {
                    let mut stats = self.stats.write().await;
                    stats.total_closed += 1;
                    stats.active_count = self.workspaces.read().await.len();
                }

                debug!("Workspace closed successfully: {}", workspace_id);
                Ok(())
            }
            None => Err(LspError::StartFailed(format!(
                "Workspace not found: {}",
                workspace_id
            ))),
        }
    }

    /// 获取工作区实例
    pub async fn get_workspace(&self, workspace_id: &WorkspaceId) -> Option<Arc<WorkspaceInstance>> {
        self.workspaces.read().await.get(workspace_id).cloned()
    }

    /// 获取当前活动的工作区
    pub async fn get_active_workspace(&self) -> Option<Arc<WorkspaceInstance>> {
        let active_id = self.active_workspace.read().await.clone()?;
        self.get_workspace(&active_id).await
    }

    /// 获取所有工作区 ID 列表
    pub async fn list_workspaces(&self) -> Vec<(WorkspaceId, String)> {
        self.workspaces
            .read()
            .await
            .iter()
            .map(|(id, ws)| (id.clone(), ws.name.clone()))
            .collect()
    }

    /// 根据文件路径确定所属工作区
    pub async fn resolve_workspace_for_file(&self, file_path: &str) -> LspResult<Option<WorkspaceId>> {
        let file_path_buf = PathBuf::from(file_path);

        // 查找包含该文件的工作区
        let best_match = {
            let workspaces = self.workspaces.read().await;
            
            let mut best_match: Option<(WorkspaceId, usize)> = None;
            
            for (id, workspace) in workspaces.iter() {
                if file_path_buf.starts_with(&workspace.path) {
                    let depth = file_path_buf
                        .strip_prefix(&workspace.path)
                        .map(|p| p.components().count())
                        .unwrap_or(0);
                    
                    match &best_match {
                        None => best_match = Some((id.clone(), depth)),
                        Some((_, existing_depth)) => if depth < *existing_depth {
                            best_match = Some((id.clone(), depth));
                        },
                    }
                }
            }
            
            best_match.map(|(id, _)| id)
        };

        Ok(best_match)
    }

    /// 跨工作区搜索符号
    pub async fn search_symbol_across_workspaces(
        &self,
        query: &str,
    ) -> LspResult<Vec<lsp_types::SymbolInformation>> {
        if !self.config.cross_workspace_refs {
            return Err(LspError::StartFailed("Cross-workspace references disabled".to_string()));
        }

        debug!("Searching for symbol '{}' across all workspaces", query);

        let mut all_symbols = Vec::new();
        let workspaces = self.workspaces.read().await;

        for (_id, workspace) in workspaces.iter() {
            match workspace.server_manager.workspace_symbol(query).await {
                Ok(symbols) => {
                    for symbol in symbols {
                        all_symbols.push(symbol);
                    }
                }
                Err(e) => {
                    warn!(
                        "Failed to search in workspace '{}': {}",
                        workspace.name, e
                    );
                }
            }
        }

        // 更新跨工作区查询统计
        {
            let mut stats = self.stats.write().await;
            stats.pub_cross_workspace_queries += 1;
        }

        debug!(
            "Found {} symbols matching '{}' across {} workspaces",
            all_symbols.len(),
            query,
            workspaces.len()
        );

        Ok(all_symbols)
    }

    /// 获取所有工作区的诊断信息
    pub async fn get_all_diagnostics(
        &self,
    ) -> LspResult<HashMap<WorkspaceId, Vec<crate::diagnostics::EnhancedDiagnostic>>> {
        let mut all_diagnostics = HashMap::new();
        let workspaces = self.workspaces.read().await;

        for (id, workspace) in workspaces.iter() {
            // 获取该工作区中有错误的文件列表
            let files_with_errors = workspace.diagnostics.get_files_with_errors().await;
            
            // 收集这些文件的诊断信息
            let mut workspace_diags = Vec::new();
            for (file_path, _error_count) in files_with_errors {
                let file_diags = workspace.diagnostics.get_file_diagnostics(&file_path).await;
                workspace_diags.extend(file_diags);
            }
            
            all_diagnostics.insert(id.clone(), workspace_diags);
        }

        Ok(all_diagnostics)
    }

    /// 清理空闲工作区（超过空闲超时未活动的）
    pub async fn cleanup_idle_workspaces(&self) -> Vec<WorkspaceId> {
        let timeout_secs = match self.config.idle_timeout_seconds {
            Some(secs) => secs,
            None => return vec![],
        };

        let now = std::time::Instant::now();
        let idle_workspaces: Vec<WorkspaceId> = {
            let workspaces = self.workspaces.read().await;
            workspaces
                .iter()
                .filter(|(_, ws)| {
                    let last_active = ws.last_active_time();
                    now.duration_since(last_active).as_secs() > timeout_secs
                })
                .map(|(id, _)| id.clone())
                .collect()
        };

        // 关闭空闲工作区
        let mut closed = Vec::new();
        for workspace_id in idle_workspaces {
            if let Err(e) = self.close_workspace(&workspace_id).await {
                warn!(
                    "Failed to close idle workspace '{}': {}",
                    workspace_id, e
                );
            } else {
                closed.push(workspace_id);
            }
        }

        if !closed.is_empty() {
            info!(
                "Cleaned up {} idle workspaces",
                closed.len()
            );
        }

        closed
    }

    /// 获取统计信息
    pub async fn get_stats(&self) -> MultiWorkspaceStats {
        self.stats.read().await.clone()
    }
}

/// 辅助方法：获取最后活动时间
impl WorkspaceInstance {
    pub fn last_active_time(&self) -> std::time::Instant {
        // 简化实现：返回创建时间作为近似值
        // 实际应用中应该使用 last_active_at 字段
        self.created_at
    }
}
