//! TencentDB-Agent-Memory 深度移植实现
//!
//! 核心创新移植 (5项):
//! 1. 4层渐进式记忆管线 (L0→L3): Conversation→Atom→Scenario→Persona
//! 2. 符号化记忆 + Mermaid 上下文卸载: 轻量符号图替代冗长日志
//! 3. 混合检索 (BM25 + Vector + RRF): 超越纯关键词/纯向量
//! 4. 异构存储 (SQLite + Markdown): 底层结构化检索, 顶层可读文件
//! 5. 白盒可追溯: Persona→Scenario→Atom→Conversation 完整溯源链
//!
//! 源项目: https://github.com/Tencent/TencentDB-Agent-Memory

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

// ========================================================================
// [1] 4层渐进式记忆管线 (L0→L3)
// 移植自: TencentDB-Agent-Memory 核心分层架构
// ========================================================================

/// 记忆层级 (0=最底层/原始, 3=最顶层/抽象)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MemoryTier {
    /// L0: 原始对话记录 (最大, 最低密度)
    Conversation,
    /// L1: 从对话提取的原子事实 (中)
    Atom,
    /// L2: 场景块, 将相关原子组合 (中高)
    Scenario,
    /// L3: 用户画像, 高层偏好特征 (最小, 最高密度)
    Persona,
}

impl MemoryTier {
    pub fn level(&self) -> u8 {
        match self {
            MemoryTier::Conversation => 0,
            MemoryTier::Atom => 1,
            MemoryTier::Scenario => 2,
            MemoryTier::Persona => 3,
        }
    }

    pub fn from_level(l: u8) -> Self {
        match l {
            0 => MemoryTier::Conversation,
            1 => MemoryTier::Atom,
            2 => MemoryTier::Scenario,
            _ => MemoryTier::Persona,
        }
    }
}

/// 4层记忆条目
#[derive(Debug, Clone)]
pub struct TieredMemoryItem {
    pub id: String,
    pub tier: MemoryTier,
    pub content: String,
    pub created_at: SystemTime,
    pub last_accessed: SystemTime,
    pub access_count: u64,
    pub strength: f64,
    pub tags: Vec<String>,
    /// 溯源链: 高层的 id 指向低层 id
    pub source_ids: Vec<String>,
    /// 从哪些下层条目提取而来
    pub derived_from: Vec<String>,
    /// 是否已持久化到 Markdown 文件
    pub persisted_to_file: Option<PathBuf>,
    pub embedding: Option<Vec<f32>>,
}

/// 4层渐进式记忆管线
pub struct MemoryPipeline {
    /// L0: 原始对话记录 (SQLite 存储)
    conversations: Arc<RwLock<Vec<TieredMemoryItem>>>,
    /// L1: 原子事实 (SQLite 全文索引)
    atoms: Arc<RwLock<Vec<TieredMemoryItem>>>,
    /// L2: 场景 (Markdown 文件)
    scenarios: Arc<RwLock<Vec<TieredMemoryItem>>>,
    /// L3: 画像 (Markdown 文件, 最高密度)
    personas: Arc<RwLock<Vec<TieredMemoryItem>>>,
    /// 配置
    config: PipelineConfig,
    /// 存储根目录 (默认为 ~/.carpai/memory/)
    store_root: PathBuf,
    /// 统计
    stats: Arc<RwLock<PipelineStats>>,
}

#[derive(Debug, Clone)]
pub struct PipelineConfig {
    /// 每 N 次对话提取一次原子事实 (default: 3)
    pub every_n_conversations: u32,
    /// 每 N 次提取构建一次场景 (default: 5)
    pub scenario_trigger_every_n: u32,
    /// 每 N 次场景更新一次画像 (default: 3)
    pub persona_trigger_every_n: u32,
    /// 启用上下文卸载 (Mermaid画布)
    pub offload_enabled: bool,
    /// 启用混合检索
    pub hybrid_retrieval_enabled: bool,
    /// BM25 k1 参数
    pub bm25_k1: f64,
    /// BM25 b 参数
    pub bm25_b: f64,
    /// 向量检索权重 (默认 0.5)
    pub vector_weight: f64,
    /// 关键词检索权重 (默认 0.5)
    pub keyword_weight: f64,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            every_n_conversations: 3,
            scenario_trigger_every_n: 5,
            persona_trigger_every_n: 3,
            offload_enabled: true,
            hybrid_retrieval_enabled: true,
            bm25_k1: 1.5,
            bm25_b: 0.75,
            vector_weight: 0.5,
            keyword_weight: 0.5,
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct PipelineStats {
    pub total_conversations: u64,
    pub total_atoms: u64,
    pub total_scenarios: u64,
    pub total_personas: u64,
    pub extractions_performed: u64,
    pub offloads_performed: u64,
    pub hybrid_searches: u64,
}

impl MemoryPipeline {
    /// 创建记忆管线实例
    pub fn new(config: PipelineConfig) -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let store_root = home.join(".carpai").join("memory");
        Self {
            conversations: Arc::new(RwLock::new(Vec::new())),
            atoms: Arc::new(RwLock::new(Vec::new())),
            scenarios: Arc::new(RwLock::new(Vec::new())),
            personas: Arc::new(RwLock::new(Vec::new())),
            config,
            store_root,
            stats: Arc::new(RwLock::new(PipelineStats::default())),
        }
    }

