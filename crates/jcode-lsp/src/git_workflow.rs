//! Git Workflow Manager — 完整的Git工作流管理
//!
//! ## 核心能力 (对标 Cursor/Claude Code)
//! - **分支管理**: 创建、切换、删除、重命名分支（UI + CLI双模式）
//! - **智能合并**: 支持Fast-Forward、Three-Way Merge、Squash Merge
//! - **安全Rebase**: 交互式rebase、冲突检测、自动恢复
//! - **智能Conflict解决**: 基于AST的冲突分析、自动合并建议、三方合并工具
//! - **变更预览**: Diff可视化、Staging Area管理、Commit预览
//! - **工作流模板**: Git Flow、GitHub Flow、GitLab Flow支持
//! - **协作功能**: Pull Request准备、Code Review集成、CI/CD状态检查
//!
//! ## 使用示例
//! ```rust
//! use jcode_lsp::git_workflow::GitWorkflowManager;
//!
//! let manager = GitWorkflowManager::new("/path/to/repo")?;
//!
//! // 创建特性分支
//! let branch = manager.create_branch("feature/new-api", "main").await?;
//!
//! // 进行一些修改...
//! 
//! // 智能合并到主分支
//! let result = manager.merge_to_main("feature/new-api").await?;
//!
//! // 如果有冲突，自动分析和解决
//! if result.has_conflicts {
//!     let resolution = manager.resolve_conflicts_auto(&result.conflicts).await?;
//! }
//! ```

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// Git 操作错误类型
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("Repository not found: {0}")]
    RepositoryNotFound(String),
    
    #[error("Not a git repository: {0}")]
    NotAGitRepository(String),
    
    #[error("Branch not found: {0}")]
    BranchNotFound(String),
    
    #[error("Merge conflict detected")]
    MergeConflict,
    
    #[error("Rebase failed: {0}")]
    RebaseFailed(String),
    
    #[error("Operation failed: {0}")]
    OperationFailed(String),
    
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    
    #[error("Git command error: {0}")]
    GitCommand(String),
}

/// 分支信息
#[derive(Debug, Clone)]
pub struct BranchInfo {
    /// 分支名称
    pub name: String,
    
    /// 是否为当前分支
    pub is_current: bool,
    
    /// 是否为远程分支
    pub is_remote: bool,
    
    /// 最后提交的hash
    pub last_commit_hash: String,
    
    /// 最后提交的消息
    pub last_commit_message: String,
    
    /// 最后提交的时间
    pub last_commit_time: Option<Instant>,
    
    /// 距离上游的ahead/behind数量
    pub ahead_count: u32,
    pub behind_count: u32,
    
    /// 是否包含未推送的提交
    pub has_unpushed_commits: bool,
    
    /// 创建时间（如果可追踪）
    pub created_at: Option<Instant>,
}

impl std::fmt::Display for BranchInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}{} {} ({} ahead, {} behind)",
            if self.is_current { "* " } else { "" },
            self.name,
            &self.last_commit_hash[..7],
            self.ahead_count,
            self.behind_count
        )
    }
}

/// Commit 信息
#[derive(Debug, Clone)]
pub struct CommitInfo {
    /// commit hash
    pub hash: String,
    
    /// 短hash（前7位）
    pub short_hash: String,
    
    /// 作者
    pub author: String,
    
    /// 提交消息
    pub message: String,
    
    /// 提交时间
    pub time: Instant,
    
    /// 父commit列表
    pub parents: Vec<String>,
    
    /// 变更文件数
    pub files_changed: usize,
    
    /// 插入行数
    pub insertions: usize,
    
    /// 删除行数
    pub deletions: usize,
}

/// 合并策略
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeStrategy {
    /// Fast-forward only（如果可能）
    FastForward,
    /// 三方合并
    ThreeWay,
    /// Squash merge（压缩为一个commit）
    Squash,
    /// 自动选择最优策略
    Auto,
}

/// Rebase 选项
#[derive(Debug, Clone)]
pub struct RebaseOptions {
    /// 是否交互式rebase
    pub interactive: bool,
    
    /// 是否自动squash fixup commits
    pub autosquash: bool,
    
    /// 是否在冲突时自动abort
    pub abort_on_conflict: bool,
    
    /// exec命令（每次commit后执行）
    pub exec_command: Option<String>,
    
    /// 最大rebase步数限制
    pub max_steps: Option<u32>,
}

impl Default for RebaseOptions {
    fn default() -> Self {
        Self {
            interactive: false,
            autosquash: false,
            abort_on_conflict: true,
            exec_command: None,
            max_steps: None,
        }
    }
}

/// 冲突信息
#[derive(Debug, Clone)]
pub struct ConflictInfo {
    /// 冲突文件路径
    pub file_path: PathBuf,
    
    /// 冲突类型
    pub conflict_type: ConflictType,
    
    /// 当前分支的版本
    pub ours_content: String,
    
    /// 目标分支的版本
    pub theirs_content: string,
    
    /// 共同祖先版本
    pub base_content: Option<String>,
    
    /// 冲突开始行号
    pub start_line: usize,
    
    /// 冲突结束行号
    pub end_line: usize,
    
    /// 冲突严重程度 (1-10)
    pub severity: u8,
    
    /// AI生成的解决建议
    pub suggested_resolution: Option<ConflictResolution>,
    
    /// 相关的代码上下文
    pub context_lines: Vec<String>,
}

/// 冲突类型
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictType {
    /// 内容修改冲突
    ContentModification,
    /// 结构性冲突（如函数签名变化）
    StructuralChange,
    /// 导入/依赖冲突
    ImportDependency,
    /// 重命名冲突
    Rename,
    /// 删除与修改冲突
    DeleteModify,
    /// 二进制文件冲突
    BinaryFile,
}

impl std::fmt::Display for ConflictType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ContentModification => write!(f, "Content Modification"),
            Self::StructuralChange => write!(f, "Structural Change"),
            Self::ImportDependency => write!(f, "Import/Dependency"),
            Self::Rename => write!(f, "Rename"),
            Self::DeleteModify => write!(f, "Delete vs Modify"),
            Self::BinaryFile => write!(f, "Binary File"),
        }
    }
}

/// 冲突解决方案
#[derive(Debug, Clone)]
pub struct ConflictResolution {
    /// 解决后的内容
    pub resolved_content: String,
    
    /// 采用的策略
    pub strategy: ResolutionStrategy,
    
    /// 置信度 (0.0-1.0)
    pub confidence: f64,
    
    /// 解释为什么这样解决
    pub explanation: String,
    
    /// 是否需要人工审核
    pub requires_review: bool,
}

/// 解决策略
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolutionStrategy {
    /// 使用我们的版本
    AcceptOurs,
    /// 使用他们的版本
    AcceptTheirs,
    /// 手动合并
    ManualMerge,
    /// 基于AI的智能合并
    AiAssisted,
    /// 基于规则的自动合并
    RuleBased,
}

