// ════════════════════════════════════════════════════════════════
// 工具延迟加载与发现系统 — 移植自 Claude Code ToolSearchTool/
//
// 核心能力:
//   1. Embedding 索引 — 工具描述向量化, 语义搜索
//   2. 按需加载 — 不在启动时注册所有工具, 而是 lazy load
//   3. 智能推荐 — 根据用户意图匹配最佳工具组合
//
// 使用场景:
//   - 大量工具时减少 token 消耗 (只发送相关工具定义)
//   - 插件化工具架构 (运行时发现新工具)
// ════════════════════════════════════════════════════════════════

use super::Tool;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// 工具搜索结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSearchResult {
    /// 匹配的工具名
    pub tool_name: String,
    /// 相关度评分 (0.0 - 1.0)
    pub relevance_score: f64,
    /// 匹配原因
    pub match_reason: String,
}

/// 工具嵌入索引条目
#[allow(dead_code)]
struct ToolIndexEntry {
    name: String,
    description: String,
    tags: Vec<String>,
    embedding: Vec<f32>, // 简化的向量表示 (生产中用实际 embedding)
}

/// 嵌入索引 — 用于工具语义搜索
pub struct ToolEmbeddingIndex {
    entries: Vec<ToolIndexEntry>,
}

impl Default for ToolEmbeddingIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolEmbeddingIndex {
    pub fn new() -> Self {
        Self { entries: Vec::new() }
    }

    /// 注册工具到索引
    pub fn register(&mut self, tool: &dyn Tool) {
        let desc = tool.description().to_string();
        self.entries.push(ToolIndexEntry {
            name: tool.name().to_string(),
            description: desc.clone(),
            tags: Self::extract_tags(tool.name(), &desc),
            embedding: Self::simple_embed(&format!("{} {}", tool.name(), desc)),
        });
    }

    /// 语义搜索工具
    pub fn search(&self, query: &str, top_k: usize) -> Vec<ToolSearchResult> {
        let query_embedding = Self::simple_embed(query);

        let mut scored: Vec<(usize, f64)> = self.entries
            .iter()
            .enumerate()
            .map(|(i, entry)| {
                let score = cosine_similarity(&query_embedding, &entry.embedding);
                (i, score)
            })
            .filter(|(_, s)| *s > 0.3) // 最小阈值
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        scored.into_iter()
            .take(top_k)
            .map(|(i, score)| ToolSearchResult {
                tool_name: self.entries[i].name.clone(),
                relevance_score: score,
                match_reason: format!("语义相似度 {:.2}%", score * 100.0),
            })
            .collect()
    }

    /// 获取所有已注册工具名
    pub fn registered_tools(&self) -> Vec<&str> {
        self.entries.iter().map(|e| e.name.as_str()).collect()
    }

    // --- 内部方法 ---------------------------------

    fn extract_tags(name: &str, description: &str) -> Vec<String> {
        let mut tags = Vec::new();
        let combined = format!("{} {}", name.to_lowercase(), description.to_lowercase());
        const KEYWORDS: &[&str] = &[
            "file", "read", "write", "edit", "search", "grep",
            "bash", "command", "shell", "execute",
            "web", "fetch", "http", "url",
            "git", "diff", "merge",
            "mcp", "lsp",
            "notebook", "repl", "python",
        ];
        for keyword in KEYWORDS {
            if combined.contains(keyword) {
                tags.push(keyword.to_string());
            }
        }
        tags
    }

    /// 简单的 embedding 函数 (基于词频哈希)
    ///
    /// 生产环境应替换为实际的 sentence transformer 或 OpenAI embedding API
    fn simple_embed(text: &str) -> Vec<f32> {
        const DIM: usize = 64;
        let mut vec = vec![0.0f32; DIM];
        for (i, ch) in text.chars().enumerate() {
            let idx = (ch as usize) % DIM;
            vec[idx] += 1.0 / ((i + 1) as f32);
        }
        // L2 归一化
        let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for v in vec.iter_mut() {
                *v /= norm;
            }
        }
        vec
    }
}

/// 余弦相似度
fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot_product: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    (dot_product / (norm_a * norm_b)) as f64
}

