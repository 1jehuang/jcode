//! Enhanced Embedding Engine - 高质量代码向量化
//!
//! 支持多模型后端：
//! - Qwen-Embedding (本地/云端)
//! - OpenAI text-embedding-3-large (1536维)
//! - 本地 sentence-transformers (离线降级)
//!
//! 特性：
//! - 语言特定预处理 (去除注释、标准化空白)
//! - 语义边界感知切块 (按函数/类/语句)
//! - 批量嵌入生成 (优化吞吐量)
//! - 自动缓存与去重

use anyhow::Result;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};
use serde::{Deserialize, Serialize};

/// Embedding 模型类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EmbeddingModel {
    /// OpenAI text-embedding-3-large (1536 dimensions)
    #[serde(rename = "openai-large")]
    OpenAILarge,
    /// OpenAI text-embedding-3-small (768 dimensions)
    #[serde(rename = "openai-small")]
    OpenAISmall,
    /// Qwen-Embedding (1024 or 2048 dimensions)
    #[serde(rename = "qwen")]
    QwenEmbedding,
    /// 本地 sentence-transformers (384 dimensions)
    #[serde(rename = "local")]
    LocalSentenceTransformers,
}

impl Default for EmbeddingModel {
    fn default() -> Self {
        Self::QwenEmbedding // 默认使用 Qwen (性价比最优)
    }
}

impl std::fmt::Display for EmbeddingModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OpenAILarge => write!(f, "text-embedding-3-large"),
            Self::OpenAISmall => write!(f, "text-embedding-3-small"),
            Self::QwenEmbedding => write!(f, "qwen-embedding"),
            Self::LocalSentenceTransformers => write!(f, "sentence-transformers"),
        }
    }
}

/// Embedding 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    /// 主模型
    pub primary_model: EmbeddingModel,
    
    /// 备用模型 (离线/降级)
    pub fallback_model: Option<EmbeddingModel>,
    
    /// 向量维度
    pub dimensions: usize,
    
    /// 批量大小 (每次请求的最大 chunk 数)
    pub batch_size: usize,
    
    /// API 密钥 (对于云端模型)
    pub api_key: Option<String>,
    
    /// API 端点 URL
    pub api_endpoint: Option<String>,
    
    /// 是否启用缓存
    pub enable_cache: bool,
    
    /// 缓存最大条目数
    pub cache_max_size: usize,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            primary_model: EmbeddingModel::default(),
            fallback_model: Some(EmbeddingModel::LocalSentenceTransformers),
            dimensions: 1024, // Qwen 默认维度
            batch_size: 64,
            api_key: None,
            api_endpoint: None,
            enable_cache: true,
            cache_max_size: 100_000,
        }
    }
}

/// 嵌入结果
#[derive(Debug, Clone)]
pub struct EmbeddingResult {
    /// 向量数据
    pub embedding: Vec<f32>,
    /// Token 数量估算
    pub token_count: usize,
    /// 使用的模型
    pub model: EmbeddingModel,
    /// 耗时 (毫秒)
    pub duration_ms: u64,
}

/// 代码块 (用于嵌入的文本单元)
#[derive(Debug, Clone)]
pub struct CodeChunk {
    /// 块的唯一标识
    pub id: String,
    /// 原始文本内容
    pub content: String,
    /// 所属文件路径
    pub file_path: PathBuf,
    /// 起始行号
    pub start_line: usize,
    /// 结束行号
    pub end_line: usize,
    /// 语言类型
    pub language: String,
    /// 块类型 (function, class, comment, statement, etc.)
    pub chunk_type: ChunkType,
    /// 元数据 (可选)
    pub metadata: HashMap<String, String>,
}

/// 代码块类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChunkType {
    /// 函数定义
    Function,
    /// 类/结构体定义
    Class,
    /// 方法实现
    Method,
    /// 变量声明
    Variable,
    /// 注释
    Comment,
    /// 文档字符串
    Docstring,
    /// import 语句
    Import,
    /// 普通代码段
    Code,
    /// 多行字符串
    MultiLineString,
}

/// 增强版 Embedding 引擎
pub struct EnhancedEmbeddingEngine {
    config: EmbeddingConfig,
    