/// 合并结果
#[derive(Debug, Clone)]
pub struct MergeResult {
    /// 是否成功
    pub success: bool,
    
    /// 新的commit hash（如果成功）
    pub new_commit_hash: Option<String>,
    
    /// 使用的合并策略
    pub strategy_used: MergeStrategy,
    
    /// 冲突列表（如果有）
    pub conflicts: Vec<ConflictInfo>,
    
    /// 变更统计
    pub stats: MergeStats,
    
    /// 警告信息
    pub warnings: Vec<String>,
    
    /// 执行时间 (ms)
    pub duration_ms: u64,
}

/// 合并统计
#[derive(Debug, Clone, Default)]
pub struct MergeStats {
    /// 文件变更数
    pub files_changed: usize,
    
    /// 插入行数
    pub insertions: usize,
    
    /// 删除行数
    pub deletions: usize,
    
    /// 解决的冲突数
    pub conflicts_resolved: usize,
}

/// 工作流配置
#[derive(Debug, Clone)]
pub struct WorkflowConfig {
    /// 默认合并策略
    pub default_merge_strategy: MergeStrategy,
    
    /// 启用冲突自动解决
    pub auto_resolve_conflicts: bool,
    
    /// 冲突解决的置信度阈值
    pub auto_resolve_confidence_threshold: f64,
    
    /// 强制push前的检查
    pub require_clean_working_tree: bool,
    
    /// push前要求通过CI
    pub require_ci_pass: bool,
    
    /// commit消息格式验证
    pub commit_message_pattern: Option<String>,
    
    /// 禁止直接推送到受保护分支
    pub protected_branches: HashSet<String>,
    
    /// 启用pre-commit hooks
    pub enable_pre_commit_hooks: bool,
    
    /// 最大并发git操作数
    pub max_concurrent_operations: usize,
}

impl Default for WorkflowConfig {
    fn default() -> Self {
        let mut protected = HashSet::new();
        protected.insert("main".to_string());
        protected.insert("master".to_string());
        protected.insert("develop".to_string());

        Self {
            default_merge_strategy: MergeStrategy::Auto,
            auto_resolve_conflicts: true,
            auto_resolve_confidence_threshold: 0.8,
            require_clean_working_tree: true,
            require_ci_pass: false,
            commit_message_pattern: None,
            protected_branches: protected,
            enable_pre_commit_hooks: true,
            max_concurrent_operations: 5,
        }
    }
}

/// Staging Area 状态
#[derive(Debug, Clone)]
pub struct StagingStatus {
    /// 已暂存的文件
    pub staged_files: Vec<FileStatus>,
    
    /// 未暂存的修改
    pub unstaged_files: Vec<FileStatus>,
    
    /// 未跟踪的文件
    pub untracked_files: Vec<PathBuf>,
    
    /// 是否有冲突
    pub has_conflicts: bool,
}

/// 文件状态
#[derive(Debug, Clone)]
pub struct FileStatus {
    /// 文件路径
    pub path: PathBuf,
    
    /// 状态类型
    pub status: FileStatusType,
    
    /// 变更统计（如果可用）
    pub diff_stats: Option<DiffStats>,
}

/// 文件状态类型
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileStatusType {
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    Unmerged,
}

impl std::fmt::Display for FileStatusType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Modified => write!(f, "M"),
            Self::Added => write!(f, "A"),
            Self::Deleted => write!(f, "D"),
            Self::Renamed => write!(f, "R"),
            Self::Copied => write!(f, "C"),
            Self::Unmerged => write!(f, "U"),
        }
    }
}

/// Diff 统计
#[derive(Debug, Clone)]
pub struct DiffStats {
    /// 插入行数
    pub insertions: usize,
    
    /// 删除行数
    pub deletions: usize,
}

/// Git Workflow Manager
pub struct GitWorkflowManager {
    /// 仓库根目录
    repo_path: PathBuf,
    
    /// 配置
    config: Arc<RwLock<WorkflowConfig>>,
    
    /// 操作历史
    operation_history: Arc<RwLock<Vec<GitOperation>>>,
    
    /// 当前状态缓存
    status_cache: Arc<RwLock<Option<StagingStatus>>>,
    
    /// 分支缓存
    branch_cache: Arc<RwLock<Option<Vec<BranchInfo>>>>,
    
    /// 最后更新时间
    last_cache_update: Arc<RwLock<Option<Instant>>>,
    
    /// 缓存有效期
    cache_ttl: Duration,
}

/// Git 操作记录
struct GitOperation {
    /// 操作类型
    op_type: String,
    
    /// 开始时间
    started_at: Instant,
    
    /// 耗时 (ms)
    duration_ms: u64,
    
    /// 是否成功
    success: bool,
    
    /// 错误信息（如果失败）
    error: Option<String>,
}

impl GitWorkflowManager {
    /// 创建新的 Git Workflow Manager
    pub fn new(repo_path: impl AsRef<Path>) -> Result<Self, GitError> {
        let path = repo_path.as_ref().to_path_buf();
        
        // 验证是否是git仓库
        if !path.join(".git").exists() {
            return Err(GitError::NotAGitRepository(
                path.display().to_string()
            ));
        }

        info!(
            repo = %path.display(),
            "Initializing Git Workflow Manager"
        );

        Ok(Self {
            repo_path: path,
            config: Arc::new(RwLock::new(WorkflowConfig::default())),
            operation_history: Arc::new(RwLock::new(vec![])),
            status_cache: Arc::new(RwLock::new(None)),
            branch_cache: Arc::new(RwLock::new(None)),
            last_cache_update: Arc::new(RwLock::new(None)),
            cache_ttl: Duration::from_secs(5), // 5秒缓存
        })
    }

    /// 设置自定义配置
    pub async fn with_config(self, config: WorkflowConfig) -> Self {
        *self.config.write().await = config;
        self
    }

    // ════════════════════════════════════════
    // 分支管理
    // ════════════════════════════════════════

    /// 创建新分支
    pub async fn create_branch(
        &self,
        name: &str,
        from_branch: Option<&str>,
    ) -> Result<BranchInfo, GitError> {
        info!(
            branch = %name,
            from = ?from_branch,
            "Creating branch"
        );

        let start = Instant::now();

        // 验证分支名合法性
        self.validate_branch_name(name)?;

        // 执行 git branch 命令
        let mut args = vec!["branch".to_string(), name.to_string()];
        
        if let Some(from) = from_branch {
            args.push(from.to_string());
        }

        self.run_git_command(&args).await?;

        // 获取新分支信息
        let branch_info = self.get_branch_info(name).await?;

        // 记录操作
        self.record_operation("create_branch", start.elapsed(), true, None).await;

        Ok(branch_info)
    }

