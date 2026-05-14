//! Symbol Index - 多层级符号索引系统
//!
//! 提供快速的符号查找能力：
//! - 倒排索引: symbol_name -> [(file, location)]
//! - 前缀索引 (Trie): 用于模糊搜索
//! - 类型分类索引: 按符号类型组织
//!
//! 特性：
//! - O(1) 精确匹配
//! - O(k) 前缀搜索 (k = prefix 长度)
//! - 支持编辑距离模糊匹配
//! - 自动从 AST 构建索引

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

/// 符号类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SymbolKind {
    /// 函数
    Function,
    /// 方法
    Method,
    /// 结构体/类
    Struct,
    /// 枚举
    Enum,
    /// Trait/接口
    Trait,
    /// 接口
    Interface,
    /// 变量
    Variable,
    /// 常量
    Constant,
    /// 参数
    Parameter,
    /// 类型别名
    TypeAlias,
    /// 模块
    Module,
    /// 字段
    Field,
    /// 属性
    Property,
    /// 未知
    Unknown,
}

impl std::fmt::Display for SymbolKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Function => write!(f, "function"),
            Self::Method => write!(f, "method"),
            Self::Struct => write!(f, "struct"),
            Self::Enum => write!(f, "enum"),
            Self::Trait => write!(f, "trait"),
            Self::Interface => write!(f, "interface"),
            Self::Variable => write!(f, "variable"),
            Self::Constant => write!(f, "constant"),
            Self::Parameter => write!(f, "parameter"),
            Self::TypeAlias => write!(f, "type_alias"),
            Self::Module => write!(f, "module"),
            Self::Field => write!(f, "field"),
            Self::Property => write!(f, "property"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

/// 符号位置信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolLocation {
    /// 文件路径
    pub file_path: PathBuf,
    
    /// 行号 (0-based)
    pub line: usize,
    
    /// 列号 (0-based)
    pub column: usize,
    
    /// 符号类型
    pub kind: SymbolKind,
    
    /// 所属作用域 (namespace/class/module)
    pub scope: String,
    
    /// 函数签名 (仅对函数/方法有效)
    pub signature: Option<String>,
}

impl SymbolLocation {
    pub fn new(
        file_path: impl Into<PathBuf>,
        line: usize,
        column: usize,
        kind: SymbolKind,
    ) -> Self {
        Self {
            file_path: file_path.into(),
            line,
            column,
            kind,
            scope: String::new(),
            signature: None,
        }
    }

    pub fn with_scope(mut self, scope: impl Into<String>) -> Self {
        self.scope = scope.into();
        self
    }

    pub fn with_signature(mut self, sig: impl Into<String>) -> Self {
        self.signature = Some(sig.into());
        self
    }
}

/// Trie 节点 (前缀树)
#[derive(Debug, Clone, Default)]
struct TrieNode {
    children: HashMap<char, TrieNode>,
    is_end_of_word: bool,
    symbol_ids: Vec<usize>, // 存储指向 inverted_index 的 ID
}

/// 符号索引配置
#[derive(Debug, Clone)]
pub struct SymbolIndexConfig {
    /// 是否启用模糊搜索
    pub enable_fuzzy_search: bool,
    
    /// 最大编辑距离 (用于模糊搜索)
    pub max_edit_distance: usize,
    
    /// 索引构建时的并行度
    pub parallelism: usize,
    
    /// 缓存大小
    pub cache_size: usize,
}

impl Default for SymbolIndexConfig {
    fn default() -> Self {
        Self {
            enable_fuzzy_search: true,
            max_edit_distance: 2,
            parallelism: 4,
            cache_size: 100_000,
        }
    }
}

/// 符号索引系统
pub struct SymbolIndex {
    /// 倒排索引: symbol_name (lowercase) -> [SymbolLocation]
    inverted_index: Arc<RwLock<HashMap<String, Vec<SymbolLocation>>>>,
    
    /// 前缀索引 (Trie)
    prefix_trie: Arc<RwLock<TrieNode>>,
    
    /// 类型分类索引: SymbolKind -> [symbol_name]
    type_index: Arc<RwLock<HashMap<SymbolKind, HashSet<String>>>>,
    
    /// 文件级索引: file_path -> [symbol_name]
    file_index: Arc<RwLock<HashMap<PathBuf, HashSet<String>>>>,
    
    /// 配置
    config: SymbolIndexConfig,
    
    /// 统计信息
    stats: Arc<RwLock<SymbolIndexStats>>,
}