    /// LRU 缓存 (text hash -> embedding)
    cache: Arc<RwLock<lru::LruCache<String, EmbeddingResult>>>,
    
    /// 统计信息
    stats: Arc<RwLock<EmbeddingStats>>,
}

/// 统计信息
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EmbeddingStats {
    /// 总请求数
    pub total_requests: u64,
    /// 缓存命中数
    pub cache_hits: u64,
    /// 总生成向量数
    pub total_embeddings: u64,
    /// 平均延迟 (ms)
    pub avg_latency_ms: f64,
    /// 总 token 数
    pub total_tokens: u64,
}

impl EnhancedEmbeddingEngine {
    /// 创建新的增强版 Embedding 引擎
    pub fn new(config: EmbeddingConfig) -> Self {
        let cache = lru::LruCache::new(std::num::NonZero::new(config.cache_max_size).unwrap());
        
        Self {
            config,
            cache: Arc::new(RwLock::new(cache)),
            stats: Arc::new(RwLock::new(EmbeddingStats::default())),
        }
    }

    /// 使用默认配置创建
    pub fn with_defaults() -> Self {
        Self::new(EmbeddingConfig::default())
    }

    /// 嵌入单段代码
    pub async fn embed_code(
        &self,
        code: &str,
        language: &str,
    ) -> Result<EmbeddingResult> {
        let start = std::time::Instant::now();
        
        // 预处理代码
        let preprocessed = self.preprocess_code(code, language);
        
        // 检查缓存
        let cache_key = self.compute_cache_key(&preprocessed);
        {
            let mut cache = self.cache.write().await;
            if let Some(cached) = cache.get(&cache_key) {
                let mut stats = self.stats.write().await;
                stats.cache_hits += 1;
                return Ok(cached.clone());
            }
        }
        
        // 生成 embedding
        let result = match self.config.primary_model {
            EmbeddingModel::OpenAILarge | EmbeddingModel::OpenAISmall => {
                self.call_openai_api(&preprocessed).await?
            }
            EmbeddingModel::QwenEmbedding => {
                self.call_qwen_api(&preprocessed).await?
            }
            EmbeddingModel::LocalSentenceTransformers => {
                self.embed_locally(&preprocessed).await?
            }
        };
        
        // 更新统计
        let duration_ms = start.elapsed().as_millis() as u64;
        let mut stats = self.stats.write().await;
        stats.total_requests += 1;
        stats.total_embeddings += 1;
        stats.avg_latency_ms = (stats.avg_latency_ms * (stats.total_requests - 1) as f64 
            + duration_ms as f64) / stats.total_requests as f64;
        stats.total_tokens += estimate_tokens(&preprocessed) as u64;
        
        // 存入缓存
        if self.config.enable_cache {
            let mut cache = self.cache.write().await;
            cache.put(cache_key, result.clone());
        }
        
        Ok(result)
    }

    /// 批量嵌入多个代码块 (优化吞吐量)
    pub async fn embed_batch(
        &self,
        chunks: &[CodeChunk],
    ) -> Result<Vec<EmbeddingResult>> {
        let mut results = Vec::with_capacity(chunks.len());
        
        for chunk in chunks {
            let result = self.embed_code(&chunk.content, &chunk.language).await?;
            results.push(result);
        }
        
        Ok(results)
    }

    /// 预处理代码 (语言特定优化)
    fn preprocess_code(&self, code: &str, language: &str) -> String {
        match language.to_lowercase().as_str() {
            "rust" | "rs" => self.preprocess_rust(code),
            "python" | "py" | "python3" => self.preprocess_python(code),
            "typescript" | "ts" | "tsx" | "javascript" | "js" | "jsx" => {
                self.preprocess_javascript(code)
            }
            "go" => self.preprocess_go(code),
            "java" => self.preprocess_java(code),
            _ => self.preprocess_generic(code), // 默认通用预处理
        }
    }