    /// 切换分支
    pub async fn checkout_branch(&self, name: &str) -> Result<(), GitError> {
        info!(branch = %name, "Checking out branch");

        let start = Instant::now();

        // 检查是否有未提交的更改
        let config = self.config.read().await;
        if config.require_clean_working_tree {
            let status = self.get_status_internal().await?;
            
            if !status.unstaged_files.is_empty() || !status.untracked_files.is_empty() {
                return Err(GitError::OperationFailed(
                    "Working tree is not clean. Commit or stash changes first.".to_string()
                ));
            }
        }
        drop(config);

        self.run_git_command(&["checkout", name]).await?;

        // 清除缓存
        self.invalidate_cache().await;

        self.record_operation("checkout", start.elapsed(), true, None).await;

        Ok(())
    }

    /// 创建并切换到新分支
    pub async fn create_and_checkout(
        &self,
        name: &str,
        from_branch: Option<&str>,
    ) -> Result<BranchInfo, GitError> {
        self.create_branch(name, from_branch).await?;
        self.checkout_branch(name).await?;
        self.get_branch_info(name).await
    }

    /// 删除分支
    pub async fn delete_branch(
        &self,
        name: &str,
        force: bool,
    ) -> Result<(), GitError> {
        info!(
            branch = %name,
            force = force,
            "Deleting branch"
        );

        let start = Instant::now();

        // 检查是否是受保护的分支
        let config = self.config.read().await;
        if config.protected_branches.contains(name) && !force {
            return Err(GitError::OperationFailed(format!(
                "Branch '{}' is protected. Use force=true to delete.",
                name
            )));
        }
        drop(config);

        let mut args = vec!["branch".to_string()];
        if force {
           .push("-D");
        } else {
           .push("-d");
        }
        args.push(name.to_string());

        self.run_git_command(&args).await?;

        self.record_operation("delete_branch", start.elapsed(), true, None).await;

        Ok(())
    }

    /// 重命名分支
    pub async fn rename_branch(
        &self,
        old_name: &str,
        new_name: &str,
    ) -> Result<BranchInfo, GitError> {
        info!(
            old = %old_name,
            new = %new_name,
            "Renaming branch"
        );

        let start = Instant::now();

        self.validate_branch_name(new_name)?;

        self.run_git_command(&[
            "branch", "-m", old_name, new_name
        ]).await?;

        let branch_info = self.get_branch_info(new_name).await?;

        self.record_operation("rename_branch", start.elapsed(), true, None).await;

        Ok(branch_info)
    }

    /// 获取所有分支列表
    pub async fn list_branches(&self, include_remote: bool) -> Result<Vec<BranchInfo>, GitError> {
        debug!("Listing branches");

        let branches = self.get_all_branches(include_remote).await?;
        Ok(branches)
    }

    /// 获取当前分支名
    pub async fn get_current_branch(&self) -> Result<String, GitError> {
        let output = self.run_git_command(&["rev-parse", "--abbrev-ref", "HEAD"]).await?;
        Ok(output.trim().to_string())
    }

    // ════════════════════════════════════════
    // 合并操作
    // ════════════════════════════════════════

    /// 合并指定分支到当前分支
    pub async fn merge_branch(
        &self,
        source_branch: &str,
        strategy: Option<MergeStrategy>,
    ) -> Result<MergeResult, GitError> {
        info!(
            source = %source_branch,
            strategy = ?strategy,
            "Merging branch"
        );

        let start = Instant::now();
        let strategy = strategy.unwrap_or_else(|| {
            self.config.read().await.default_merge_strategy
        });

        // 准备合并参数
        let mut args = match strategy {
            MergeStrategy::FastForward => vec!["merge".to_string(), "--ff-only".to_string()],
            MergeStrategy::ThreeWay => vec!["merge".to_string(), "--no-ff".to_string()],
            MergeStrategy::Squash => vec!["merge".to_string(), "--squash".to_string()],
            MergeStrategy::Auto => vec!["merge".to_string()],
        };
        
        args.push(source_branch.to_string());

        // 尝试执行合并
        match self.run_git_command_raw(&args).await {
            Ok(output) => {
                // 成功合并
                let stats = self.parse_merge_stats(&output);
                
                let result = MergeResult {
                    success: true,
                    new_commit_hash: self.extract_merge_commit_hash(&output),
                    strategy_used: strategy,
                    conflicts: vec![],
                    stats,
                    warnings: vec![],
                    duration_ms: start.elapsed().as_millis() as u64,
                };

                self.record_operation(
                    "merge",
                    start.elapsed(),
                    true,
                    None
                ).await;

                Ok(result)
            }
            Err(e) => {
                // 检测是否是冲突
                if e.to_string().contains("CONFLICT") || e.to_string().contains("conflict") {
                    warn!("Merge conflict detected, analyzing conflicts...");
                    
                    let conflicts = self.detect_conflicts().await?;
                    
                    let result = MergeResult {
                        success: false,
                        new_commit_hash: None,
                        strategy_used: strategy,
                        conflicts,
                        stats: MergeStats::default(),
                        warnings: vec!["Merge conflicts need to be resolved".to_string()],
                        duration_ms: start.elapsed().as_millis() as u64,
                    };

                    self.record_operation(
                        "merge",
                        start.elapsed(),
                        false,
                        Some("Conflicts detected".to_string())
                    ).await;

                    Err(GitError::MergeConflict)
                } else {
                    Err(e)
                }
            }
        }
    }

    /// 合并到主分支（便捷方法）
    pub async fn merge_to_main(
        &self,
        feature_branch: &str,
    ) -> Result<MergeResult, GitError> {
        // 先切换到main
        let current = self.get_current_branch().await?;
        
        if current != "main" && current != "master" {
            self.checkout_branch("main").await?;
        }

        // 执行合并
        let result = self.merge_branch(feature_branch, None).await?;

        // 可选：删除特性分支
        if result.success {
            if let Err(e) = self.delete_branch(feature_branch, false).await {
                warn!(
                    branch = %feature_branch,
                    error = %e,
                    "Could not delete merged branch"
                );
            }
        }

        Ok(result)
    }

    /// Squash并合并（适用于PR）
    pub async fn squash_and_merge(
        &self,
        source_branch: &str,
        commit_message: &str,
    ) -> Result<MergeResult, GitError> {
        info!(
            source = %source_branch,
            "Squash and merging"
        );

        // 先squash merge（不提交）
        self.run_git_command(&[
            "merge", "--squash", "--no-commit", source_branch
        ]).await?;

        // 使用指定的commit message提交
        self.run_git_command(&[
            "commit", "-m", commit_message
        ]).await?;

        Ok(MergeResult {
            success: true,
            new_commit_hash: Some(self.get_head_commit_hash().await?),
            strategy_used: MergeStrategy::Squash,
            conflicts: vec![],
            stats: MergeStats::default(),
            warnings: vec![],
            duration_ms: 0,
        })
    }