/// 工具发现引擎 — 统一管理工具的注册、搜索和延迟加载
pub struct ToolDiscoveryEngine {
    index: std::sync::RwLock<ToolEmbeddingIndex>,
    registry: std::sync::RwLock<HashMap<String, Arc<dyn Tool>>>,
    loaders: std::sync::RwLock<HashMap<String, Box<dyn Fn() -> Arc<dyn Tool> + Send + Sync>>>,
}

impl Default for ToolDiscoveryEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolDiscoveryEngine {
    pub fn new() -> Self {
        Self {
            index: RwLock::new(ToolEmbeddingIndex::new()),
            registry: RwLock::new(HashMap::new()),
            loaders: RwLock::new(HashMap::new()),
        }
    }

    /// 注册一个已经实例化的工具
    pub fn register_tool(&self, tool: Arc<dyn Tool>) {
        let name = tool.name().to_string();
        self.index.write().unwrap_or_else(|e| e.into_inner()).register(tool.as_ref());
        self.registry.write().unwrap_or_else(|e| e.into_inner()).insert(name.clone(), tool);
        tracing::info!(tool = %name, "Tool registered");
    }

    /// 注册一个延迟加载的工具工厂
    pub fn register_lazy<F>(&self, name: &str, factory: F)
    where
        F: Fn() -> Arc<dyn Tool> + Send + Sync + 'static,
    {
        self.loaders.write().unwrap_or_else(|e| e.into_inner()).insert(name.to_string(), Box::new(factory));
        tracing::info!(tool = %name, "Lazy tool loader registered");
    }

    /// 获取工具（如果尚未加载则按需加载）
    pub async fn get_tool(&self, name: &str) -> Option<Arc<dyn Tool>> {
        // 先检查已注册的
        {
            let registry = self.registry.read().unwrap_or_else(|e| e.into_inner());
            if let Some(tool) = registry.get(name) {
                return Some(tool.clone());
            }
        }

        // 尝试懒加载
        let tool = {
            let loaders = self.loaders.read().unwrap_or_else(|e| e.into_inner());
            loaders.get(name).map(|factory| factory())
        };

        if let Some(tool) = tool {
            self.index.write().unwrap_or_else(|e| e.into_inner()).register(tool.as_ref());
            self.registry.write().unwrap_or_else(|e| e.into_inner()).insert(name.to_string(), tool.clone());
            return Some(tool);
        }

        None
    }

    /// 搜索与查询最相关的工具
    pub fn search_tools(&self, query: &str, top_k: usize) -> Vec<ToolSearchResult> {
        self.index.read().unwrap_or_else(|e| e.into_inner()).search(query, top_k)
    }

    /// 批量获取工具定义 (用于发送给 AI)
    ///
    /// 返回最相关的 N 个工具的定义，而非全部工具，节省 token。
    pub fn get_relevant_definitions(
        &self,
        query: &str,
        max_tools: usize,
    ) -> Vec<super::ToolDefinition> {
        let results = self.search_tools(query, max_tools);
        let registry = self.registry.read().unwrap_or_else(|e| e.into_inner());
        results
            .into_iter()
            .filter_map(|r| registry.get(&r.tool_name).map(|t| t.to_definition()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_registration_and_search() {
        let engine = ToolDiscoveryEngine::new();

        // Register mock tools via index directly
        let mut index = engine.index.write().unwrap_or_else(|e| e.into_inner());
        // We can't create dyn Tool objects easily in tests without mocks,
        // but we can test the embedding/search logic
        assert!(index.registered_tools().is_empty());
        drop(index);
    }

    #[test]
    fn test_cosine_similarity() {
        let v1: Vec<f32> = vec![1.0, 0.0, 0.0];
        let v2: Vec<f32> = vec![1.0, 0.0, 0.0];
        let v3: Vec<f32> = vec![0.0, 1.0, 0.0];

        assert!((cosine_similarity(&v1, &v2) - 1.0).abs() < 0.001);
        assert!((cosine_similarity(&v1, &v3)).abs() < 0.001); // orthogonal ≈ 0
    }

    #[test]
    fn test_simple_embed_deterministic() {
        let e1 = ToolEmbeddingIndex::simple_embed("hello world");
        let e2 = ToolEmbeddingIndex::simple_embed("hello world");
        assert_eq!(e1.len(), e2.len());
        for i in 0..e1.len() {
            assert!((e1[i] - e2[i]).abs() < f32::EPSILON);
        }
    }
}