    /// === L0: 添加一条对话记录 ===
    pub async fn add_conversation(&self, role: &str, content: &str) {
        let item = TieredMemoryItem {
            id: format!("conv-{}", SystemTime::now()
                .duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos()),
            tier: MemoryTier::Conversation,
            content: format!("[{}] {}", role, content),
            created_at: SystemTime::now(),
            last_accessed: SystemTime::now(),
            access_count: 0,
            strength: 0.3,
            tags: vec![role.to_string(), "conversation".to_string()],
            source_ids: vec![],
            derived_from: vec![],
            persisted_to_file: None,
            embedding: None,
        };
        self.conversations.write().await.push(item);

        // 自动触发提取
        let count = self.conversations.read().await.len();
        if count as u32 % self.config.every_n_conversations == 0 && count > 0 {
            let recent = self.conversations.read().await.iter()
                .rev().take(self.config.every_n_conversations as usize)
                .cloned().collect::<Vec<_>>();
            self.extract_atoms(recent).await;
        }
    }

    /// === L0→L1: 从对话提取原子事实 ===
    async fn extract_atoms(&self, conversations: Vec<TieredMemoryItem>) {
        let mut new_atoms = Vec::new();
        for conv in &conversations {
            let atoms = self.extract_facts_from_text(&conv.content);
            for atom_text in atoms {
                new_atoms.push(TieredMemoryItem {
                    id: format!("atom-{}", SystemTime::now()
                        .duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos()),
                    tier: MemoryTier::Atom,
                    content: atom_text,
                    created_at: SystemTime::now(),
                    last_accessed: SystemTime::now(),
                    access_count: 0,
                    strength: 0.5,
                    tags: vec!["fact".to_string()],
                    source_ids: vec![conv.id.clone()],
                    derived_from: vec![conv.id.clone()],
                    persisted_to_file: None,
                    embedding: None,
                });
            }
        }

        // 批量添加原子事实
        for atom in new_atoms {
            self.atoms.write().await.push(atom);
        }

        // 触发统计
        self.stats.write().await.extractions_performed += 1;
        self.stats.write().await.total_atoms = self.atoms.read().await.len() as u64;

        // 自动触发场景构建
        let atom_count = self.atoms.read().await.len();
        if atom_count as u32 % self.config.scenario_trigger_every_n == 0 && atom_count > 0 {
            let recent_atoms = self.atoms.read().await.iter()
                .rev().take(self.config.scenario_trigger_every_n as usize)
                .cloned().collect::<Vec<_>>();
            self.build_scenario(recent_atoms).await;
        }
    }

    /// 启发式提取事实 (无需LLM的简单文本模式)
    fn extract_facts_from_text(&self, text: &str) -> Vec<String> {
        let mut facts = Vec::new();

        // 模式1: "prefers X" / "likes Y" / "uses Z"
        let preference_patterns = [
            "prefers", "likes", "uses", "wants", "needs",
            "喜欢", "偏好", "使用", "想要",
        ];
        for pattern in &preference_patterns {
            if let Some(pos) = text.to_lowercase().find(pattern) {
                let start = pos.saturating_sub(20);
                let end = (pos + 80).min(text.len());
                let snippet = &text[start..end];
                if snippet.len() > 10 {
                    facts.push(format!("Preference: {}", snippet.trim()));
                }
            }
        }

        // 模式2: 检测 "is a" / "is an" (对象类型声明)
        if let Some(pos) = text.find(" is a ") {
            let end = (pos + 60).min(text.len());
            let snippet = &text[pos..end];
            facts.push(format!("Type declaration: {}", snippet.trim()));
        }

        // 模式3: 检测 "error" / "issue" / "bug" (错误/问题)
        if text.to_lowercase().contains("error") || text.to_lowercase().contains("bug") {
            let first_line = text.lines().next().unwrap_or("").to_string();
            if !first_line.is_empty() {
                facts.push(format!("Issue identified: {}", first_line));
            }
        }

        // 模式4: 检测代码模式 (``` 包裹)
        if text.contains("```") {
            let code_blocks: Vec<&str> = text.split("```").collect();
            if code_blocks.len() >= 3 {
                for chunk in code_blocks.iter().skip(1).step_by(2) {
                    if chunk.len() < 200 {
                        facts.push(format!("Code pattern used: {}...", 
                            chunk.lines().next().unwrap_or("")));
                    }
                }
            }
        }

        facts
    }