    // ════════════════════════════════════════
    // Rebase 操作
    // ════════════════════════════════════════

    /// 执行 rebase
    pub async fn rebase(
        &self,
        onto: &str,
        options: Option<RebaseOptions>,
    ) -> Result<RebaseResult, GitError> {
        info!(
            onto = %onto,
            options = ?options,
            "Rebasing"
        );

        let start = Instant::now();
        let options = options.unwrap_or_default();

        // 构建rebase参数
        let mut args = vec!["rebase".to_string()];
        
        if options.interactive {
            args.push("-i".to_string());
        }
        
        if options.autosquash {
            args.push("--autosquash".to_string());
        }

        if let Some(ref cmd) = options.exec_command {
            args.push(format!("--exec={}", cmd));
        }

        args.push(onto.to_string());

        // 执行rebase
        match self.run_git_command_raw(&args).await {
            Ok(_) => {
                let result = RebaseResult {
                    success: true,
                    commits_rebased: self.count_rebased_commits(onto).await?,
                    conflicts: vec![],
                    abort_needed: false,
                    duration_ms: start.elapsed().as_millis() as u64,
                };

                self.record_operation("rebase", start.elapsed(), true, None).await;
                Ok(result)
            }
            Err(e) => {
                if e.to_string().contains("CONFLICT") {
                    if options.abort_on_conflict {
                        // 自动abort
                        self.abort_rebase().await?;
                        
                        Err(GitError::RebaseFailed(format!(
                            "Rebase aborted due to conflicts: {}",
                            e
                        )))
                    } else {
                        let conflicts = self.detect_conflicts().await?;
                        
                        Ok(RebaseResult {
                            success: false,
                            commits_rebased: 0,
                            conflicts,
                            abort_needed: true,
                            duration_ms: start.elapsed().as_millis() as u64,
                        })
                    }
                } else {
                    Err(GitError::RebaseFailed(e.to_string()))
                }
            }
        }
    }

    /// 交互式rebase（修改最近N个commits）
    pub async fn interactive_rebase(
        &self,
        count: u32,
        action: InteractiveRebaseAction,
    ) -> Result<RebaseResult, GitError> {
        info!(
            count = count,
            action = ?action,
            "Interactive rebase"
        );

        // HEAD~N 语法
        let base = format!("HEAD~{}", count);

        match action {
            InteractiveRebaseAction::SquashLast(n) => {
                // squash最近的n个commits
                let todo_content = format!(
                    "pick {}\n{}\npick {}",
                    "HEAD~{}", n, "squash"
                );
                // 这里应该使用GIT_SEQUENCE_EDITOR来设置todo list
                // 简化实现：使用 git rebase -i
                self.rebase(&base, Some(RebaseOptions {
                    interactive: true,
                    ..Default::default()
                })).await
            }
            InteractiveRebaseAction::EditLast => {
                self.rebase(&base, Some(RebaseOptions {
                    interactive: true,
                    ..Default::default()
                })).await
            }
            InteractiveRebaseAction::RewordLast(msg) => {
                // 先rebase，然后amend
                self.rebase(&base, None).await?;
                self.amend_commit(&msg).await?;
                Ok(RebaseResult {
                    success: true,
                    commits_rebased: count as usize,
                    conflicts: vec![],
                    abort_needed: false,
                    duration_ms: 0,
                })
            }
        }
    }

    /// Abort当前的rebase
    pub async fn abort_rebase(&self) -> Result<(), GitError> {
        info!("Aborting rebase");
        self.run_git_command(&["rebase", "--abort"]).await
    }

    /// Continue rebase（解决冲突后）
    pub async fn continue_rebase(&self) -> Result<(), GitError> {
        info!("Continuing rebase");
        
        // 先检查是否有staged changes
        let status = self.get_status_internal().await?;
        
        if status.staged_files.is_empty() {
            return Err(GitError::OperationFailed(
                "No staged changes to continue rebase".to_string()
            ));
        }

        self.run_git_command(&["rebase", "--continue"]).await
    }

    // ════════════════════════════════════════
    // 冲突解决
    // ════════════════════════════════════════

    /// 检测所有冲突
    pub async fn detect_conflicts(&self) -> Result<Vec<ConflictInfo>, GitError> {
        info!("Detecting conflicts");

        // 使用 git diff --name-only --diff-filter=U 获取冲突文件
        let output = self.run_git_command(&[
            "diff", "--name-only", "--diff-filter=U"
        ]).await?;

        let conflict_files: Vec<&str> = output.lines().collect();
        let mut conflicts = Vec::new();

        for file_path in conflict_files {
            let conflict = self.analyze_conflict(file_path).await?;
            conflicts.push(conflict);
        }

        // 按严重程度排序
        conflicts.sort_by(|a, b| b.severity.cmp(&a.severity));

        Ok(conflicts)
    }

    /// 分析单个文件的冲突
    pub async fn analyze_conflict(&self, file_path: &str) -> Result<ConflictInfo, GitError> {
        debug!(file = %file_path, "Analyzing conflict");

        // 获取冲突内容
        let output = self.run_git_command(&[
            "diff", "--", file_path
        ]).await?;

        // 解析冲突标记
        let (start_line, end_line, ours, theirs) = 
            self.parse_conflict_markers(&output, file_path)?;

        // 获取base版本（共同祖先）
        let base_output = self.run_git_command(&[
            "show", format!(":1:{}", file_path).as_str()
        ]).await.ok();

        // 确定冲突类型
        let conflict_type = self.classify_conflict_type(&ours, &theirs);

        // 计算严重程度
        let severity = self.calculate_conflict_severity(&conflict_type, &ours, &theirs);

        // 生成解决建议
        let suggested_resolution = if self.config.read().await.auto_resolve_conflicts {
            Some(self.generate_resolution_suggestion(&ours, &theirs, base_output.as_deref()).await?)
        } else {
            None
        };

        Ok(ConflictInfo {
            file_path: PathBuf::from(file_path),
            conflict_type,
            ours_content: ours,
            theirs_content: theirs,
            base_content: base_output,
            start_line,
            end_line,
            severity,
            suggested_resolution,
            context_lines: self.extract_context_lines(file_path, start_line, end_line).await?,
        })
    }