/// 统计信息
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SymbolIndexStats {
    /// 总符号数
    pub total_symbols: usize,
    /// 总文件数
    pub total_files: usize,
    /// 各类型的符号数量
    pub symbols_by_type: HashMap<String, usize>,
    /// 平均每个文件的符号数
    pub avg_symbols_per_file: f64,
}

impl SymbolIndex {
    /// 创建新的符号索引
    pub fn new(config: SymbolIndexConfig) -> Self {
        Self {
            inverted_index: Arc::new(RwLock::new(HashMap::new())),
            prefix_trie: Arc::new(RwLock::new(TrieNode::default())),
            type_index: Arc::new(RwLock::new(HashMap::new())),
            file_index: Arc::new(RwLock::new(HashMap::new())),
            config,
            stats: Arc::new(RwLock::new(SymbolIndexStats::default())),
        }
    }

    /// 使用默认配置创建
    pub fn with_defaults() -> Self {
        Self::new(SymbolIndexConfig::default())
    }

    /// 添加单个符号到索引
    pub fn add_symbol(&self, name: &str, location: SymbolLocation) {
        let name_lower = name.to_lowercase();

        // 更新倒排索引
        {
            let mut index = self.inverted_index.write();
            index.entry(name_lower.clone())
                .or_insert_with(Vec::new)
                .push(location.clone());
        }

        // 更新前缀索引 (Trie)
        {
            let mut trie = self.prefix_trie.write();
            self.insert_into_trie(&mut trie, &name_lower);
        }

        // 更新类型索引
        {
            let mut type_idx = self.type_index.write();
            type_idx.entry(location.kind)
                .or_insert_with(HashSet::new)
                .insert(name_lower.clone());
        }

        // 更新文件索引
        {
            let mut file_idx = self.file_index.write();
            file_idx.entry(location.file_path.clone())
                .or_insert_with(HashSet::new)
                .insert(name_lower);
        }

        // 更新统计
        {
            let mut stats = self.stats.write();
            stats.total_symbols += 1;
            *stats.symbols_by_type
                .entry(format!("{}", location.kind))
                .or_insert(0) += 1;
        }

        debug!(symbol = %name, kind = %location.kind, file = %location.file_path.display(), "Symbol added to index");
    }

    /// 批量添加符号
    pub fn add_symbols_batch(&self, symbols: Vec<(String, SymbolLocation)>) {
        let count = symbols.len();
        for (name, location) in symbols {
            self.add_symbol(&name, location);
        }

        info!(count = count, "Batch of symbols added");
    }

    /// 精确查找符号
    pub fn exact_search(&self, query: &str) -> Vec<SymbolLocation> {
        let query_lower = query.to_lowercase();
        
        let index = self.inverted_index.read();
        match index.get(&query_lower) {
            Some(locations) => locations.clone(),
            None => Vec::new(),
        }
    }

    /// 前缀搜索 (模糊匹配)
    pub fn prefix_search(&self, query: &str, limit: usize) -> Vec<SymbolLocation> {
        let query_lower = query.to_lowercase();
        let trie = self.prefix_trie.read();
        
        // 在 Trie 中找到所有以 query 为前缀的节点
        let matching_names = self.collect_prefix_matches(&trie, &query_lower);
        
        // 从倒排索引中获取位置信息
        let index = self.inverted_index.read();
        let mut results: Vec<SymbolLocation> = matching_names
            .iter()
            .filter_map(|name| index.get(name))
            .flatten()
            .cloned()
            .collect();
        
        // 如果结果不足，尝试模糊搜索
        if results.len() < limit && self.config.enable_fuzzy_search {
            let fuzzy_results = self.fuzzy_search(query, limit - results.len());
            results.extend(fuzzy_results);
        }
        
        // 去重并限制数量
        results.sort_by(|a, b| a.file_path.cmp(&b.file_path).then(a.line.cmp(&b.line)));
        results.dedup_by(|a, b| a.file_path == b.file_path && a.line == b.line);
        results.into_iter().take(limit).collect()
    }

    /// 编辑距离模糊搜索
    pub fn fuzzy_search(&self, query: &str, limit: usize) -> Vec<SymbolLocation> {
        if !self.config.enable_fuzzy_search {
            return Vec::new();
        }

        let query_lower = query.to_lowercase();
        let max_dist = self.config.max_edit_distance;

        let index = self.inverted_index.read();

        let mut candidates: Vec<(String, SymbolLocation, usize)> = index
            .iter()
            .filter(|(name, _)| {
                levenshtein_distance(name, &query_lower) <= max_dist
            })
            .flat_map(|(name, locations)| {
                let name_clone = name.clone();
                let dist = levenshtein_distance(name, &query_lower);
                locations.iter().cloned().map(move |loc| {
                    (name_clone.clone(), loc, dist)
                })
            })
            .collect();

        candidates.sort_by_key(|(_, _, dist)| *dist);

        candidates
            .into_iter()
            .map(|(_, loc, _)| loc)
            .take(limit)
            .collect()
    }