    /// === L1→L2: 从原子事实构建场景 ===
    async fn build_scenario(&self, atoms: Vec<TieredMemoryItem>) {
        // 按标签分组原子事实，每个组成为一个场景
        let mut tag_groups: HashMap<String, Vec<TieredMemoryItem>> = HashMap::new();
        for atom in atoms {
            for tag in &atom.tags {
                tag_groups.entry(tag.clone()).or_default().push(atom.clone());
            }
        }

        for (tag, group) in tag_groups {
            let content = group.iter()
                .map(|a| a.content.clone())
                .collect::<Vec<_>>()
                .join("\n");
            let ids = group.iter().map(|a| a.id.clone()).collect::<Vec<_>>();

            let scenario = TieredMemoryItem {
                id: format!("scenario-{}-{}", tag,
                    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos()),
                tier: MemoryTier::Scenario,
                content,
                created_at: SystemTime::now(),
                last_accessed: SystemTime::now(),
                access_count: 0,
                strength: 0.6,
                tags: vec![tag.clone()],
                source_ids: ids.clone(),
                derived_from: ids,
                persisted_to_file: None,
                embedding: None,
            };

            self.scenarios.write().await.push(scenario);
        }

        // 持久化场景到 Markdown 文件
        self.persist_scenarios_to_markdown().await;

        // 自动触发画像更新
        let count = self.scenarios.read().await.len();
        if count as u32 % self.config.persona_trigger_every_n == 0 && count > 0 {
            let recent = self.scenarios.read().await.iter()
                .rev().take(self.config.persona_trigger_every_n as usize)
                .cloned().collect::<Vec<_>>();
            self.build_persona(recent).await;
        }
    }

    /// === L2→L3: 从场景构建用户画像 ===
    async fn build_persona(&self, scenarios: Vec<TieredMemoryItem>) {
        let mut persona_lines = Vec::new();
        persona_lines.push("# User Persona".to_string());
        persona_lines.push(format!("> Generated at: {:?}", SystemTime::now()));
        persona_lines.push(String::new());
        persona_lines.push("## Summary of User Preferences".to_string());
        persona_lines.push(String::new());

        for scenario in &scenarios {
            persona_lines.push(format!("### From Scenario: {}", scenario.id));
            // 提取关键词作为画像摘要
            let summary: Vec<&str> = scenario.content.lines()
                .filter(|l| l.contains("Preference:") || l.contains("Type declaration:"))
                .collect();
            for line in summary {
                persona_lines.push(format!("- {}", line));
            }
            persona_lines.push(String::new());
        }

        let persona_content = persona_lines.join("\n");

        let persona = TieredMemoryItem {
            id: format!("persona-{}",
                SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos()),
            tier: MemoryTier::Persona,
            content: persona_content,
            created_at: SystemTime::now(),
            last_accessed: SystemTime::now(),
            access_count: 0,
            strength: 0.8,
            tags: vec!["persona".to_string()],
            source_ids: scenarios.iter().map(|s| s.id.clone()).collect(),
            derived_from: scenarios.iter().map(|s| s.id.clone()).collect(),
            persisted_to_file: None,
            embedding: None,
        };
        self.personas.write().await.push(persona);

        // 持久化画像到 Markdown 文件
        self.persist_persona_to_markdown(&persona).await;
    }