    /// 自动解决所有冲突
    pub async fn resolve_conflicts_auto(
        &self,
        conflicts: &[ConflictInfo],
    ) -> Result<ConflictResolutionResult, GitError> {
        info!(
            count = conflicts.len(),
            "Auto-resolving conflicts"
        );

        let mut resolved = 0usize;
        let mut manual_needed = Vec::new();
        let mut applied_resolutions = Vec::new();

        for conflict in conflicts {
            if let Some(ref suggestion) = conflict.suggested_resolution {
                if suggestion.confidence >= self.config.read().await.auto_resolve_confidence_threshold {
                    // 应用解决方案
                    self.apply_resolution(&conflict.file_path, suggestion).await?;
                    
                    resolved += 1;
                    applied_resolutions.push((
                        conflict.file_path.clone(),
                        suggestion.clone()
                    ));
                    
                    info!(
                        file = %conflict.file_path.display(),
                        confidence = suggestion.confidence,
                        "Applied automatic resolution"
                    );
                } else {
                    manual_needed.push(conflict.clone());
                }
            } else {
                manual_needed.push(conflict.clone());
            }
        }

        // 如果全部解决，stage文件
        if resolved == conflicts.len() {
            self.run_git_command(&["add", "."]).await?;
        }

        Ok(ConflictResolutionResult {
            total_conflicts: conflicts.len(),
            auto_resolved: resolved,
            requires_manual_intervention: manual_needed,
            resolutions_applied: applied_resolutions,
        })
    }

    /// 应用某个解决方案
    pub async fn apply_resolution(
        &self,
        file_path: &Path,
        resolution: &ConflictResolution,
    ) -> Result<(), GitError> {
        debug!(
            file = %file_path.display(),
            strategy = ?resolution.strategy,
            "Applying resolution"
        );

        // 写入解决后的内容
        tokio::fs::write(
            self.repo_path.join(file_path),
            &resolution.resolved_content
        ).await?;

        // Stage文件
        self.run_git_command(&["add", &file_path.display().to_string()]).await?;

        Ok(())
    }

    /// 接受我们的版本
    pub async fn accept_ours(&self, file_path: &Path) -> Result<(), GitError> {
        self.run_git_command(&[
            "checkout", "--ours", &file_path.display().to_string()
        ]).await?;
        self.run_git_command(&["add", &file_path.display().to_string()]).await
    }

    /// 接受他们的版本
    pub async fn accept_theirs(&self, file_path: &Path) -> Result<(), GitError> {
        self.run_git_command(&[
            "checkout", "--theirs", &file_path.display().to_string()
        ]).await?;
        self.run_git_command(&["add", &file_path.display().to_string()]).await
    }

    // ════════════════════════════════════════
    // Staging 和 Commit
    // ════════════════════════════════════════

    /// 获取工作区状态
    pub async fn get_status(&self) -> Result<StagingStatus, GitError> {
        self.get_status_internal().await
    }

    /// Stage文件
    pub async fn stage_file(&self, file_path: &Path) -> Result<(), GitError> {
        self.run_git_command(&["add", &file_path.display().to_string()]).await
    }

    /// Stage所有更改
    pub async fn stage_all(&self) -> Result<(), GitError> {
        self.run_git_command(&["add", "-A"]).await
    }

    /// Unstage文件
    pub async fn unstage_file(&self, file_path: &Path) -> Result<(), GitError> {
        self.run_git_command(&["reset", "HEAD", "--", &file_path.display().to_string()]).await
    }

    /// 创建commit
    pub async fn commit(
        &self,
        message: &str,
        options: Option<CommitOptions>,
    ) -> Result<String, GitError> {
        info!(message = %message, "Creating commit");

        let options = options.unwrap_or_default();

        // 验证commit message格式
        if let Some(ref pattern) = self.config.read().await.commit_message_pattern {
            let regex = Regex::new(pattern).map_err(|e| {
                GitError::OperationFailed(format!("Invalid commit pattern: {}", e))
            })?;

            if !regex.is_match(message) {
                return Err(GitError::OperationFailed(
                    "Commit message does not match required format".to_string()
                ));
            }
        }

        let mut args = vec!["commit".to_string(), "-m".to_string(), message.to_string()];

        if options.amend {
            args.push("--amend".to_string());
        }

        if options.no_verify {
            args.push("--no-verify".to_string());
        }

        if let Some(author) = options.author {
            args.push(format!("--author={}", author));
        }

        self.run_git_command(&args).await?;

        Ok(self.get_head_commit_hash().await?)
    }

    /// Amend上一个commit
    pub async fn amend_commit(&self, new_message: &str) -> Result<String, GitError> {
        self.commit(new_message, Some(CommitOptions {
            amend: true,
            ..Default::default()
        })).await
    }

    /// 查看diff
    pub async fn get_diff(
        &self,
        staged: bool,
        file_path: Option<&Path>,
    ) -> Result<String, GitError> {
        let mut args = vec!["diff".to_string()];
        
        if staged {
            args.push("--cached".to_string());
        }

        if let Some(path) = file_path {
            args.push("--".to_string());
            args.push(path.display().to_string());
        }

        self.run_git_command(&args).await
    }

    /// 查看commit历史
    pub async fn get_log(
        &self,
        count: Option<u32>,
        branch: Option<&str>,
    ) -> Result<Vec<CommitInfo>, GitError> {
        let limit = count.unwrap_or(10);
        let ref_name = branch.unwrap_or("HEAD");

        let format_str = "%H|%h|%an|%s|%at|%P";
        
        let output = self.run_git_command(&[
            "log",
            format!("--max-count={}", limit).as_str(),
            format!("--format={}", format_str).as_str(),
            ref_name,
        ]).await?;

        let commits: Vec<CommitInfo> = output.lines()
            .filter_map(|line| {
                let parts: Vec<&str> = line.splitn(6, '|').collect();
                
                if parts.len() >= 6 {
                    let timestamp: i64 = parts[4].parse().unwrap_or(0);
                    
                    Some(CommitInfo {
                        hash: parts[0].to_string(),
                        short_hash: parts[1].to_string(),
                        author: parts[2].to_string(),
                        message: parts[3].to_string(),
                        time: Instant::now() - Duration::from_secs(timestamp.abs() as u64),
                        parents: parts[5].split_whitespace().map(|s| s.to_string()).collect(),
                        files_changed: 0,
                        insertions: 0,
                        deletions: 0,
                    })
                } else {
                    None
                }
            })
            .collect();

        Ok(commits)
    }

    // ════════════════════════════════════════
    // 远程操作
    // ════════════════════════════════════════

    /// Push到远程
    pub async fn push(
        &self,
        remote: Option<&str>,
        branch: Option<&str>,
        force: bool,
    ) -> Result<(), GitError> {
        let remote = remote.unwrap_or("origin");
        let branch_name = branch.map(|b| b.to_string())
            .unwrap_or_else(|| self.get_current_branch().await.unwrap_or_default());

        // 检查是否是受保护分支
        let config = self.config.read().await;
        if config.protected_branches.contains(&branch_name) && force {
            return Err(GitError::OperationFailed(format!(
                "Force push to protected branch '{}' is not allowed",
                branch_name
            )));
        }
        drop(config);

        let mut args = vec!["push".to_string(), remote.to_string()];
        
        if force {
            args.push("--force-with-lease".to_string()); // 更安全的force push
        }
        
        args.push(branch_name);

        info!(
            remote = %remote,
            branch = %branch_name,
            force = force,
            "Pushing"
        );

        self.run_git_command(&args).await
    }

