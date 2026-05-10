//! 语义缓存与自动上下文注入 — 四层融合
//!
//! Layer 1: 文件级 — 打开的文件名 → 匹配相关记忆
//! Layer 2: 符号级 — LSP documentSymbol → 函数/类型级检索
//! Layer 3: 语义级 — 代码段 embedding → 余弦相似度检索
//! Layer 4: 依赖级 — DependencyGraph → 关联文件变更历史

use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

// ══════════════════════════════════════════════════════════════════
// 基础类型
// ══════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct Embedding(pub Vec<f32>);

#[derive(Debug, Clone)]
pub struct SemanticEntry {
    pub key: String,
    pub content: String,
    pub embedding: Embedding,
    pub source_file: Option<String>,
    pub source_symbol: Option<String>,  // Layer 2: 关联的符号名
    pub dependencies: Vec<String>,      // Layer 4: 依赖的文件列表
    pub hit_count: u64,
}

#[derive(Debug, Clone)]
pub struct SimilarityResult {
    pub key: String,
    pub content: String,
    pub score: f64,
    pub layer: u8,                     // 来自哪一层
}

#[derive(Debug, Clone)]
pub struct ContextInjectResult {
    pub results: Vec<SimilarityResult>,
    pub layers_activated: Vec<u8>,
    pub total_candidates: usize,
}

// ══════════════════════════════════════════════════════════════════
// 四层上下文融合器
// ══════════════════════════════════════════════════════════════════

/// 四层上下文融合器
pub struct ContextFusion {
    cache: Arc<SemanticCache>,
    dep_graph: Arc<RwLock<HashMap<String, Vec<String>>>>,  // Layer 4: 依赖图
    symbol_index: Arc<RwLock<HashMap<String, Vec<String>>>>, // Layer 2: 文件→符号
}

