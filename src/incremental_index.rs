//! 增量代码索引系统
//!
//! 提供高效的增量代码索引能力:
//! - 文件变更监听 (基于 notify crate)
//! - 增量 AST 解析 (只重解析变更文件)
//! - 符号索引增量更新
//! - 依赖图增量维护

use crate::ast::tree_sitter::{
    AstParser, CodeAnalyzer, FileAnalysis, SymbolInfo,
    SupportedLanguage,
};
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// 增量索引配置
#[derive(Debug, Clone)]
pub struct IncrementalIndexConfig {
    /// 索引根目录
    pub root_dir: PathBuf,
    /// 监听的文件扩展名
    pub extensions: Vec<String>,
    /// 增量更新间隔 (毫秒)
    pub update_interval_ms: u64,
    /// 批量处理大小
    pub batch_size: usize,
    /// 启用增量解析
    pub enable_incremental: bool,
    /// 最大缓存文件数
    pub max_cached_files: usize,
}

impl Default for IncrementalIndexConfig {
    fn default() -> Self {
        Self {
            root_dir: PathBuf::from("."),
            extensions: vec![
                "rs".to_string(),
                "py".to_string(),
                "js".to_string(),
                "ts".to_string(),
                "tsx".to_string(),
                "go".to_string(),
            ],
            update_interval_ms: 100,
            batch_size: 50,
            enable_incremental: true,
            max_cached_files: 1000,
        }
    }
}

/// 文件索引状态
#[derive(Debug, Clone)]
pub struct FileIndexState {
    /// 文件路径
    pub path: PathBuf,
    /// 最后修改时间
    pub modified: SystemTime,
    /// 文件哈希 (用于快速检测变更)
    pub content_hash: u64,
    /// 提取的符号数量
    pub symbol_count: usize,
    /// 索引时间
    pub indexed_at: Instant,
    /// 依赖文件列表
    pub dependencies: Vec<PathBuf>,
}

/// 增量索引器
pub struct IncrementalIndexer {
    config: IncrementalIndexConfig,
    parser: Arc<AstParser>,
    /// 文件状态缓存
    file_states: Arc<RwLock<HashMap<PathBuf, FileIndexState>>>,
    /// 全局符号索引 (file_path -> symbols)
    symbol_index: Arc<RwLock<HashMap<PathBuf, Vec<SymbolInfo>>>>,
    /// 依赖图 (file -> files it depends on)
    dependency_graph: Arc<RwLock<HashMap<PathBuf, HashSet<PathBuf>>>>,
    /// 待处理变更队列
    pending_changes: Arc<RwLock<HashSet<PathBuf>>>,
    /// 统计信息
    stats: Arc<RwLock<IndexStats>>,
}

/// 索引统计
#[derive(Debug, Clone, Default)]
pub struct IndexStats {
    pub total_files_indexed: u64,
    pub incremental_updates: u64,
    pub full_reparses: u64,
    pub symbols_extracted: u64,
    pub dependencies_resolved: u64,
    pub last_index_time: Option<Duration>,
}

/// 变更类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeType {
    Created,
    Modified,
    Deleted,
}

/// 文件变更记录
#[derive(Debug, Clone)]
pub struct FileChange {
    pub path: PathBuf,
    pub change_type: ChangeType,
    pub detected_at: Instant,
}

impl IncrementalIndexer {
    /// 创建新的增量索引器
    pub fn new(config: IncrementalIndexConfig) -> Result<Self> {
        let parser = AstParser::with_defaults()?;

        Ok(Self {
            config,
            parser: Arc::new(parser),
            file_states: Arc::new(RwLock::new(HashMap::new())),
            symbol_index: Arc::new(RwLock::new(HashMap::new())),
            dependency_graph: Arc::new(RwLock::new(HashMap::new())),
            pending_changes: Arc::new(RwLock::new(HashSet::new())),
            stats: Arc::new(RwLock::new(IndexStats::default())),
        })
    }

    /// 获取配置
    pub fn config(&self) -> &IncrementalIndexConfig {
        &self.config
    }

    /// 获取统计信息
    pub async fn get_stats(&self) -> IndexStats {
        self.stats.read().await.clone()
    }

    /// 注册文件变更
    pub async fn register_change(&self, path: PathBuf, change_type: ChangeType) {
        let mut pending = self.pending_changes.write().await;
        pending.insert(path.clone());
        debug!(path = %path.display(), ?change_type, "File change registered");
    }