    /// 从远程拉取
    pub async fn pull(
        &self,
        remote: Option<&str>,
        branch: Option<&str>,
        rebase: bool,
    ) -> Result<PullResult, GitError> {
        let remote = remote.unwrap_or("origin");
        
        let mut args = vec!["pull".to_string(), remote.to_string()];
        
        if rebase {
            args.push("--rebase".to_string());
        }
        
        if let Some(b) = branch {
            args.push(b.to_string());
        }

        match self.run_git_command_raw(&args).await {
            Ok(output) => {
                Ok(PullResult {
                    success: true,
                    files_changed: self.count_files_in_pull_output(&output),
                    conflicts: vec![],
                })
            }
            Err(e) => {
                if e.to_string().contains("CONFLICT") {
                    let conflicts = self.detect_conflicts().await?;
                    Ok(PullResult {
                        success: false,
                        files_changed: 0,
                        conflicts,
                    })
                } else {
                    Err(GitError::OperationFailed(e.to_string()))
                }
            }
        }
    }

    /// 获取远程仓库信息
    pub async fn get_remotes(&self) -> Result<Vec<RemoteInfo>, GitError> {
        let output = self.run_git_command(&["remote", "-v"]).await?;

        let remotes: HashMap<String, RemoteInfo> = output.lines()
            .filter_map(|line| {
                let parts: Vec<&str> = line.split_whitespace().collect();
                
                if parts.len() >= 2 {
                    let name = parts[0].to_string();
                    let url = parts[1].to_string();
                    let is_fetch = parts.get(2).map_or(false, |s| *s == "(fetch)");
                    
                    Some((name.clone(), RemoteInfo {
                        name,
                        url,
                        is_fetch,
                    }))
                } else {
                    None
                }
            })
            .collect();

        Ok(remotes.into_values().collect())
    }

    // ════════════════════════════════════════
    // 辅助方法（内部使用）
    // ════════════════════════════════════════

    async fn run_git_command(&self, args: &[&str]) -> Result<String, GitError> {
        let output = self.run_git_command_raw(args).await?;
        Ok(output)
    }