impl ContextFusion {
    pub fn new() -> Self {
        Self {
            cache: Arc::new(SemanticCache::new()),
            dep_graph: Arc::new(RwLock::new(HashMap::new())),
            symbol_index: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn cache(&self) -> &SemanticCache { &self.cache }

    /// Layer 4 注册: 添加依赖关系
    pub fn register_dependency(&self, file: &str, depends_on: Vec<String>) {
        self.dep_graph.write().insert(file.to_string(), depends_on);
    }

    /// Layer 2 注册: 添加文件的符号列表
    pub fn register_symbols(&self, file: &str, symbols: Vec<String>) {
        self.symbol_index.write().insert(file.to_string(), symbols);
    }

    /// 四层融合检索
    pub fn retrieve_context(&self, open_files: &[String], top_k: usize) -> ContextInjectResult {
        let mut all = Vec::new();
        let mut layers = Vec::new();

        // Layer 1: 文件级
        let l1 = self.layer1_file_match(open_files);
        if !l1.is_empty() { layers.push(1); }
        all.extend(l1);

        // Layer 2: 符号级
        let l2 = self.layer2_symbol_match(open_files);
        if !l2.is_empty() { layers.push(2); }
        all.extend(l2);

        // Layer 3: 语义级 (需要 embedding 查询)
        // 调用方需要提供当前代码段的 embedding
        // 如果没有提供，跳过此层

        // Layer 4: 依赖级
        let l4 = self.layer4_dependency_match(open_files);
        if !l4.is_empty() { layers.push(4); }
        all.extend(l4);

        // 合并去重排序
        all.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        all.truncate(top_k);
        all.dedup_by(|a, b| a.key == b.key);

        ContextInjectResult {
            total_candidates: all.len(),
            layers_activated: layers,
            results: all,
        }
    }

    /// Layer 1: 文件名匹配
    fn layer1_file_match(&self, open_files: &[String]) -> Vec<SimilarityResult> {
        let mut results = Vec::new();
        for file in open_files {
            for entry in self.cache.entries.read().iter() {
                if let Some(ref src) = entry.source_file {
                    if src == file || file.contains(src) || src.contains(file) {
                        results.push(SimilarityResult {
                            key: entry.key.clone(),
                            content: entry.content.clone(),
                            score: 0.9 + (entry.hit_count as f64 * 0.01).min(0.1),
                            layer: 1,
                        });
                    }
                }
            }
        }
        results
    }

    /// Layer 2: 符号级 — 检索当前文件中函数/类型相关的记忆
    fn layer2_symbol_match(&self, open_files: &[String]) -> Vec<SimilarityResult> {
        let mut results = Vec::new();
        let symbols = self.symbol_index.read();

        for file in open_files {
            if let Some(file_symbols) = symbols.get(file) {
                for symbol in file_symbols {
                    for entry in self.cache.entries.read().iter() {
                        // 缓存条目标记了符号名时匹配
                        if let Some(ref es) = entry.source_symbol {
                            if es == symbol || symbol.contains(es) || es.contains(symbol) {
                                results.push(SimilarityResult {
                                    key: entry.key.clone(),
                                    content: entry.content.clone(),
                                    score: 0.95,  // 符号级匹配 → 高置信度
                                    layer: 2,
                                });
                            }
                        }
                    }
                }
            }
        }
        results
    }

    /// Layer 3: 语义级 — 嵌入向量余弦相似度检索
    pub fn layer3_semantic_match(&self, query: &Embedding, top_k: usize) -> Vec<SimilarityResult> {
        self.cache.search(query, top_k)
            .into_iter()
            .map(|r| SimilarityResult { layer: 3, ..r })
            .collect()
    }

    /// Layer 4: 依赖级 — 当前文件依赖的文件 → 检索这些文件的历史记忆
    fn layer4_dependency_match(&self, open_files: &[String]) -> Vec<SimilarityResult> {
        let mut results = Vec::new();
        let graph = self.dep_graph.read();

        for file in open_files {
            if let Some(deps) = graph.get(file) {
                for dep in deps {
                    for entry in self.cache.entries.read().iter() {
                        if let Some(ref src) = entry.source_file {
                            if src.contains(dep) || dep.contains(src) {
                                results.push(SimilarityResult {
                                    key: entry.key.clone(),
                                    content: entry.content.clone(),
                                    score: 0.85,  // 依赖级 → 较高置信度
                                    layer: 4,
                                });
                            }
                        }
                    }
                }
            }
        }
        results
    }

    /// 生成注入 prompt
    pub fn format_injection_prompt(&self, results: &[SimilarityResult]) -> String {
        if results.is_empty() {
            return String::new();
        }

        let mut prompt = String::from("<relevant-context>\n");
        for r in results {
            prompt.push_str(&format!("[L{}] {} ({:.1}%): {}\n",
                r.layer, r.key, r.score * 100.0, r.content));
        }
        prompt.push_str("</relevant-context>");
        prompt
    }
}

// ══════════════════════════════════════════════════════════════════
// 语义缓存 (单层)
// ══════════════════════════════════════════════════════════════════

pub struct SemanticCache {
    pub(super) entries: Arc<RwLock<Vec<SemanticEntry>>>,
}

impl SemanticCache {
    pub fn new() -> Self {
        Self { entries: Arc::new(RwLock::new(Vec::new())) }
    }

    pub fn insert(&self, key: &str, content: &str, embedding: Embedding, source: Option<String>) {
        let mut entries = self.entries.write();
        if let Some(existing) = entries.iter_mut().find(|e| e.key == key) {
            existing.content = content.to_string();
            existing.embedding = embedding;
            existing.source_file = source;
        } else {
            entries.push(SemanticEntry {
                key: key.to_string(),
                content: content.to_string(),
                embedding,
                source_file: source,
                source_symbol: None,
                dependencies: vec![],
                hit_count: 0,
            });
        }
    }

    /// 带符号名和依赖的插入
    pub fn insert_with_context(
        &self, key: &str, content: &str, embedding: Embedding,
        source: Option<String>, symbol: Option<String>, deps: Vec<String>,
    ) {
        let mut entries = self.entries.write();
        entries.push(SemanticEntry {
            key: key.to_string(),
            content: content.to_string(),
            embedding,
            source_file: source,
            source_symbol: symbol,
            dependencies: deps,
            hit_count: 0,
        });
    }

    pub fn search(&self, query: &Embedding, top_k: usize) -> Vec<SimilarityResult> {
        let entries = self.entries.read();
        let mut scored: Vec<(f64, &SemanticEntry)> = entries
            .iter()
            .map(|e| (cosine_similarity(&query.0, &e.embedding.0), e))
            .collect();

        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.into_iter()
            .take(top_k)
            .filter(|(score, _)| *score > 0.7)  // 语义级阈值 0.7
            .map(|(score, entry)| SimilarityResult {
                key: entry.key.clone(),
                content: entry.content.clone(),
                score,
                layer: 3,
            })
            .collect()
    }
}

impl Default for SemanticCache { fn default() -> Self { Self::new() } }

#[inline]
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    if a.len() != b.len() || a.is_empty() { return 0.0; }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let mag_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let mag_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if mag_a == 0.0 || mag_b == 0.0 { return 0.0; }
    (dot / (mag_a * mag_b)) as f64
}