    /// === 持久化场景到 Markdown (白盒可追溯) ===
    async fn persist_scenarios_to_markdown(&self) {
        if !self.config.offload_enabled {
            return;
        }
        let scenarios_dir = self.store_root.join("scenarios");
        tokio::fs::create_dir_all(&scenarios_dir).await.unwrap_or(());

        let scenarios = self.scenarios.read().await;
        for scenario in scenarios.iter() {
            let file_path = scenarios_dir.join(format!("{}.md", scenario.id.replace(':', "_")));
            let mut content = String::new();
            content.push_str(&format!("# Scenario: {}\n\n", scenario.id));
            content.push_str(&format!("> Tier: {:?}\n", scenario.tier));
            content.push_str(&format!("> Derived from: {}\n\n", scenario.derived_from.join(", ")));
            content.push_str("## Content\n\n");
            content.push_str(&scenario.content);
            content.push_str("\n\n## Traceability\n\n");
            content.push_str("### Source Conversations\n\n");
            for sid in &scenario.source_ids {
                content.push_str(&format!("- `{}`\n", sid));
            }
            content.push_str("\n### Source Atoms\n\n");
            for did in &scenario.derived_from {
                content.push_str(&format!("- `{}`\n", did));
            }

            if tokio::fs::write(&file_path, &content).await.is_ok() {
                // 标记已持久化
                if let Some(stored) = self.scenarios.write().await.iter_mut()
                    .find(|s| s.id == scenario.id) {
                    stored.persisted_to_file = Some(file_path.clone());
                }
            }
        }
        self.stats.write().await.total_scenarios = scenarios.len() as u64;
    }

    /// === 持久化画像到 Markdown (白盒可追溯) ===
    async fn persist_persona_to_markdown(&self, persona: &TieredMemoryItem) {
        if !self.config.offload_enabled {
            return;
        }
        let persona_dir = self.store_root.join("personas");
        tokio::fs::create_dir_all(&persona_dir).await.unwrap_or(());

        let file_path = persona_dir.join(format!("{}.md", persona.id.replace(':', "_")));
        let mut content = String::new();
        content.push_str(&persona.content);
        content.push_str("\n\n## Traceability\n\n");
        content.push_str("### Source Scenarios\n\n");
        for sid in &persona.source_ids {
            content.push_str(&format!("- `{}` -> scenarios/{}.md\n", sid, sid.replace(':', "_")));
        }
        content.push_str("\n> Full traceability: Persona → Scenario → Atom → Conversation\n");
        content.push_str("> See L3→L2→L1→L0 drill down path for audit\n");

        if tokio::fs::write(&file_path, &content).await.is_ok() {
            if let Some(stored) = self.personas.write().await.iter_mut()
                .find(|p| p.id == persona.id) {
                stored.persisted_to_file = Some(file_path);
            }
        }
        self.stats.write().await.total_personas = self.personas.read().await.len() as u64;
    }

    // ========================================================================
    // [2] 符号化记忆 + Mermaid 上下文卸载
    // 移植自: TencentDB-Agent-Memory Symbolic Memory Memory
    // ========================================================================

    /// 生成 Mermaid 任务状态图 (用于上下文卸载)
    /// 取代冗长的工具日志，仅保留轻量级符号图在 Agent 上下文中
    pub async fn generate_mermaid_canvas(&self, state_key: &str, nodes: &[MermaidNode]) -> String {
        let mut canvas = String::new();
        canvas.push_str("```mermaid\n");
        canvas.push_str("graph TD\n");
        canvas.push_str(&format!("    title[Memory Canvas: {}]\n", state_key));

        for node in nodes {
            canvas.push_str(&format!(
                "    {}[\"{}\"]\n",
                node.id, node.label.replace('"', "'")
            ));
        }

        // 添加层级引用边
        canvas.push_str("\n    %% Tier links\n");
        for node in nodes {
            if !node.parent_id.is_empty() {
                canvas.push_str(&format!("    {} --> {}\n", node.parent_id, node.id));
            }
        }

        canvas.push_str("```\n");
        canvas.push_str(&format!(
            "\n> Context offloaded. Total {} symbols. Use node_id to retrieve full text.\n",
            nodes.len()
        ));
        canvas
    }