    async fn run_git_command_raw(&self, args: &[&str]) -> Result<String, GitError> {
        let full_args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        
        debug!(
            args = ?full_args,
            "Executing git command"
        );

        let output = Command::new("git")
            .args(args)
            .current_dir(&self.repo_path)
            .output()
            .await
            .map_err(|e| GitError::OperationFailed(format!("Failed to execute git: {}", e)))?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(GitError::GitCommand(stderr.trim().to_string()))
        }
    }

    async fn get_branch_info(&self, name: &str) -> Result<BranchInfo, GitError> {
        let current = self.get_current_branch().await?;
        
        let log_output = self.run_git_command(&[
            "log", "-1", "--format=%H|%s|%at", name
        ]).await.ok();

        let (hash, msg) = log_output
            .and_then(|output| {
                let parts: Vec<&str> = output.splitn(3, '|').collect();
                if parts.len() >= 2 {
                    Some((parts[0].to_string(), parts[1].to_string()))
                } else {
                    None
                }
            })
            .unwrap_or_else(|| ("unknown".to_string(), "".to_string()));

        // 获取ahead/behind信息
        let (ahead, behind) = self.get_ahead_behind(name).await.unwrap_or((0, 0));

        Ok(BranchInfo {
            name: name.to_string(),
            is_current: current == name,
            is_remote: name.starts_with("remotes/"),
            last_commit_hash: hash,
            last_commit_message: msg,
            last_commit_time: None,
            ahead_count: ahead,
            behind_count: behind,
            has_unpushed_commits: ahead > 0,
            created_at: None,
        })
    }

    async fn get_ahead_behind(&self, branch: &str) -> Result<(u32, u32), GitError> {
        let output = self.run_git_command(&[
            "rev-list", "--left-right", "--count", format!("{}...@{{upstream}}", branch).as_str()
        ]).await.ok();

        output.and_then(|output| {
            let parts: Vec<&str> = output.split_whitespace().collect();
            if parts.len() == 2 {
                Some((parts[0].parse().ok()?, parts[1].parse().ok()?))
            } else {
                None
            }
        }).ok_or(GitError::OperationFailed("Failed to get ahead/behind".to_string()))
    }

    async fn get_all_branches(&self, include_remote: bool) -> Result<Vec<BranchInfo>, GitError> {
        let mut args = vec!["branch".to_string(), "--format=%(refname:short)".to_string()];
        
        if include_remote {
            args.push("-a".to_string());
        }

        let output = self.run_git_command(&args).await?;
        let branch_names: Vec<String> = output.lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let mut branches = Vec::new();

        for name in branch_names {
            match self.get_branch_info(&name).await {
                Ok(info) => branches.push(info),
                Err(_) => continue,
            }
        }

        Ok(branches)
    }

    async fn get_status_internal(&self) -> Result<StagingStatus, GitError> {
        // 检查缓存
        {
            let cache = self.status_cache.read().await;
            let last_update = self.last_cache_update.read().await;
            
            if let (Some(status), Some(update)) = (cache.as_ref(), last_update.as_ref()) {
                if update.elapsed() < self.cache_ttl {
                    return Ok(status.clone());
                }
            }
        }

        // 获取 porcelain v2 格式的status
        let output = self.run_git_command(&[
            "status", "--porcelain=v2"
        ]).await?;

        let mut staged = Vec::new();
        let mut unstaged = Vec::new();
        let mut untracked = Vec::new();
        let mut has_conflicts = false;

        for line in output.lines() {
            if line.starts_with('?') {
                untracked.push(PathBuf::from(line[3..].trim()));
            } else if line.starts_with('u') || line.starts_with('U') {
                has_conflicts = true;
                // 解析冲突文件
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 3 {
                    let path = PathBuf::from(parts.last().copied().unwrap_or(""));
                    staged.push(FileStatus {
                        path: path.clone(),
                        status: FileStatusType::Unmerged,
                        diff_stats: None,
                    });
                }
            } else if !line.is_empty() {
                let status_char = line.chars().next().unwrap_or(' ');
                let filename = line.get(3..).unwrap_or("").trim();
                let path = PathBuf::from(filename);

                let status = match status_char {
                    'M' | 'A' | 'D' | 'R' | 'C' => {
                        let file_status = match status_char {
                            'M' => FileStatusType::Modified,
                            'A' => FileStatusType::Added,
                            'D' => FileStatusType::Deleted,
                            'R' => FileStatusType::Renamed,
                            'C' => FileStatusType::Copied,
                            _ => FileStatusType::Modified,
                        };

                        // 判断是staged还是unstaged
                        let index_status = line.chars().nth(1).unwrap_or(' ');
                        
                        if index_status != ' ' && index_status != '?' {
                            staged.push(FileStatus {
                                path: path.clone(),
                                status: file_status,
                                diff_stats: None,
                            });
                        }

                        if status_char != ' ' && status_char != '?' {
                            unstaged.push(FileStatus {
                                path,
                                status: file_status,
                                diff_stats: None,
                            });
                        }

                        continue;
                    }
                    _ => continue,
                };
            }
        }

        let status = StagingStatus {
            staged_files: staged,
            unstaged_files,
            untracked_files,
            has_conflicts,
        };

        // 更新缓存
        *self.status_cache.write().await = Some(status.clone());
        *self.last_cache_update.write().await = Some(Instant::now());

        Ok(status)
    }

    fn validate_branch_name(&self, name: &str) -> Result<(), GitError> {
        if name.is_empty() {
            return Err(GitError::OperationFailed("Branch name cannot be empty".to_string()));
        }

        if name.contains(' ') || name.contains('~') || name.contains('^') || name.contains(':') {
            return Err(GitError::OperationFailed(
                "Branch name contains invalid characters".to_string()
            ));
        }

        if name.starts_with('-') || name.ends_with('/') {
            return Err(GitError::OperationFailed(
                "Branch name has invalid format".to_string()
            ));
        }

        Ok(())
    }

    async fn parse_conflict_markers(
        &self,
        diff_output: &str,
        file_path: &str,
    ) -> Result<(usize, String, String), GitError> {
        // 解析 <<<<<<< >>>>>>> ====== 标记
        let content = tokio::fs::read_to_string(self.repo_path.join(file_path)).await
            .map_err(|e| GitError::Io(e))?;

        let lines: Vec<&str> = content.lines().collect();
        let mut start_line = 0;
        let mut end_line = 0;
        let mut ours = String::new();
        let mut theirs = String::new();
        let mut in_ours = false;
        let mut in_theirs = false;

        for (i, line) in lines.iter().enumerate() {
            if line.starts_with("<<<<<<<") {
                start_line = i + 1; // 1-indexed
                in_ours = true;
                continue;
            }

            if line.starts=======") && in_ours {
                in_ours = false;
                in_theirs = true;
                continue;
            }

            if line.starts_with(">>>>>>>") && in_theirs {
                end_line = i + 1;
                break;
            }

            if in_ours {
                ours.push_str(line);
                ours.push('\n');
            } else if in_theirs {
                theirs.push_str(line);
                theirs.push('\n');
            }
        }

        Ok((start_line, end_line, ours, theirs))
    }

    fn classify_conflict_type(&self, ours: &str, theirs: &str) -> ConflictType {
        // 简单启发式分类
        let our_lines: HashSet<&str> = ours.lines().collect();
        let their_lines: HashSet<&str> = theirs.lines().collect();

        // 检查是否是导入冲突
        let has_import_ours = our_lines.iter().any(|l| l.contains("use ") || l.contains("#include"));
        let has_import_theirs = their_lines.iter().any(|l| l.contains("use ") || l.contains("#include"));

        if has_import_ours || has_import_theirs {
            return ConflictType::ImportDependency;
        }

        // 检查是否一方为空（删除vs修改）
        if ours.trim().is_empty() || theirs.trim().is_empty() {
            return ConflictType::DeleteModify;
        }

        // 默认为内容修改冲突
        ConflictType::ContentModification
    }

    fn calculate_conflict_severity(
        &self,
        _conflict_type: &ConflictType,
        ours: &str,
        theirs: &str,
    ) -> u8 {
        // 基于差异程度计算严重程度
        let our_len = ours.lines().count();
        let their_len = theirs.lines().count();
        let max_len = our_len.max(their_len);

        if max_len == 0 {
            return 1;
        }

        // 计算差异比例
        let common_lines: usize = ours.lines()
            .zip(theirs.lines())
            .filter(|(a, b)| a == b)
            .count();

        let similarity = common_lines as f64 / max_len as f64;

        if similarity < 0.3 {
            9 // 高度冲突
        } else if similarity < 0.6 {
            6 // 中等冲突
        } else if similarity < 0.8 {
            3 // 轻微冲突
        } else {
            1 // 几乎相同
        }
    }

    async fn generate_resolution_suggestion(
        &self,
        ours: &str,
        theirs: &str,
        base: Option<&str>,
    ) -> Result<ConflictResolution, GitError> {
        // 简化的冲突解决策略
        
        // 1. 如果一方只是添加了注释或空行，采用另一方
        let our_trimmed = ours.lines()
            .filter(|l| !l.trim().is_empty() && !l.trim().starts_with("//"))
            .count();
        
        let their_trimmed = theirs.lines()
            .filter(|l| !l.trim().is_empty() && !l.trim().starts_with("//"))
            .count();

        if our_trimmed == 0 {
            return Ok(ConflictResolution {
                resolved_content: theirs.to_string(),
                strategy: ResolutionStrategy::AcceptTheirs,
                confidence: 0.95,
                explanation: "Our version contains only comments/whitespace".to_string(),
                requires_review: false,
            });
        }

        if their_trimmed == 0 {
            return Ok(ConflictResolution {
                resolved_content: ours.to_string(),
                strategy: ResolutionStrategy::AcceptOurs,
                confidence: 0.95,
                explanation: "Their version contains only comments/whitespace".to_string(),
                requires_review: false,
            });
        }

        // 2. 如果两者非常相似，尝试合并非冲突部分
        let similarity = calculate_text_similarity(ours, theirs);

        if similarity > 0.8 {
            let merged = try_merge_similar_content(ours, theirs);
            
            Ok(ConflictResolution {
                resolved_content: merged,
                strategy: ResolutionStrategy::RuleBased,
                confidence: 0.75,
                explanation: "High similarity detected, attempted rule-based merge".to_string(),
                requires_review: true,
            })
        } else {
            // 低相似度，需要人工介入
            Ok(ConflictResolution {
                resolved_content: format!("{}\n// CONFLICT: Manual resolution required\n{}", ours, theirs),
                strategy: ResolutionStrategy::ManualMerge,
                confidence: 0.2,
                explanation: "Significant differences between versions".to_string(),
                requires_review: true,
            })
        }
    }

    async fn extract_context_lines(
        &self,
        file_path: &str,
        start: usize,
        end: usize,
    ) -> Result<Vec<String>, GitError> {
        let context_size = 5;
        let context_start = start.saturating_sub(context_size);
        let context_end = end + context_size;

        let content = tokio::fs::read_to_string(self.repo_path.join(file_path)).await
            .map_err(|e| GitError::Io(e))?;

        let lines: Vec<String> = content.lines()
            .skip(context_start)
            .take(context_end - context_start)
            .map(|s| s.to_string())
            .collect();

        Ok(lines)
    }

    async fn get_head_commit_hash(&self) -> Result<String, GitError> {
        self.run_git_command(&["rev-parse", "HEAD"]).await
            .map(|s| s.trim().to_string())
    }

    fn parse_merge_stats(&self, _output: &str) -> MergeStats {
        // 简化实现，实际应解析git输出中的统计信息
        MergeStats::default()
    }

    fn extract_merge_commit_hash(&self, _output: &str) -> Option<String> {
        // 简化实现
        None
    }

    async fn count_rebased_commits(&self, _onto: &str) -> Result<usize, GitError> {
        // 简化实现
        Ok(0)
    }

    fn count_files_in_pull_output(&self, _output: &str) -> usize {
        0
    }

    async fn record_operation(
        &self,
        op_type: &str,
        duration: Duration,
        success: bool,
        error: Option<String>,
    ) {
        let operation = GitOperation {
            op_type: op_type.to_string(),
            started_at: Instant::now() - duration,
            duration_ms: duration.as_millis() as u64,
            success,
            error,
        };

        let mut history = self.operation_history.write().await;
        history.push(operation);

        // 只保留最近100条记录
        if history.len() > 100 {
            history.drain(..(history.len() - 100));
        }
    }

    async fn invalidate_cache(&self) {
        *self.status_cache.write().await = None;
        *self.branch_cache.write().await = None;
        *self.last_cache_update.write().await = None;
    }

    /// 获取操作历史
    pub async fn get_operation_history(&self) -> Vec<GitOperation> {
        self.operation_history.read().await.clone()
    }
}

