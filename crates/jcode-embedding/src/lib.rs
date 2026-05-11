//! JCode Embedding - 代码语义向量化与上下文感知
//!
//! 提供核心的代码理解能力：
//! - Enhanced Embedding Engine: 多模型高质量代码向量生成
//! - File Activity Tracker: 用户行为追踪与相关性计算
//! - SymbolIndex: 多层级符号索引系统 (精确/前缀/模糊搜索)
//! - SemanticIndex: 语义级向量索引 (相似度检索)
//!
//! 使用示例:
//! ```rust
//! use jcode_embedding::{
//!     EnhancedEmbeddingEngine, FileActivityTracker,
//!     SymbolIndex, SemanticIndex
//! };
//! 
//! // 创建引擎
//! let engine = EnhancedEmbeddingEngine::with_defaults();
//! let result = engine.embed_code("fn main() {}", "rust").await?;
//! 
//! // 追踪文件活动
//! let tracker = FileActivityTracker::with_defaults();
//! tracker.record_access(Path::new("src/main.rs"));
//! 
//! // 符号索引
//! let symbol_idx = SymbolIndex::with_defaults();
//! symbol_idx.add_symbol("main", SymbolLocation::new(...));
//! let results = symbol_idx.prefix_search("ma", 10);
//! 
//! // 语义索引
//! let semantic_idx = SemanticIndex::with_defaults();
//! semantic_idx.index_code_chunk("id", &embedding, metadata).await?;
//! ```

pub mod enhanced_engine;
pub mod file_tracker;
pub mod symbol_index;
pub mod semantic_index;

// 重新导出主要类型
pub use enhanced_engine::{
    EnhancedEmbeddingEngine,
    EmbeddingConfig,
    EmbeddingModel,
    EmbeddingResult,
    CodeChunk,
    ChunkType,
};

pub use file_tracker::{
    FileActivityTracker,
    ActivityConfig,
    FileActivityRecord,
    RelevanceScore,
    RelevanceBreakdown,
    ActivityStats,
};

pub use symbol_index::{
    SymbolIndex,
    SymbolIndexConfig,
    SymbolLocation,
    SymbolKind,
    SymbolIndexStats,
};

pub use semantic_index::{
    SemanticIndex,
    SemanticIndexConfig,
    VectorDatabase,
    InMemoryVectorDB,
    VectorSearchResult,
    SemanticSearchResult,
    SimilarFunctionResult,
};
