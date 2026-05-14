//! Semantic Index - 语义级向量索引系统
//!
//! 提供基于语义相似度的代码搜索能力：
//! - 向量存储与检索
//! - 语义相似度计算
//! - 跨文件功能相似性分析
//!
//! 特性：
//! - 支持 FAISS/Qdrant/Milvus 等向量数据库
//! - 余弦相似度 + 混合排序
//! - 元数据过滤

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};
use anyhow::Result;

/// 向量数据库 trait (抽象后端)
pub trait VectorDatabase: Send + Sync {
    /// 插入向量
    fn insert(&mut self, id: &str, vector: Vec<f32>, metadata: HashMap<String, String>) -> Result<()>;

    /// 搜索最相似的向量
    fn search(&self, query: &[f32], top_k: usize) -> Result<Vec<VectorSearchResult>>;

    /// 删除向量
    fn delete(&mut self, id: &str) -> Result<bool>;

    /// 获取向量数量
    fn len(&self) -> usize;
}

/// 向量搜索结果
#[derive(Debug, Clone)]
pub struct VectorSearchResult {
    /// ID
    pub id: String,
    /// 相似度分数 (0-1)
    pub score: f64,
    /// 元数据
    pub metadata: VectorMetadata,
}

/// 向量元数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorMetadata {
    /// 文件路径
    pub file_path: PathBuf,
    /// 符号名称
    pub symbol_name: Option<String>,
    /// 符号类型
    pub kind: Option<String>,
    /// 原始文本片段 (前 200 字符)
    pub snippet: Option<String>,
    /// 语言
    pub language: Option<String>,
    /// 行号范围
    pub start_line: Option<usize>,
    pub end_line: Option<usize>,
}

/// 内存实现 (用于开发/测试)
pub struct InMemoryVectorDB {
    vectors: HashMap<String, (Vec<f32>, VectorMetadata)>,
}

impl InMemoryVectorDB {
    pub fn new() -> Self {
        Self {
            vectors: HashMap::new(),
        }
    }
}

impl VectorDatabase for InMemoryVectorDB {
    fn insert(
        &mut self,
        id: &str,
        vector: Vec<f32>,
        metadata: HashMap<String, String>,
    ) -> Result<()> {
        let vm = VectorMetadata {
            file_path: metadata.get("file_path")
                .map(PathBuf::from)
                .unwrap_or_default(),
            symbol_name: metadata.get("symbol_name").cloned(),
            kind: metadata.get("kind").cloned(),
            snippet: metadata.get("snippet").cloned(),
            language: metadata.get("language").cloned(),
            start_line: metadata.get("start_line")
                .and_then(|s| s.parse().ok()),
            end_line: metadata.get("end_line")
                .and_then(|s| s.parse().ok()),
        };
        
        self.vectors.insert(id.to_string(), (vector, vm));
        Ok(())
    }

    fn search(&self, query: &[f32], top_k: usize) -> Result<Vec<VectorSearchResult>> {
        let mut results: Vec<(String, f64)> = self.vectors
            .iter()
            .map(|(id, (vec, _meta))| {
                let score = cosine_similarity(query, vec);
                (id.clone(), score)
            })
            .collect();
        
        // 按分数降序排序
        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        
        Ok(results
            .into_iter()
            .take(top_k)
            .map(|(id, score)| {
                let (_, meta) = &self.vectors[&id];
                VectorSearchResult {
                    id,
                    score,
                    metadata: meta.clone(),
                }
            })
            .collect())
    }

    fn delete(&mut self, id: &str) -> Result<bool> {
        Ok(self.vectors.remove(id).is_some())
    }

    fn len(&self) -> usize {
        self.vectors.len()
    }
}

/// 语义搜索结果
#[derive(Debug, Clone)]
pub struct SemanticSearchResult {
    /// 匹配的代码片段
    pub file_path: PathBuf,
    /// 函数名 (如果是函数)
    pub function_name: Option<String>,
    /// 相似度分数 (0-1)
    pub similarity: f64,
    /// 代码片段
    pub snippet: Option<String>,
    /// 行号范围
    pub start_line: Option<usize>,
    pub end_line: Option<usize>,
}

/// 类似函数搜索结果
#[derive(Debug, Clone)]
pub struct SimilarFunctionResult {
    pub file_path: PathBuf,
    pub function_name: String,
    pub similarity: f64,
    pub snippet: String,
}

/// 语义索引配置
#[derive(Debug, Clone)]
pub struct SemanticIndexConfig {
    /// 向量维度
    pub dimensions: usize,
    
    /// 默认返回数量
    pub default_top_k: usize,
    