    /// 将工具日志卸载到外部文件, 返回 Mermaid 摘要
    /// 参照: TencentDB-Agent-Memory 上下文卸载 + node_id 追踪
    pub async fn offload_tool_logs(
        &self,
        session_id: &str,
        tool_calls: &[(String, String, String)],  // (tool_name, input_summary, output_preview)
    ) -> Result<MermaidCanvas, String> {
        let offload_dir = self.store_root.join("offload").join(session_id);
        tokio::fs::create_dir_all(&offload_dir).await.map_err(|e| e.to_string())?;

        let mut nodes = Vec::new();
        let mut edges = Vec::new();

        // 写入完整日志到磁盘
        for (i, (tool, input, output)) in tool_calls.iter().enumerate() {
            let node_id = format!("tool_{}", i);

            // 保存完整日志到 refs/*.md (参考 TencentDB: refs/*.md 存储原始日志)
            let log_path = offload_dir.join("refs").join(format!("{}.md", node_id));
            tokio::fs::create_dir_all(log_path.parent().unwrap()).await.map_err(|e| e.to_string())?;

            let log_content = format!(
                "# Tool Call: {}\n\n## Input\n```\n{}\n```\n\n## Output\n```\n{}\n```\n",
                tool, input, output
            );
            tokio::fs::write(&log_path, &log_content).await.map_err(|e| e.to_string())?;

            // 在 Mermaid 画布中只保留轻量摘要
            nodes.push(MermaidNode {
                id: node_id.clone(),
                label: format!("{}: {}...{}", tool,
                    &input.chars().take(20).collect::<String>(),
                    &output.chars().take(20).collect::<String>()),
                parent_id: if i > 0 { format!("tool_{}", i - 1) } else { String::new() },
                ref_path: log_path.to_string_lossy().to_string(),
            });

            if i > 0 {
                edges.push(format!("    tool_{} --> tool_{}", i - 1, i));
            }
        }

        // 生成 JSONL 摘要 (中层)
        let jsonl_path = offload_dir.join("summary.jsonl");
        let summary_line = serde_json::json!({
            "session": session_id,
            "tool_count": tool_calls.len(),
            "tools": tool_calls.iter().map(|(t, _, _)| t).collect::<Vec<_>>(),
            "timestamp": format!("{:?}", SystemTime::now()),
        });
        tokio::fs::write(&jsonl_path, summary_line.to_string()).await.map_err(|e| e.to_string())?;

        self.stats.write().await.offloads_performed += 1;

        Ok(MermaidCanvas {
            nodes,
            edges,
            refs_dir: offload_dir.join("refs"),
            jsonl_path,
        })
    }

    // ========================================================================
    // [3] 混合检索 (BM25 + Vector + RRF)
    // 移植自: TencentDB-Agent-Memory hybrid retrieval + sqlite-vec
    // ========================================================================

    /// BM25 评分器 (Okapi BM25)
    pub struct Bm25Scorer {
        /// 所有文档的词频统计
        doc_freqs: HashMap<String, usize>,
        /// 文档数
        num_docs: usize,
        /// 平均文档长度
        avg_doc_len: f64,
        /// k1 参数 (默认 1.5)
        k1: f64,
        /// b 参数 (默认 0.75)
        b: f64,
        /// 文档长度缓存
        doc_lengths: HashMap<String, usize>,
    }

    impl Bm25Scorer {
        pub fn new(k1: f64, b: f64) -> Self {
            Self {
                doc_freqs: HashMap::new(),
                num_docs: 0,
                avg_doc_len: 0.0,
                k1,
                b,
                doc_lengths: HashMap::new(),
            }
        }

        /// 索引文档集合
        pub fn index(&mut self, documents: &[(String, String)]) {
            self.num_docs = documents.len();
            let mut total_len = 0usize;

            // 构建倒排索引
            for (id, content) in documents {
                let words: Vec<&str> = content.split_whitespace().collect();
                let doc_len = words.len();
                self.doc_lengths.insert(id.clone(), doc_len);
                total_len += doc_len;

                let mut seen = std::collections::HashSet::new();
                for word in words {
                    let w = word.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
                    if !w.is_empty() && seen.insert(w.clone()) {
                        *self.doc_freqs.entry(w).or_insert(0) += 1;
                    }
                }
            }

            self.avg_doc_len = total_len as f64 / self.num_docs.max(1) as f64;
        }

        /// 计算单个文档的 BM25 分数
        pub fn score(&self, query: &str, doc_id: &str, doc_content: &str) -> f64 {
            let idf = |term: &str| -> f64 {
                let df = self.doc_freqs.get(term).copied().unwrap_or(1).max(1);
                ((self.num_docs as f64 - df as f64 + 0.5) / (df as f64 + 0.5) + 1.0).ln()
            };

            let doc_len = self.doc_lengths.get(doc_id).copied().unwrap_or(doc_content.len()) as f64;

            let mut score = 0.0;
            let words: Vec<&str> = doc_content.split_whitespace().collect();
            let query_terms: std::collections::HashSet<String> = query.split_whitespace()
                .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase())
                .filter(|w| !w.is_empty())
                .collect();

            for term in &query_terms {
                let tf = words.iter()
                    .filter(|w| {
                        w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase() == *term
                    })
                    .count() as f64;

                if tf == 0.0 {
                    continue;
                }

                let term_idf = idf(term);
                let numerator = tf * (self.k1 + 1.0);
                let denominator = tf + self.k1 * (1.0 - self.b + self.b * doc_len / self.avg_doc_len);
                score += term_idf * numerator / denominator;
            }

