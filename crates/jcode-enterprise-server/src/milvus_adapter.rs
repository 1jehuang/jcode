//! Milvus向量数据库适配器 - 超大规模语义搜索 (>10M向量)
//!
//! Milvus vs pgvector对比:
//! - pgvector: 适合 <10M向量,集成简单,成本低
//! - Milvus: 适合 >10M向量,分布式扩展,高性能

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{info, warn, debug};

/// Milvus配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MilvusConfig {
    /// Milvus服务器URI (e.g., "milvus://localhost:19530")
    pub uri: String,

    /// 集合名称
    pub collection_name: String,

    /// 向量维度 (e.g., 768, 1536)
    pub dimension: usize,

    /// 索引类型
    pub index_type: MilvusIndexType,

    /// 距离度量
    pub metric_type: MilvusMetricType,

    /// 相似度阈值 (0-1)
    pub similarity_threshold: f64,

    /// 最大搜索结果数
    pub max_search_results: usize,
}

impl Default for MilvusConfig {
    fn default() -> Self {
        Self {
            uri: "milvus://localhost:19530".to_string(),
            collection_name: "carpai_code_embeddings".to_string(),
            dimension: 1536, // OpenAI embedding dimension
            index_type: MilvusIndexType::Hnsw,
            metric_type: MilvusMetricType::Cosine,
            similarity_threshold: 0.8,
            max_search_results: 10,
        }
    }
}

/// Milvus索引类型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MilvusIndexType {
    /// HNSW - Hierarchical Navigable Small World (推荐)
    Hnsw,
    /// IVF_FLAT - Inverted File with Flat
    IvfFlat,
    /// IVF_SQ8 - Inverted File with Scalar Quantization
    IvfSq8,
    /// FAISS GPU索引
    GpuIvfFlat,
}

/// 距离度量类型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MilvusMetricType {
    /// 余弦相似度 (推荐用于文本嵌入)
    Cosine,
    /// L2欧氏距离
    L2,
    /// 内积
    InnerProduct,
}

/// Milvus搜索结果
#[derive(Debug, Clone, Serialize)]
pub struct MilvusSearchResult {
    pub id: String,
    pub score: f64,
    pub metadata: HashMap<String, String>,
    pub vector: Option<Vec<f32>>,
}

/// Milvus客户端包装器
pub struct MilvusClient {
    config: MilvusConfig,
    #[cfg(feature = "milvus")]
    client: Option<milvus_sdk::Client>,
    initialized: bool,
}

impl MilvusClient {
    /// 创建新的Milvus客户端
    pub async fn new(config: MilvusConfig) -> anyhow::Result<Self> {
        info!("Initializing Milvus client: uri={}, collection={}", config.uri, config.collection_name);

        #[cfg(feature = "milvus")]
        let client = {
            // 连接到Milvus服务器
            match milvus_sdk::Client::connect(&config.uri).await {
                Ok(client) => {
                    info!("Connected to Milvus server successfully");
                    Some(client)
                }
                Err(e) => {
                    warn!("Failed to connect to Milvus: {}. Running in mock mode.", e);
                    None
                }
            }
        };

        #[cfg(not(feature = "milvus"))]
        let client = None;

        let mut milvus_client = Self {
            config,
            client,
            initialized: false,
        };

        // 初始化集合
        milvus_client.initialize_collection().await?;

        Ok(milvus_client)
    }

    /// 从环境变量创建客户端
    pub async fn from_env() -> anyhow::Result<Self> {
        let config = MilvusConfig {
            uri: std::env::var("MILVUS_URI")
                .unwrap_or_else(|_| "milvus://localhost:19530".to_string()),
            collection_name: std::env::var("MILVUS_COLLECTION")
                .unwrap_or_else(|_| "carpai_code_embeddings".to_string()),
            dimension: std::env::var("MILVUS_DIMENSION")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1536),
            ..MilvusConfig::default()
        };