    /// 最小相似度阈值
    pub min_similarity_threshold: f64,
}

impl Default for SemanticIndexConfig {
    fn default() -> Self {
        Self {
            dimensions: 1024,
            default_top_k: 10,
            min_similarity_threshold: 0.7,
        }
    }
}

/// 语义索引系统
pub struct SemanticIndex {
    /// 向量数据库
    db: Arc<RwLock<Box<dyn VectorDatabase>>>,
    
    /// 配置
    config: SemanticIndexConfig,
    
    /// 统计信息
    stats: Arc<RwLock<SemanticStats>>,
}

/// 统计信息
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SemanticStats {
    /// 总向量数
    pub total_vectors: usize,
    /// 总查询次数
    pub total_queries: u64,
    /// 平均查询延迟 (ms)
    pub avg_query_latency_ms: f64,
}

impl SemanticIndex {
    /// 创建新的语义索引 (使用内存数据库)
    pub fn with_in_memory_db(config: SemanticIndexConfig) -> Self {
        Self {
            db: Arc::new(RwLock::new(Box::new(InMemoryVectorDB::new()))),
            config,
            stats: Arc::new(RwLock::new(SemanticStats::default())),
        }
    }

    /// 使用默认配置创建
    pub fn with_defaults() -> Self {
        Self::with_in_memory_db(SemanticIndexConfig::default())
    }

    /// 索引代码块
    pub async fn index_code_chunk(
        &self,
        id: &str,
        embedding: &[f32],
        metadata: HashMap<String, String>,
    ) -> Result<()> {
        let mut db = self.db.write().await;
        db.insert(id, embedding.to_vec(), metadata)?;
        
        debug!(id = %id, "Code chunk indexed");
        Ok(())
    }

    /// 批量索引
    pub async fn index_batch(
        &self,
        items: Vec<(String, Vec<f32>, HashMap<String, String>)>,
    ) -> Result<()> {
        let count = items.len();
        let mut db = self.db.write().await;

        for (id, embedding, metadata) in items {
            db.insert(&id, embedding, metadata)?;
        }

        info!(count = count, "Batch indexed");
        Ok(())
    }

    /// 语义搜索
    pub async fn semantic_search(
        &self,
        query_embedding: &[f32],
        top_k: Option<usize>,
    ) -> Result<Vec<SemanticSearchResult>> {
        let start = std::time::Instant::now();
        
        let k = top_k.unwrap_or(self.config.default_top_k);
        let mut db = self.db.write().await;
        let results = db.search(query_embedding, k)?;
        
        let duration_ms = start.elapsed().as_millis() as f64;
        
        // 更新统计
        {
            let mut stats = self.stats.write().await;
            stats.total_queries += 1;
            stats.avg_query_latency_ms =
                (stats.avg_query_latency_ms * (stats.total_queries - 1) as f64 + duration_ms)
                / stats.total_queries as f64;
        }
        
        // 过滤低相关性结果并转换格式
        let final_results: Vec<SemanticSearchResult> = results
            .into_iter()
            .filter(|r| r.score >= self.config.min_similarity_threshold)
            .map(|r| SemanticSearchResult {
                file_path: r.metadata.file_path.clone(),
                function_name: r.metadata.symbol_name.clone(),
                similarity: r.score,
                snippet: r.metadata.snippet.clone(),
                start_line: r.metadata.start_line,
                end_line: r.metadata.end_line,
            })
            .collect();
        
        Ok(final_results)
    }

    /// 查找语义相似的函数
    pub async fn find_similar_functions(
        &self,
        function_embedding: &[f32],
        exclude_files: &[PathBuf],
        limit: usize,
    ) -> Result<Vec<SimilarFunctionResult>> {
        let mut db = self.db.write().await;
        let results = db.search(function_embedding, limit * 2)?;

        let filtered_results: Vec<VectorSearchResult> = results
            .into_iter()
            .filter(|r| !exclude_files.contains(&r.metadata.file_path))
            .collect();
        
        // 仅保留函数类型的匹配
        let functions: Vec<SimilarFunctionResult> = filtered_results
            .into_iter()
            .filter(|r| r.metadata.kind.as_deref() == Some("function"))
            .map(|r| SimilarFunctionResult {
                file_path: r.metadata.file_path.clone(),
                function_name: r.metadata.symbol_name.unwrap_or_else(|| "anonymous".to_string()),
                similarity: r.score,
                snippet: r.metadata.snippet.unwrap_or_default(),
            })
            .take(limit)
            .collect();
        
        Ok(functions)
    }

    /// 删除索引项
    pub async fn delete(&self, id: &str) -> Result<bool> {
        let mut db = self.db.write().await;
        db.delete(id)
    }