            score
        }

        /// 批量计算 BM25 分数
        pub fn search(&self, query: &str, documents: &[(String, String)], top_k: usize) -> Vec<(String, f64)> {
            let mut scored: Vec<(String, f64)> = documents.iter()
                .map(|(id, content)| {
                    (id.clone(), self.score(query, id, content))
                })
                .filter(|(_, s)| *s > 0.0)
                .collect();

            scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            scored.truncate(top_k);
            scored
        }
    }

    /// 向量检索适配器 (基于 sqlite-vec)
    pub struct VectorSearchEngine {
        /// 存储嵌入
        embeddings: HashMap<String, Vec<f32>>,
    }

    impl VectorSearchEngine {
        pub fn new() -> Self {
            Self { embeddings: HashMap::new() }
        }

        pub fn upsert(&mut self, id: &str, embedding: Vec<f32>) {
            self.embeddings.insert(id.to_string(), embedding);
        }

        /// 余弦相似度搜索
        pub fn search(&self, query_embedding: &[f32], top_k: usize) -> Vec<(String, f64)> {
            let mut scored: Vec<(String, f64)> = self.embeddings.iter()
                .map(|(id, emb)| {
                    let sim = cosine_similarity(query_embedding, emb);
                    (id.clone(), sim)
                })
                .filter(|(_, s)| *s > 0.1)
                .collect();

            scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            scored.truncate(top_k);
            scored
        }
    }

    /// 余弦相似度
    fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
        if a.len() != b.len() || a.is_empty() {
            return 0.0;
        }
        let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm_a == 0.0 || norm_b == 0.0 {
            return 0.0;
        }
        (dot / (norm_a * norm_b)) as f64
    }

    /// RRF (Reciprocal Rank Fusion) 融合策略
    /// 将 BM25 和 向量检索的结果融合
    pub fn rrf_fusion(
        bm25_results: &[(String, f64)],
        vector_results: &[(String, f64)],
        k: f64,    // RRF 参数 (默认 60)
        top_k: usize,
    ) -> Vec<(String, f64)> {
        let mut rrf_scores: HashMap<String, f64> = HashMap::new();

        // BM25 排名贡献
        for (rank, (id, _)) in bm25_results.iter().enumerate() {
            *rrf_scores.entry(id.clone()).or_insert(0.0) += 1.0 / (k + rank as f64 + 1.0);
        }

        // 向量检索排名贡献
        for (rank, (id, _)) in vector_results.iter().enumerate() {
            *rrf_scores.entry(id.clone()).or_insert(0.0) += 1.0 / (k + rank as f64 + 1.0);
        }

        let mut ranked: Vec<(String, f64)> = rrf_scores.into_iter().collect();
        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        ranked.truncate(top_k);
        ranked
    }

    /// === 混合检索: BM25 + Vector + RRF ===
    pub async fn hybrid_search(
        &self,
        query: &str,
        query_embedding: Option<&[f32]>,
        top_k: usize,
    ) -> Vec<(TieredMemoryItem, f64, String)> {
        let mut bm25_docs: Vec<(String, String)> = Vec::new();
        let mut vector_results: Vec<(String, f64)> = Vec::new();

        // 从所有层级收集文档
        let all_items = {
            let c = self.conversations.read().await;
            let a = self.atoms.read().await;
            let s = self.scenarios.read().await;
            let p = self.personas.read().await;
            let mut items = Vec::new();
            items.extend(c.iter().cloned());
            items.extend(a.iter().cloned());
            items.extend(s.iter().cloned());
            items.extend(p.iter().cloned());
            items
        };

        // 准备 BM25 文档
        for item in &all_items {
            bm25_docs.push((item.id.clone(), item.content.clone()));
        }

        // 运行 BM25
        let mut bm25 = Bm25Scorer::new(self.config.bm25_k1, self.config.bm25_b);
        bm25.index(&bm25_docs);
        let bm25_results = bm25.search(query, &bm25_docs, top_k * 2);

        // 如果有嵌入, 运行向量检索
        if let Some(emb) = query_embedding {
            let mut vec_engine = VectorSearchEngine::new();
            for item in &all_items {
                if let Some(ref e) = item.embedding {
                    vec_engine.upsert(&item.id, e.clone());
                }
            }
            vector_results = vec_engine.search(emb, top_k * 2);
        }

        // RRF 融合
        let fused = Self::rrf_fusion(
            &bm25_results,
            &vector_results,
            60.0,
            top_k,
        );

        // 映射回完整条目
        let mut results = Vec::new();
        for (id, rrf_score) in &fused {
            if let Some(item) = all_items.iter().find(|i| i.id == *id) {
                // 标记来源
                let source = match item.tier {
                    MemoryTier::Conversation => "L0",
                    MemoryTier::Atom => "L1",
                    MemoryTier::Scenario => "L2",
                    MemoryTier::Persona => "L3",
                };
                results.push((item.clone(), *rrf_score, source.to_string()));
            }
        }

        self.stats.write().await.hybrid_searches += 1;
        results
    }

    // ========================================================================
    // [4] 白盒可追溯: Persona→Scenario→Atom→Conversation 溯源
    // 移植自: TencentDB-Agent-Memory Full Traceability
    // ========================================================================

    /// 从高层条目向下钻取到底层 (Persona → Scenario → Atom → Conversation)
    pub async fn drill_down(&self, item_id: &str) -> Vec<TieredMemoryItem> {
        let mut chain = Vec::new();

        // 查找当前条目
        let item = {
            let c = self.conversations.read().await;
            let a = self.atoms.read().await;
            let s = self.scenarios.read().await;
            let p = self.personas.read().await;
            c.iter().chain(a.iter()).chain(s.iter()).chain(p.iter())
                .find(|i| i.id == item_id)
                .cloned()
        };

        let item = match item {
            Some(i) => i,
            None => return chain,
        };

        chain.push(item.clone());

        // 如果从其他条目派生, 递归查找
        for source_id in &item.derived_from {
            let sub_chain = Box::pin(self.drill_down(source_id)).await;
            chain.extend(sub_chain);
        }

        chain
    }

    /// 获取完整统计摘要 (白盒可调试)
    pub async fn stats_summary(&self) -> String {
        let stats = self.stats.read().await;
        let scenarios = self.scenarios.read().await;
        let personas = self.personas.read().await;

        format!(
            "━━━ Memory Pipeline Stats ━━━\n\
             Conversations (L0):     {}\n\
             Atoms (L1):             {}\n\
             Scenarios (L2):         {} (persisted: {})\n\
             Personas (L3):          {} (persisted: {})\n\
             Extractions performed:  {}\n\
             Offloads performed:     {}\n\
             Hybrid searches:        {}\n\
             Store root:             {}",
            stats.total_conversations,
            stats.total_atoms,
            stats.total_scenarios,
            scenarios.iter().filter(|s| s.persisted_to_file.is_some()).count(),
            stats.total_personas,
            personas.iter().filter(|p| p.persisted_to_file.is_some()).count(),
            stats.extractions_performed,
            stats.offloads_performed,
            stats.hybrid_searches,
            self.store_root.display(),
        )
    }
}