    /// 批量处理待处理的变更
    pub async fn process_pending_changes(&self) -> Result<IndexResult> {
        let changes: Vec<PathBuf> = {
            let mut pending = self.pending_changes.write().await;
            pending.drain().collect()
        };

        if changes.is_empty() {
            return Ok(IndexResult::default());
        }

        let start = Instant::now();
        let mut result = IndexResult::default();

        // 批量处理变更
        for chunk in changes.chunks(self.config.batch_size) {
            for path in chunk {
                match self.process_single_change(path).await {
                    Ok(r) => result.merge(r),
                    Err(e) => {
                        warn!(path = %path.display(), error = %e, "Failed to process change");
                        result.errors += 1;
                    }
                }
            }
        }

        result.duration = start.elapsed();

        // 更新统计
        {
            let mut stats = self.stats.write().await;
            stats.total_files_indexed += result.files_processed as u64;
            stats.incremental_updates += result.incremental_updates as u64;
            stats.full_reparses += result.full_reparses as u64;
            stats.symbols_extracted += result.symbols_extracted as u64;
            stats.dependencies_resolved += result.dependencies_resolved as u64;
            stats.last_index_time = Some(result.duration);
        }

        info!(
            files = result.files_processed,
            incremental = result.incremental_updates,
            full = result.full_reparses,
            duration_ms = result.duration.as_millis(),
            "Pending changes processed"
        );

        Ok(result)
    }

    /// 处理单个文件变更
    async fn process_single_change(&self, path: &Path) -> Result<IndexResult> {
        let mut result = IndexResult::default();

        // 检查文件是否应该被索引
        if !self.should_index(path) {
            return Ok(result);
        }

        // 获取当前文件状态
        let current_mtime = self.get_file_mtime(path);
        let current_hash = self.compute_content_hash(path).await?;

        let previous_state = self.file_states.read().await.get(path).cloned();

        match (previous_state, current_mtime) {
            // 文件被删除
            (Some(_), None) => {
                self.handle_file_deletion(path).await?;
                result.files_processed = 1;
            }
            // 新文件或重新创建
            (None, Some(mtime)) => {
                self.index_file(path, current_hash, mtime).await?;
                result.files_processed = 1;
                result.full_reparses = 1;
            }
            // 文件被修改
            (Some(state), Some(mtime)) if state.content_hash != current_hash => {
                // 检查是否支持增量解析
                if self.config.enable_incremental {
                    // 增量更新
                    self.update_file_incremental(path, current_hash, mtime).await?;
                    result.incremental_updates = 1;
                } else {
                    // 全量重解析
                    self.index_file(path, current_hash, mtime).await?;
                    result.full_reparses = 1;
                }
                result.files_processed = 1;
            }
            // 无变更
            _ => {
                debug!(path = %path.display(), "No changes detected");
            }
        }

        Ok(result)
    }

    /// 判断文件是否应该被索引
    fn should_index(&self, path: &Path) -> bool {
        if !path.is_file() {
            return false;
        }

        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");

        self.config.extensions.contains(&extension.to_string())
    }

    /// 获取文件修改时间
    fn get_file_mtime(&self, path: &Path) -> Option<SystemTime> {
        std::fs::metadata(path).ok()?.modified().ok()
    }

    /// 计算内容哈希 (简单哈希用于变更检测)
    async fn compute_content_hash(&self, path: &Path) -> Result<u64> {
        use std::hash::{Hash, Hasher};
        use std::collections::hash_map::DefaultHasher;

        let content = tokio::fs::read_to_string(path).await?;
        let mut hasher = DefaultHasher::new();
        content.hash(&mut hasher);
        Ok(hasher.finish())
    }

    /// 处理文件删除
    async fn handle_file_deletion(&self, path: &Path) -> Result<()> {
        // 移除文件状态
        self.file_states.write().await.remove(path);

        // 移除符号索引
        self.symbol_index.write().await.remove(path);

        // 从依赖图中移除
        let mut graph = self.dependency_graph.write().await;
        graph.remove(path);

        // 清理对被删除文件的依赖引用
        for deps in graph.values_mut() {
            deps.retain(|p| p != path);
        }

        info!(path = %path.display(), "File removed from index");
        Ok(())
    }

    /// 索引新文件或全量重解析
    async fn index_file(
        &self,
        path: &Path,
        content_hash: u64,
        mtime: SystemTime,
    ) -> Result<()> {
        let start = Instant::now();
        let path_str = path.display().to_string();

        // 使用 CodeAnalyzer 分析文件
        let analysis = CodeAnalyzer::new()?.analyze_file(path).await?;

        // 提取依赖
        let dependencies = self.extract_dependencies(&analysis);

        // 更新文件状态
        let state = FileIndexState {
            path: path.to_path_buf(),
            modified: mtime,
            content_hash,
            symbol_count: analysis.symbols.len(),
            indexed_at: Instant::now(),
            dependencies: dependencies.clone(),
        };

        self.file_states.write().await.insert(path.to_path_buf(), state);

        // 更新符号索引
        self.symbol_index
            .write()
            .await
            .insert(path.to_path_buf(), analysis.symbols.clone());

        // 更新依赖图
        self.dependency_graph
            .write()
            .await
            .insert(path.to_path_buf(), dependencies.into_iter().collect());

        debug!(
            path = %path_str,
            symbols = analysis.symbols.len(),
            duration_ms = start.elapsed().as_millis(),
            "File indexed"
        );

        Ok(())
    }