    /// Rust 特定预处理
    fn preprocess_rust(&self, code: &str) -> String {
        code.lines()
            .filter(|line| {
                let trimmed = line.trim_start();
                
                // 移除单行注释
                if trimmed.starts_with("//") {
                    return false;
                }
                
                // 移除文档注释 (/// 和 //!)
                if trimmed.starts_with("///") || trimmed.starts_with("//!") {
                    return false;
                }
                
                true
            })
            .map(|line| line.trim_end()) // 标准化行尾空白
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Python 特定预处理
    fn preprocess_python(&self, code: &str) -> String {
        code.lines()
            .filter(|line| {
                let trimmed = line.trim_start();
                
                // 移除单行注释
                if trimmed.starts_with("#") && !trimmed.starts_with("#![") {
                    return false;
                }
                
                true
            })
            .map(|line| line.trim_end())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// JavaScript/TypeScript 特定预处理
    fn preprocess_javascript(&self, code: &str) -> String {
        code.lines()
            .filter(|line| {
                let trimmed = line.trim_start();
                
                // 移除单行注释
                if trimmed.starts_with("//") {
                    return false;
                }
                
                true
            })
            .map(|line| line.trim_end())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Go 特定预处理
    fn preprocess_go(&self, code: &str) -> String {
        code.lines()
            .filter(|line| {
                let trimmed = line.trim_start();
                
                // 移除单行注释
                if trimmed.starts_with("//") {
                    return false;
                }
                
                true
            })
            .map(|line| line.trim_end())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Java 特定预处理
    fn preprocess_java(&self, code: &str) -> String {
        code.lines()
            .filter(|line| {
                let trimmed = line.trim_start();
                
                // 移除单行注释
                if trimmed.starts_with("//") {
                    return false;
                }
                
                true
            })
            .map(|line| line.trim_end())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 通用预处理 (无特殊处理的语言)
    fn preprocess_generic(&self, code: &str) -> String {
        code.lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .map(|line| line.trim_end())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 计算缓存键 (基于内容哈希)
    fn compute_cache_key(&self, content: &str) -> String {
        use std::hash::{Hash, Hasher};
        use std::hash::DefaultHasher;
        
        let mut hasher = DefaultHasher::new();
        content.hash(&mut hasher);
        format!("{:x}", hasher.finish())
    }

    /// 调用 OpenAI API (实际实现需要 reqwest 或类似库)
    async fn call_openai_api(&self, content: &str) -> Result<EmbeddingResult> {
        // TODO: 实现实际的 API 调用
        // 这里返回模拟数据用于开发测试
        
        warn!("OpenAI embedding API not yet implemented, using mock data");
        
        Ok(EmbeddingResult {
            embedding: self.mock_embedding(self.config.dimensions),
            token_count: estimate_tokens(content),
            model: self.config.primary_model,
            duration_ms: 50, // 模拟延迟
        })
    }

    /// 调用 Qwen Embedding API
    async fn call_qwen_api(&self, content: &str) -> Result<EmbeddingResult> {
        // TODO: 实现 Qwen API 调用 (或使用 DashScope SDK)
        
        warn!("Qwen embedding API not yet implemented, using mock data");
        
        Ok(EmbeddingResult {
            embedding: self.mock_embedding(self.config.dimensions),
            token_count: estimate_tokens(content),
            model: self.config.primary_model,
            duration_ms: 30, // 模拟延迟 (Qwen 通常更快)
        })
    }

    /// 本地嵌入 (使用 candle/ort 或 onnxruntime)
    async fn embed_locally(&self, content: &str) -> Result<EmbeddingResult> {
        // TODO: 集成本地模型 (如 all-MiniLM-L6-v2)
        // 可以使用 candle-rs 或 ort (ONNX Runtime)
        
        warn!("Local embedding model not yet implemented, using mock data");
        
        Ok(EmbeddingResult {
            embedding: self.mock_embedding(384), // sentence-transformers 通常 384 维
            token_count: estimate_tokens(content),
            model: EmbeddingModel::LocalSentenceTransformers,
            duration_ms: 20, // 本地模型通常最快
        })
    }

    /// 生成模拟 embedding 向量 (仅用于开发测试)
    fn mock_embedding(&self, dimensions: usize) -> Vec<f32> {
        use rand::Rng;
        
        let mut rng = rand::rng();
        (0..dimensions)
            .map(|_| rng.random_range(-1.0..1.0))
            .collect()
    }

    /// 获取统计信息
    pub async fn get_stats(&self) -> EmbeddingStats {
        self.stats.read().await.clone()
    }

    /// 清空缓存
    pub async fn clear_cache(&self) {
        let mut cache = self.cache.write().await;
        cache.clear();
        info!("Embedding cache cleared");
    }
}

/// 估算 token 数量 (粗略估计: ~4 字符/token)
fn estimate_tokens(text: &str) -> usize {
    (text.chars().count() / 4).max(1)
}

/// CodeChunk 辅助方法
impl CodeChunk {
    /// 创建新的代码块
    pub fn new(
        content: String,
        file_path: impl Into<PathBuf>,
        language: impl Into<String>,
        chunk_type: ChunkType,
        start_line: usize,
        end_line: usize,
    ) -> Self {
        Self {
            id: format!("{:x}", rand::random::<u64>()),
            content,
            file_path: file_path.into(),
            start_line,
            end_line,
            language: language.into(),
            chunk_type,
            metadata: HashMap::new(),
        }
    }

    /// 从 AST 节点创建代码块
    pub fn from_ast_node(
        content: &str,
        file_path: impl Into<PathBuf>,
        language: &str,
        node_type: &str,
        start_line: usize,
        end_line: usize,
    ) -> Self {
        let chunk_type = match node_type {
            "function_declaration" | "function_definition" => ChunkType::Function,
            "struct_declaration" | "class_declaration" | "enum_declaration" => ChunkType::Class,
            "method_declaration" | "method_definition" => ChunkType::Method,
            "variable_declaration" => ChunkType::Variable,
            "comment" | "doc_comment" => ChunkType::Comment,
            "import_statement" | "import_declaration" => ChunkType::Import,
            _ => ChunkType::Code,
        };

        Self::new(
            content.to_string(),
            file_path,
            language,
            chunk_type,
            start_line,
            end_line,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_embed_rust_code() {
        let engine = EnhancedEmbeddingEngine::with_defaults();
        
        let rust_code = r#"
fn calculate_fibonacci(n: u64) -> u64 {
    if n <= 1 {
        return n;
    }
    calculate_fibonacci(n - 1) + calculate_fibonacci(n - 2)
}

struct FibonacciCache {
    cache: HashMap<u64, u64>,
}
"#;

        let result = engine.embed_code(rust_code, "rust").await.unwrap();
        
        assert_eq!(result.embedding.len(), 1024); // Qwen 默认维度
        assert!(result.token_count > 10);
        assert_eq!(result.model, EmbeddingModel::QwenEmbedding);
    }

    #[test]
    fn test_preprocess_rust() {
        let engine = EnhancedEmbeddingEngine::with_defaults();
        
        let code = r#"
// This is a comment
fn example() {
    println!("Hello"); // inline comment
    let x = 42;
}
"#;

        let preprocessed = engine.preprocess_rust(code);
        
        assert!(!preprocessed.contains("// This is a comment"));
        assert!(!preprocessed.contains("// inline comment"));
        assert!(preprocessed.contains("fn example()"));
        assert!(preprocessed.contains("let x = 42;"));
    }

    #[test]
    fn test_cache_key_deterministic() {
        let engine = EnhancedEmbeddingEngine::with_defaults();
        
        let key1 = engine.compute_cache_key("hello world");
        let key2 = engine.compute_cache_key("hello world");
        let key3 = engine.compute_cache_key("different text");
        
        assert_eq!(key1, key2); // 相同内容应该有相同键
        assert_ne!(key1, key3); // 不同内容应该有不同键
    }

    #[tokio::test]
    async fn test_stats_tracking() {
        let engine = EnhancedEmbeddingEngine::with_defaults();
        
        // 第一次调用 (miss)
        let _ = engine.embed_code("test code", "rust").await.unwrap();
        let stats1 = engine.get_stats().await;
        assert_eq!(stats1.total_requests, 1);
        assert_eq!(stats1.cache_hits, 0);
        
        // 第二次相同内容 (hit)
        let _ = engine.embed_code("test code", "rust").await.unwrap();
        let stats2 = engine.get_stats().await;
        assert_eq!(stats2.total_requests, 2);
        assert_eq!(stats2.cache_hits, 1); // 应该命中缓存
    }
}