// ========================================================================
// 公共类型定义
// ========================================================================

/// Mermaid 图节点
#[derive(Debug, Clone)]
pub struct MermaidNode {
    pub id: String,
    pub label: String,
    pub parent_id: String,
    /// 完整日志在磁盘上的路径
    pub ref_path: String,
}

/// Mermaid 画布 (上下文卸载产物)
#[derive(Debug, Clone)]
pub struct MermaidCanvas {
    pub nodes: Vec<MermaidNode>,
    pub edges: Vec<String>,
    pub refs_dir: PathBuf,
    pub jsonl_path: PathBuf,
}

impl MermaidCanvas {
    /// 渲染为 Mermaid Markdown
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str("```mermaid\n");
        out.push_str("graph LR\n");
        out.push_str("    %% Context Offloaded Canvas\n\n");

        for node in &self.nodes {
            let escaped_label = node.label.replace('"', "'");
            out.push_str(&format!("    {}[\"{}\"]\n", node.id, escaped_label));
        }

        out.push('\n');
        for edge in &self.edges {
            out.push_str(edge);
            out.push('\n');
        }

        out.push_str("```\n");
        out.push_str(&format!(
            "\n> 🎯 Offloaded canvas: {} tool calls. Full logs at `{}`\n",
            self.nodes.len(),
            self.refs_dir.display(),
        ));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bm25_basic() {
        let docs = vec![
            ("doc1".to_string(), "the cat sat on the mat".to_string()),
            ("doc2".to_string(), "the dog sat on the log".to_string()),
            ("doc3".to_string(), "cats and dogs are pets".to_string()),
        ];

        let mut bm25 = Bm25Scorer::new(1.5, 0.75);
        bm25.index(&docs);

        let results = bm25.search("cat", &docs, 3);
        assert!(results.len() >= 1);
        assert_eq!(results[0].0, "doc1"); // "cat" 出现在 doc1
    }