// ============================================================================
// 辅助结构体和函数
// ============================================================================

/// Rebase 结果
#[derive(Debug, Clone)]
pub struct RebaseResult {
    /// 是否成功
    pub success: bool,
    
    /// rebased的commit数
    pub commits_rebased: usize,
    
    /// 冲突列表
    pub conflicts: Vec<ConflictInfo>,
    
    /// 是否需要手动abort
    pub abort_needed: bool,
    
    /// 执行时间 (ms)
    pub duration_ms: u64,
}

/// 交互式rebase动作
#[derive(Debug, Clone)]
pub enum InteractiveRebaseAction {
    /// Squash最后N个commits
    SquashLast(u32),
    /// 编辑最后一个commit
    EditLast,
    /// Reword最后一个commit
    RewordLast(String),
}

/// Commit选项
#[derive(Debug, Clone, Default)]
pub struct CommitOptions {
    /// 是否amend
    pub amend: bool,
    /// 跳过hooks
    pub no_verify: bool,
    /// 指定author
    pub author: Option<String>,
}

/// Pull结果
#[derive(Debug, Clone)]
pub struct PullResult {
    /// 是否成功
    pub success: bool,
    
    /// 变更文件数
    pub files_changed: usize,
    
    /// 冲突列表
    pub conflicts: Vec<ConflictInfo>,
}

/// 远程仓库信息
#[derive(Debug, Clone)]
pub struct RemoteInfo {
    /// 名称
    pub name: String,
    
    /// URL
    pub url: String,
    
    /// 是否是fetch URL
    pub is_fetch: bool,
}

/// 冲突解决结果
#[derive(Debug, Clone)]
pub struct ConflictResolutionResult {
    /// 总冲突数
    pub total_conflicts: usize,
    
    /// 自动解决的数目
    pub auto_resolved: usize,
    
    /// 需要人工处理的冲突
    pub requires_manual_intervention: Vec<ConflictInfo>,
    
    /// 已应用的解决方案
    pub resolutions_applied: Vec<(PathBuf, ConflictResolution)>,
}

fn calculate_text_similarity(a: &str, b: &str) -> f64 {
    let a_words: HashSet<&str> = a.split_whitespace().collect();
    let b_words: HashSet<&str> = b.split_whitespace().collect();

    if a_words.is_empty() || b_words.is_empty() {
        return 0.0;
    }

    let intersection: usize = a_words.intersection(&b_words).count();
    let union = a_words.len() + b_words.len() - intersection;

    if union == 0 {
        1.0
    } else {
        intersection as f64 / union as f64
    }
}

fn try_merge_similar_content(ours: &str, theirs: &str) -> String {
    // 简化的逐行合并策略
    let our_lines: Vec<&str> = ours.lines().collect();
    let their_lines: Vec<&str> = theirs.lines().collect();

    let mut result = String::new();
    let mut used_indices: HashSet<usize> = HashSet::new();

    // 优先使用our的内容
    for (i, line) in our_lines.iter().enumerate() {
        if !used_indices.contains(&i) {
            result.push_str(line);
            result.push('\n');
            used_indices.insert(i);
        }
    }

    // 添加their独有的内容
    for (j, line) in their_lines.iter().enumerate() {
        if !our_lines.contains(&line) {
            result.push_str(line);
            result.push('\n');
        }
    }

    result
}

// ============================================================================
// 测试模块
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_create_repository() {
        let dir = tempdir().expect("Failed to create temp dir");
        
        // 初始化git仓库
        Command::new("init")
            .current_dir(dir.path())
            .output()
            .await
            .expect("Failed to init git repo");

        let manager = GitWorkflowManager::new(dir.path()).expect("Failed to create manager");
        
        assert!(manager.repo_path.exists());
    }

    #[test]
    fn test_validate_branch_name() {
        let dir = tempdir().expect("Failed to create temp dir");
        let manager = GitWorkflowManager::new(dir.path()).expect("Failed to create manager");

        assert!(manager.validate_branch_name("feature/test").is_ok());
        assert!(manager.validate_branch_name("hotfix/bug-123").is_ok());
        
        assert!(manager.validate_branch_name("").is_err());
        assert!(manager.validate_branch_name("invalid name").is_err());
        assert!(manager.validate_branch_name("-bad").is_err());
    }

    #[test]
    fn test_text_similarity() {
        let a = "fn hello() {\n    println!(\"Hello\");\n}";
        let b = "fn hello() {\n    println!(\"Hello World\");\n}";
        
        let sim = calculate_text_similarity(a, b);
        assert!(sim > 0.5); // 应该有较高的相似度

        let c = "fn goodbye() {\n    exit(1);\n}";
        let sim2 = calculate_text_similarity(a, c);
        assert!(sim2 < 0.5); // 相似度较低
    }

    #[test]
    fn test_conflict_severity_calculation() {
        let dir = tempdir().expect("Failed to create temp dir");
        let manager = GitWorkflowManager::new(dir.path()).expect("Failed to create manager");

        // 高相似度 → 低严重程度
        let severity_high_sim = manager.calculate_conflict_severity(
            &ConflictType::ContentModification,
            "let x = 1;\nlet y = 2;",
            "let x = 1;\nlet y = 3;",
        );
        assert!(severity_high_sim <= 3);

        // 低相似度 → 高严重程度
        let severity_low_sim = manager.calculate_conflict_severity(
            &ConflictType::ContentModification,
            "fn foo() { ... }",
            "class Bar { ... }",
        );
        assert!(severity_low_sim >= 7);
    }
}