    /// 按类型查找符号
    pub fn search_by_type(&self, kind: SymbolKind, limit: usize) -> Vec<SymbolLocation> {
        let type_idx = self.type_index.read();
        let names = match type_idx.get(&kind) {
            Some(names) => names,
            None => return Vec::new(),
        };
        
        let index = self.inverted_index.read();
        let mut results: Vec<SymbolLocation> = names
            .iter()
            .filter_map(|name| index.get(name))
            .flatten()
            .cloned()
            .collect();
        
        results.sort_by(|a, b| a.file_path.cmp(&b.file_path));
        results.into_iter().take(limit).collect()
    }

    /// 获取文件中的所有符号
    pub fn get_symbols_in_file(&self, file_path: &Path) -> Vec<SymbolLocation> {
        let file_idx = self.file_index.read();
        let names = match file_idx.get(file_path) {
            Some(names) => names,
            None => return Vec::new(),
        };
        
        let index = self.inverted_index.read();
        names
            .iter()
            .filter_map(|name| index.get(name))
            .flatten()
            .filter(|loc| loc.file_path == file_path)
            .cloned()
            .collect()
    }

    /// 获取统计信息
    pub fn get_stats(&self) -> SymbolIndexStats {
        let stats = self.stats.read().clone();
        let file_count = self.file_index.read().len();
        
        SymbolIndexStats {
            total_files: file_count,
            avg_symbols_per_file: if file_count > 0 {
                stats.total_symbols as f64 / file_count as f64
            } else {
                0.0
            },
            ..stats
        }
    }

    /// 清空所有索引
    pub fn clear(&self) {
        self.inverted_index.write().clear();
        self.prefix_trie.write().children.clear();
        self.type_index.write().clear();
        self.file_index.write().clear();
        *self.stats.write() = SymbolIndexStats::default();
        
        info!("Symbol index cleared");
    }

    // === 内部辅助方法 ===

    /// 插入字符串到 Trie
    fn insert_into_trie(&self, node: &mut TrieNode, s: &str) {
        let mut current = node;
        for ch in s.chars() {
            current = current.children.entry(ch).or_insert_with(|| TrieNode::default());
        }
        current.is_end_of_word = true;
    }

    /// 收集 Trie 中所有以 prefix 为前缀的字符串
    fn collect_prefix_matches(&self, node: &TrieNode, prefix: &str) -> Vec<String> {
        let mut results = Vec::new();
        
        // 找到前缀对应的节点
        let mut current = node;
        for ch in prefix.chars() {
            match current.children.get(&ch) {
                Some(child) => current = child,
                None => return results, // 前缀不存在
            }
        }
        
        // DFS 收集所有后续单词
        self.dfs_collect(current, prefix.to_string(), &mut results);
        
        results
    }

    /// DFS 遍历 Trie 收集完整字符串
    fn dfs_collect(&self, node: &TrieNode, current_prefix: String, results: &mut Vec<String>) {
        if node.is_end_of_word {
            results.push(current_prefix.clone());
        }
        
        for (ch, child) in &node.children {
            let mut new_prefix = current_prefix.clone();
            new_prefix.push(*ch);
            self.dfs_collect(child, new_prefix, results);
        }
    }
}