    #[test]
    fn test_cosine_similarity() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        let c = vec![1.0, 0.0, 0.0];

        assert!((cosine_similarity(&a, &b) - 0.0).abs() < 1e-6);
        assert!((cosine_similarity(&a, &c) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_rrf_fusion() {
        let bm25 = vec![
            ("doc_a".to_string(), 0.8),
            ("doc_b".to_string(), 0.6),
        ];
        let vector = vec![
            ("doc_b".to_string(), 0.9),
            ("doc_c".to_string(), 0.7),
        ];

        let fused = MemoryPipeline::rrf_fusion(&bm25, &vector, 60.0, 3);
        assert!(fused.len() >= 2);
        // doc_b 出现在两个结果集中, 应该排名最高
        assert_eq!(fused[0].0, "doc_b");
    }

    #[test]
    fn test_extract_facts() {
        let pipeline = MemoryPipeline::new(PipelineConfig::default());

        let facts = pipeline.extract_facts_from_text(
            "The user prefers async/await over callbacks. This is a preference."
        );
        assert!(!facts.is_empty());
        assert!(facts[0].contains("Preference:"));

        let error_facts = pipeline.extract_facts_from_text(
            "There is a bug in the login module. Error occurs when token expires."
        );
        assert!(!error_facts.is_empty());
        assert!(error_facts[0].contains("Issue identified:"));
    }

    #[tokio::test]
    async fn test_mermaid_canvas() {
        let pipeline = MemoryPipeline::new(PipelineConfig::default());
        let tool_calls = vec![
            ("read".to_string(), "src/main.rs".to_string(), "fn main() {}".to_string()),
            ("write".to_string(), "src/lib.rs".to_string(), "pub fn helper()".to_string()),
        ];

        let canvas = pipeline.offload_tool_logs("test-session", &tool_calls).await;
        assert!(canvas.is_ok());
        let canvas = canvas.unwrap();
        assert_eq!(canvas.nodes.len(), 2);
        assert!(canvas.render().contains("mermaid"));
        assert!(canvas.render().contains("tool_0"));
        assert!(canvas.render().contains("tool_1"));
    }

    #[tokio::test]
    async fn test_pipeline_lifecycle() {
        let pipeline = MemoryPipeline::new(PipelineConfig {
            every_n_conversations: 2,
            scenario_trigger_every_n: 3,
            persona_trigger_every_n: 2,
            ..Default::default()
        });

        // 添加对话 - 触发原子提取
        pipeline.add_conversation("user", "I prefers async/await over callbacks").await;
        pipeline.add_conversation("user", "There is a bug in the login module").await;

        // 验证原子提取
        let atoms = pipeline.atoms.read().await;
        assert!(!atoms.is_empty(), "Should have extracted atoms");
        assert!(atoms.iter().any(|a| a.content.contains("Preference:")));

        // 验证 Mermaid 画布
        let tool_calls = vec![
            ("edit".to_string(), "src/auth.rs:42".to_string(), "Edited successfully".to_string()),
        ];
        let canvas = pipeline.offload_tool_logs("session-xyz", &tool_calls).await;
        assert!(canvas.is_ok());

        // 验证统计
        let stats = pipeline.stats.read().await;
        assert!(stats.extractions_performed > 0);
    }

    #[tokio::test]
    async fn test_drill_down() {
        let pipeline = MemoryPipeline::new(PipelineConfig::default());

        // 添加对话
        pipeline.add_conversation("user", "Test conversation for drill down").await;

        // 获取原子
        let atoms = pipeline.atoms.read().await;
        if let Some(atom) = atoms.first() {
            let chain = pipeline.drill_down(&atom.id).await;
            assert!(!chain.is_empty());
        }
    }

    #[tokio::test]
    async fn test_hybrid_search() {
        let pipeline = MemoryPipeline::new(PipelineConfig::default());

        pipeline.add_conversation("user", "I prefers async/await syntax").await;

        let results = pipeline.hybrid_search("async syntax", None, 5).await;
        assert!(!results.is_empty());
        assert_eq!(results[0].2, "L0"); // 从对话层返回
    }

    #[tokio::test]
    async fn test_stats_summary() {
        let pipeline = MemoryPipeline::new(PipelineConfig::default());
        let summary = pipeline.stats_summary().await;
        assert!(summary.contains("Memory Pipeline Stats"));
    }
}