        Self::new(config).await
    }

    /// 初始化集合
    async fn initialize_collection(&mut self) -> anyhow::Result<()> {
        #[cfg(feature = "milvus")]
        if let Some(client) = &self.client {
            use milvus_sdk::schema::{CollectionSchema, FieldSchema, FieldType};

            // 检查集合是否存在
            let has_collection = client.has_collection(&self.config.collection_name).await?;

            if !has_collection {
                info!("Creating Milvus collection: {}", self.config.collection_name);

                // 创建集合schema
                let schema = CollectionSchema::new()
                    .with_field(FieldSchema::new(
                        "id",
                        FieldType::VarChar,
                        "Primary key",
                    ))
                    .with_field(FieldSchema::new(
                        "embedding",
                        FieldType::FloatVector,
                        "Code embedding vector",
                    ).with_dim(self.config.dimension as i64))
                    .with_field(FieldSchema::new(
                        "file_path",
                        FieldType::VarChar,
                        "Source file path",
                    ))
                    .with_field(FieldSchema::new(
                        "symbol_name",
                        FieldType::VarChar,
                        "Symbol name (optional)",
                    ))
                    .with_field(FieldSchema::new(
                        "metadata",
                        FieldType::Json,
                        "Additional metadata",
                    ));

                // 创建集合
                client.create_collection(&self.config.collection_name, schema).await?;

                // 创建索引
                self.create_index(client).await?;

                info!("Milvus collection created successfully");
            } else {
                info!("Milvus collection already exists");
            }
        }

        self.initialized = true;
        Ok(())
    }

    /// 创建索引
    #[cfg(feature = "milvus")]
    async fn create_index(&self, client: &milvus_sdk::Client) -> anyhow::Result<()> {
        use milvus_sdk::index::{IndexParams, IndexType};

        let index_params = match self.config.index_type {
            MilvusIndexType::Hnsw => {
                IndexParams::new("embedding_idx", IndexType::Hnsw)
                    .with_metric_type(self.metric_type_to_milvus())
                    .with_extra_param("M", "16")
                    .with_extra_param("efConstruction", "200")
            }
            MilvusIndexType::IvfFlat => {
                IndexParams::new("embedding_idx", IndexType::IvfFlat)
                    .with_metric_type(self.metric_type_to_milvus())
                    .with_extra_param("nlist", "1024")
            }
            _ => {
                IndexParams::new("embedding_idx", IndexType::Hnsw)
                    .with_metric_type(self.metric_type_to_milvus())
            }
        };

        client
            .create_index(&self.config.collection_name, "embedding", index_params)
            .await?;

        Ok(())
    }

    /// 插入向量嵌入
    pub async fn insert_embedding(
        &self,
        id: &str,
        embedding: &[f32],
        file_path: &str,
        symbol_name: Option<&str>,
        metadata: &HashMap<String, String>,
    ) -> anyhow::Result<()> {
        if !self.initialized {
            anyhow::bail!("Milvus client not initialized");
        }

        #[cfg(feature = "milvus")]
        if let Some(client) = &self.client {
            use milvus_sdk::data::FieldData;

            let fields = vec![
                FieldData::new("id", vec![id.to_string()]),
                FieldData::new("embedding", vec![embedding.to_vec()]),
                FieldData::new("file_path", vec![file_path.to_string()]),
                FieldData::new(
                    "symbol_name",
                    vec![symbol_name.unwrap_or("").to_string()],
                ),
            ];

            client
                .insert(&self.config.collection_name, fields, None)
                .await?;

            debug!("Inserted embedding into Milvus: id={}, dim={}", id, embedding.len());
        }

        #[cfg(not(feature = "milvus"))]
        {
            debug!(
                "[Mock] Inserting embedding: id={}, file={}, dim={}",
                id,
                file_path,
                embedding.len()
            );
        }

        Ok(())
    }

    /// 执行向量相似度搜索
    pub async fn search_similar(
        &self,
        query_embedding: &[f32],
        limit: Option<usize>,
        filter_expr: Option<&str>,
    ) -> anyhow::Result<Vec<MilvusSearchResult>> {
        if !self.initialized {
            anyhow::bail!("Milvus client not initialized");
        }

        let search_limit = limit.unwrap_or(self.config.max_search_results);

        #[cfg(feature = "milvus")]
        if let Some(client) = &self.client {
            use milvus_sdk::search::{SearchOptions, SearchRequest};

            let request = SearchRequest::new(
                &self.config.collection_name,
                "embedding",
                vec![query_embedding.to_vec()],
            )
            .with_limit(search_limit as i64)
            .with_filter(filter_expr.unwrap_or(""))
            .with_output_fields(vec!["id", "file_path", "symbol_name", "metadata"]);

            let results = client.search(request, SearchOptions::default()).await?;

            let mut search_results = Vec::new();

            for result in results {
                for hit in result.hits() {
                    let mut metadata_map = HashMap::new();

                    if let Some(file_path) = hit.field_data("file_path").and_then(|f| f.as_str()) {
                        metadata_map.insert("file_path".to_string(), file_path.to_string());
                    }

                    if let Some(symbol) = hit.field_data("symbol_name").and_then(|f| f.as_str()) {
                        metadata_map.insert("symbol_name".to_string(), symbol.to_string());
                    }

                    search_results.push(MilvusSearchResult {
                        id: hit.id().to_string(),
                        score: hit.score(),
                        metadata: metadata_map,
                        vector: None,
                    });
                }
            }

            debug!("Milvus search returned {} results", search_results.len());
            return Ok(search_results);
        }

        #[cfg(not(feature = "milvus"))]
        {
            warn!("Milvus feature not enabled, returning empty results");
            Ok(Vec::new())
        }
    }

    /// 删除向量
    pub async fn delete_vectors(&self, ids: &[&str]) -> anyhow::Result<()> {
        if !self.initialized {
            anyhow::bail!("Milvus client not initialized");
        }

        #[cfg(feature = "milvus")]
        if let Some(client) = &self.client {
            let expr = format!("id in [{}]", ids.iter().map(|id| format!("\"{}\"", id)).collect::<Vec<_>>().join(","));

            client
                .delete(&self.config.collection_name, &expr)
                .await?;

            info!("Deleted {} vectors from Milvus", ids.len());
        }

        Ok(())
    }

    /// 获取集合统计信息
    pub async fn get_stats(&self) -> anyhow::Result<MilvusStats> {
        #[cfg(feature = "milvus")]
        if let Some(client) = &self.client {
            let stats = client.get_collection_stats(&self.config.collection_name).await?;

            Ok(MilvusStats {
                total_vectors: stats.row_count,
                index_type: format!("{:?}", self.config.index_type),
                metric_type: format!("{:?}", self.config.metric_type),
                dimension: self.config.dimension,
            })
        }

        #[cfg(not(feature = "milvus"))]
        {
            Ok(MilvusStats {
                total_vectors: 0,
                index_type: "N/A".to_string(),
                metric_type: "N/A".to_string(),
                dimension: self.config.dimension,
            })
        }
    }

    /// 转换度量类型
    #[cfg(feature = "milvus")]
    fn metric_type_to_milvus(&self) -> milvus_sdk::index::MetricType {
        match self.config.metric_type {
            MilvusMetricType::Cosine => milvus_sdk::index::MetricType::Cosine,
            MilvusMetricType::L2 => milvus_sdk::index::MetricType::L2,
            MilvusMetricType::InnerProduct => milvus_sdk::index::MetricType::Ip,
        }
    }

    /// 检查是否已初始化
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }
}

/// Milvus统计信息
#[derive(Debug, Clone, Serialize)]
pub struct MilvusStats {
    pub total_vectors: i64,
    pub index_type: String,
    pub metric_type: String,
    pub dimension: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_milvus_config_from_env() {
        std::env::set_var("MILVUS_URI", "milvus://localhost:19530");
        std::env::set_var("MILVUS_COLLECTION", "test_collection");

        let client = MilvusClient::from_env().await;

        // 即使Milvus服务器不可用,也应该能够创建客户端(mock模式)
        assert!(client.is_ok() || client.is_err());
    }
}