    /// 获取索引大小
    pub async fn len(&self) -> usize {
        let db = self.db.read().await;
        db.len()
    }

    /// 获取统计信息
    pub async fn get_stats(&self) -> SemanticStats {
        let stats = self.stats.read().await.clone();
        let db_len = self.len().await;
        
        SemanticStats {
            total_vectors: db_len,
            ..stats
        }
    }
}

/// 计算余弦相似度
fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    
    let dot_product: f64 = a.iter()
        .zip(b.iter())
        .map(|(x, y)| *x as f64 * *y as f64)
        .sum();
    
    let magnitude_a: f64 = a.iter()
        .map(|x| (*x as f64).powi(2))
        .sum::<f64>()
        .sqrt();
    
    let magnitude_b: f64 = b.iter()
        .map(|x| (*x as f64).powi(2))
        .sum::<f64>()
        .sqrt();
    
    if magnitude_a == 0.0 || magnitude_b == 0.0 {
        return 0.0;
    }
    
    dot_product / (magnitude_a * magnitude_b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_index_and_search() {
        let idx = SemanticIndex::with_defaults();
        
        // 创建测试向量
        let vec1 = vec![1.0, 0.0, 0.0];
        let vec2 = vec![0.9, 0.1, 0.0]; // 与 vec1 相似
        let vec3 = vec![0.0, 1.0, 0.0]; // 与 vec1 不相似
        
        let mut meta1 = HashMap::new();
        meta1.insert("file_path".to_string(), "src/a.rs".to_string());
        meta1.insert("symbol_name".to_string(), "func_a".to_string());
        meta1.insert("kind".to_string(), "function".to_string());
        
        let mut meta2 = HashMap::new();
        meta2.insert("file_path".to_string(), "src/b.rs".to_string());
        meta2.insert("symbol_name".to_string(), "func_b".to_string());
        meta2.insert("kind".to_string(), "function".to_string());
        
        idx.index_code_chunk("chunk1", &vec1, meta1).await.unwrap();
        idx.index_code_chunk("chunk2", &vec2, meta2).await.unwrap();
        idx.index_code_chunk("chunk3", &vec3, HashMap::new()).await.unwrap();
        
        assert_eq!(idx.len().await, 3);
        
        // 搜索与 vec1 相似的
        let results = idx.semantic_search(&vec1, None).await.unwrap();
        assert!(!results.is_empty());
        assert!(results[0].similarity > 0.9); // 应该找到非常相似的
    }

    #[test]
    fn test_cosine_similarity() {
        let v1 = vec![1.0, 0.0];
        let v2 = vec![1.0, 0.0];
        assert!((cosine_similarity(&v1, &v2) - 1.0).abs() < 0.001); // 完全相同
        
        let v3 = vec![0.0, 1.0];
        assert!((cosine_similarity(&v1, &v3)).abs() < 0.001); // 正交
        
        let v4 = vec![];
        assert_eq!(cosine_similarity(&v1, &v4), 0.0); // 空向量
    }

    #[tokio::test]
    async fn test_delete() {
        let idx = SemanticIndex::with_defaults();
        
        idx.index_code_chunk("test", &[1.0, 0.0], HashMap::new()).await.unwrap();
        assert_eq!(idx.len().await, 1);
        
        let deleted = idx.delete("test").await.unwrap();
        assert!(deleted);
        assert_eq!(idx.len().await, 0);
        
        let deleted_again = idx.delete("test").await.unwrap();
        assert!(!deleted_again); // 再次删除应返回 false
    }

    #[tokio::test]
    async fn test_find_similar_functions() {
        let idx = SemanticIndex::with_defaults();
        
        // 添加几个"函数"
        for i in 0..5 {
            let mut meta = HashMap::new();
            meta.insert("file_path".to_string(), format!("src/file{}.rs", i));
            meta.insert("symbol_name".to_string(), format!("func_{}", i));
            meta.insert("kind".to_string(), "function".to_string());
            
            // 创建逐渐不相似的向量
            let base_vec = vec![1.0 - (i as f32 * 0.2), i as f32 * 0.2, 0.0];
            idx.index_code_chunk(&format!("func_{}", i), &base_vec, meta).await.unwrap();
        }
        
        let query = vec![1.0, 0.0, 0.0];
        let similar = idx.find_similar_functions(&query, &[], 3).await.unwrap();
        
        assert_eq!(similar.len(), 3);
        assert_eq!(similar[0].function_name, "func_0"); // 最相似的应该是 func_0
        assert!(similar[0].similarity > similar[1].similarity); // 递减顺序
    }
}