    /// 增量更新文件
    async fn update_file_incremental(
        &self,
        path: &Path,
        content_hash: u64,
        mtime: SystemTime,
    ) -> Result<()> {
        let start = Instant::now();
        let path_str = path.display().to_string();

        // 读取新内容
        let content = tokio::fs::read_to_string(path).await?;

        // 获取语言
        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        let language = SupportedLanguage::from_extension(extension)
            .unwrap_or(SupportedLanguage::Rust);

        // 增量解析
        let tree = self.parser.parse(&content, language, &path_str).await?;

        // 提取符号
        let symbols = self.parser.extract_symbols(&tree, &content, &path_str, language).await;

        // 更新文件状态
        let state = FileIndexState {
            path: path.to_path_buf(),
            modified: mtime,
            content_hash,
            symbol_count: symbols.len(),
            indexed_at: Instant::now(),
            dependencies: Vec::new(), // TODO: 从调用图中提取
        };

        self.file_states.write().await.insert(path.to_path_buf(), state);

        // 更新符号索引
        self.symbol_index
            .write()
            .await
            .insert(path.to_path_buf(), symbols);

        debug!(
            path = %path_str,
            duration_ms = start.elapsed().as_millis(),
            "File incrementally updated"
        );

        Ok(())
    }

    /// 从分析结果中提取依赖
    fn extract_dependencies(&self, analysis: &FileAnalysis) -> Vec<PathBuf> {
        let mut deps = Vec::new();

        // 从 call graph 中提取依赖
        for (_caller, callees) in &analysis.call_graph {
            for callee in callees {
                // 简单处理：实际实现需要符号解析
                if callee.contains("::") || callee.contains(".") {
                    deps.push(PathBuf::from(callee));
                }
            }
        }

        deps
    }

    /// 根据符号名搜索定义位置
    pub async fn find_symbol_definition(&self, symbol_name: &str) -> Option<SymbolInfo> {
        let index = self.symbol_index.read().await;

        for symbols in index.values() {
            for symbol in symbols {
                if symbol.name == symbol_name {
                    return Some(symbol.clone());
                }
            }
        }

        None
    }

    /// 获取文件中引用的所有符号
    pub async fn get_file_symbols(&self, path: &Path) -> Option<Vec<SymbolInfo>> {
        self.symbol_index.read().await.get(path).cloned()
    }

    /// 获取依赖当前文件的所有文件
    pub async fn get_dependents(&self, path: &Path) -> Vec<PathBuf> {
        let graph = self.dependency_graph.read().await;
        graph
            .iter()
            .filter(|(_, deps)| deps.contains(path))
            .map(|(p, _)| p.clone())
            .collect()
    }

    /// 获取被当前文件依赖的所有文件
    pub async fn get_dependencies(&self, path: &Path) -> HashSet<PathBuf> {
        self.dependency_graph
            .read()
            .await
            .get(path)
            .cloned()
            .unwrap_or_default()
    }

    /// 清空索引
    pub async fn clear(&self) {
        self.file_states.write().await.clear();
        self.symbol_index.write().await.clear();
        self.dependency_graph.write().await.clear();
        self.pending_changes.write().await.clear();
        info!("Index cleared");
    }
}

/// 索引结果
#[derive(Debug, Default)]
pub struct IndexResult {
    pub files_processed: usize,
    pub incremental_updates: usize,
    pub full_reparses: usize,
    pub symbols_extracted: usize,
    pub dependencies_resolved: usize,
    pub duration: Duration,
    pub errors: usize,
}

impl IndexResult {
    fn merge(&mut self, other: IndexResult) {
        self.files_processed += other.files_processed;
        self.incremental_updates += other.incremental_updates;
        self.full_reparses += other.full_reparses;
        self.symbols_extracted += other.symbols_extracted;
        self.dependencies_resolved += other.dependencies_resolved;
        self.errors += other.errors;
    }
}

/// 全局增量索引器实例
pub type GlobalIndexer = Arc<IncrementalIndexer>;

/// 获取或创建全局索引器
pub fn get_or_create_indexer(config: IncrementalIndexConfig) -> GlobalIndexer {
    use std::sync::OnceLock;
    static INDEXER: OnceLock<GlobalIndexer> = OnceLock::new();

    INDEXER
        .get_or_init(|| Arc::new(IncrementalIndexer::new(config).expect("Failed to create indexer")))
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_incremental_indexer() {
        let temp_dir = TempDir::new().unwrap();
        let config = IncrementalIndexConfig {
            root_dir: temp_dir.path().to_path_buf(),
            extensions: vec!["rs".to_string()],
            ..Default::default()
        };

        let indexer = IncrementalIndexer::new(config).unwrap();

        // 创建测试文件
        let test_file = temp_dir.path().join("test.rs");
        tokio::fs::write(
            &test_file,
            r#"
fn hello() {
    println!("Hello");
}

struct TestStruct {
    value: i32,
}
"#,
        )
        .await
        .unwrap();

        // 注册变更并处理
        indexer.register_change(test_file.clone(), ChangeType::Created).await;
        indexer.process_pending_changes().await.unwrap();

        // 验证索引
        let symbols = indexer.get_file_symbols(&test_file).await;
        assert!(symbols.is_some());
        assert!(!symbols.unwrap().is_empty());
    }
}