/// 计算两个字符串的 Levenshtein 编辑距离
fn levenshtein_distance(s1: &str, s2: &str) -> usize {
    let chars1: Vec<char> = s1.chars().collect();
    let chars2: Vec<char> = s2.chars().collect();
    let len1 = chars1.len();
    let len2 = chars2.len();
    
    if len1 == 0 { return len2; }
    if len2 == 0 { return len1; }
    
    let mut matrix = vec![vec![0usize; len2 + 1]; len1 + 1];
    
    // 初始化第一行和第一列
    for i in 0..=len1 {
        matrix[i][0] = i;
    }
    for j in 0..=len2 {
        matrix[0][j] = j;
    }
    
    // 填充矩阵
    for (i, c1) in chars1.iter().enumerate() {
        for (j, c2) in chars2.iter().enumerate() {
            let cost = if c1 == c2 { 0 } else { 1 };
            
            matrix[i + 1][j + 1] = (matrix[i][j + 1] + 1)      // deletion
                .min(matrix[i + 1][j] + 1)       // insertion
                .min(matrix[i][j] + cost);         // substitution
        }
    }
    
    matrix[len1][len2]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_and_exact_search() {
        let idx = SymbolIndex::with_defaults();
        
        let loc = SymbolLocation::new("src/main.rs", 10, 5, SymbolKind::Function);
        idx.add_symbol("main", loc);
        
        let results = idx.exact_search("main");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].line, 10);
    }

    #[test]
    fn test_prefix_search() {
        let idx = SymbolIndex::with_defaults();
        
        idx.add_symbol("calculate_fibonacci", SymbolLocation::new("src/math.rs", 5, 0, SymbolKind::Function));
        idx.add_symbol("calculate_factorial", SymbolLocation::new("src/math.rs", 20, 0, SymbolKind::Function));
        idx.add_symbol("process_data", SymbolLocation::new("src/utils.rs", 15, 0, SymbolKind::Function));
        
        let results = idx.prefix_search("calc", 10);
        assert_eq!(results.len(), 2); // calculate_fibonacci 和 calculate_factorial
    }

    #[test]
    fn test_fuzzy_search() {
        let idx = SymbolIndex::with_defaults();
        
        idx.add_symbol("my_function", SymbolLocation::new("a.rs", 1, 0, SymbolKind::Function));
        idx.add_symbol("your_func", SymbolLocation::new("b.rs", 2, 0, SymbolKind::Function));
        
        let results = idx.fuzzy_search("me_functon", 5); // 有一个 typo
        assert_eq!(results.len(), 1); // 应该匹配 my_function
        assert_eq!(results[0].line, 1);
    }

    #[test]
    fn test_search_by_type() {
        let idx = SymbolIndex::with_defaults();
        
        idx.add_symbol("MyStruct", SymbolLocation::new("a.rs", 1, 0, SymbolKind::Struct));
        idx.add_symbol("my_func", SymbolLocation::new("a.rs", 5, 0, SymbolKind::Function));
        idx.add_symbol("AnotherStruct", SymbolLocation::new("b.rs", 3, 0, SymbolKind::Struct));
        
        let structs = idx.search_by_type(SymbolKind::Struct, 10);
        assert_eq!(structs.len(), 2);
        
        let funcs = idx.search_by_type(SymbolKind::Function, 10);
        assert_eq!(funcs.len(), 1);
    }

    #[test]
    fn test_get_symbols_in_file() {
        let idx = SymbolIndex::with_defaults();
        
        idx.add_symbol("func_a", SymbolLocation::new("src/main.rs", 1, 0, SymbolKind::Function));
        idx.add_symbol("func_b", SymbolLocation::new("src/main.rs", 10, 0, SymbolKind::Function));
        idx.add_symbol("func_c", SymbolLocation::new("src/lib.rs", 5, 0, SymbolKind::Function));
        
        let main_symbols = idx.get_symbols_in_file(Path::new("src/main.rs"));
        assert_eq!(main_symbols.len(), 2);
        
        let lib_symbols = idx.get_symbols_in_file(Path::new("src/lib.rs"));
        assert_eq!(lib_symbols.len(), 1);
    }

    #[test]
    fn test_levenshtein_distance() {
        assert_eq!(levenshtein_distance("", ""), 0);
        assert_eq!(levenshtein_distance("abc", "abc"), 0);
        assert_eq!(levenshtein_distance("abc", "ab"), 1); // deletion
        assert_eq!(levenshtein_distance("ab", "abc"), 1); // insertion
        assert_eq!(levenshtein_distance("kitten", "sitting"), 3); // classic example
    }

    #[test]
    fn test_stats() {
        let idx = SymbolIndex::with_defaults();
        
        idx.add_symbol("f1", SymbolLocation::new("a.rs", 1, 0, SymbolKind::Function));
        idx.add_symbol("f2", SymbolLocation::new("a.rs", 2, 0, SymbolKind::Function));
        idx.add_symbol("S1", SymbolLocation::new("a.rs", 5, 0, SymbolKind::Struct));
        idx.add_symbol("f3", SymbolLocation::new("b.rs", 1, 0, SymbolKind::Function));
        
        let stats = idx.get_stats();
        assert_eq!(stats.total_symbols, 4);
        assert_eq!(stats.total_files, 2);
        assert!((stats.avg_symbols_per_file - 2.0).abs() < 0.01);
    }

    #[test]
    fn test_clear() {
        let idx = SymbolIndex::with_defaults();
        
        idx.add_symbol("test", SymbolLocation::new("a.rs", 1, 0, SymbolKind::Function));
        assert_eq!(idx.exact_search("test").len(), 1);
        
        idx.clear();
        assert_eq!(idx.exact_search("test").len(), 0);
        assert_eq!(idx.get_stats().total_symbols, 0);
    }
}
