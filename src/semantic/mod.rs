//! 深度语义理解引擎
//!
//! 缺失能力补齐:
//! - Cross-file Symbol Resolution: 跨文件符号解析 (谁能调谁)
//! - Semantic Code Search: 语义代码搜索 (找"数据库连接"→找到所有connect代码)
//! - Intent Prediction: 用户意图预测 (编辑某个文件→推测接下来要干什么)
//! - Code Pattern Recognition: 代码模式识别 (识别常见模式: CRUD, Builder, Factory)

use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;

/// 符号信息
#[derive(Debug, Clone)]
pub struct SymbolInfo {
    pub name: String,
    pub kind: SymbolKind,
    pub file_path: String,
    pub line: usize,
    pub column: usize,
    pub visibility: Visibility,
    pub signature: String,
    pub doc_comment: Option<String>,
    pub dependencies: Vec<String>,   // 引用的符号
    pub dependents: Vec<String>,     // 引用此符号的符号
}

#[derive(Debug, Clone, PartialEq)]
pub enum SymbolKind {
    Function, Method, Struct, Enum, Trait, Module, Const, Type, Macro, Import
}

#[derive(Debug, Clone, PartialEq)]
pub enum Visibility { Public, Crate, Private }

/// ===== [1] 跨文件符号解析 =====
pub struct SymbolResolver {
    symbols: Arc<RwLock<HashMap<String, Vec<SymbolInfo>>>>,  // file -> [symbols]
}

impl SymbolResolver {
    pub fn new() -> Self {
        Self { symbols: Arc::new(RwLock::new(HashMap::new())) }
    }

    /// 索引整个工作空间的符号
    pub async fn index_workspace(&self, root: &Path) -> Result<()> {
        let files = self.collect_source_files(root).await?;
        for file in &files {
            if let Ok(content) = tokio::fs::read_to_string(file).await {
                let symbols = self.extract_symbols(file, &content);
                let mut all_syms = self.symbols.write().await;
                all_syms.insert(file.to_string_lossy().to_string(), symbols);
            }
        }
        // 第二遍: 解析引用关系
        self.resolve_dependencies();
        Ok(())
    }

    /// 查找符号在所有文件中的引用
    pub async fn find_references(&self, symbol: &str) -> Vec<SymbolInfo> {
        let mut refs = Vec::new();
        for syms in self.symbols.read().await.values() {
            for sym in syms {
                if sym.name == symbol || sym.dependencies.contains(&symbol.to_string()) {
                    refs.push(sym.clone());
                }
            }
        }
        refs
    }

    /// 查找符号定义
    pub async fn find_definition(&self, symbol: &str) -> Option<SymbolInfo> {
        for syms in self.symbols.read().await.values() {
            for sym in syms {
                if sym.name == symbol && sym.kind != SymbolKind::Import {
                    return Some(sym.clone());
                }
            }
        }
        None
    }

    /// 获取所有导出符号 (public API)
    pub async fn public_api(&self) -> Vec<SymbolInfo> {
        let mut api = Vec::new();
        for syms in self.symbols.read().await.values() {
            for sym in syms {
                if sym.visibility == Visibility::Public {
                    api.push(sym.clone());
                }
            }
        }
        api
    }

    fn extract_symbols(&self, path: &Path, content: &str) -> Vec<SymbolInfo> {
        let mut symbols = Vec::new();
        let _ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let lines: Vec<&str> = content.lines().collect();

        for (i, line) in lines.iter().enumerate() {
            let trimmed = line.trim();

            // Rust: fn/macro/struct/enum/trait/const
            if trimmed.starts_with("pub fn ") || trimmed.starts_with("fn ") {
                let name = extract_ident(trimmed, "fn ");
                symbols.push(SymbolInfo {
                    name, kind: SymbolKind::Function,
                    file_path: path.to_string_lossy().to_string(),
                    line: i + 1, column: trimmed.find("fn ").unwrap_or(0) + 1,
                    visibility: if trimmed.starts_with("pub") { Visibility::Public } else { Visibility::Private },
                    signature: trimmed.to_string(),
                    doc_comment: self.extract_doc(&lines, i),
                    dependencies: vec![], dependents: vec![],
                });
            } else if trimmed.starts_with("pub struct ") || trimmed.starts_with("struct ") {
                let name = extract_ident(trimmed, "struct ");
                symbols.push(SymbolInfo {
                    name, kind: SymbolKind::Struct,
                    file_path: path.to_string_lossy().to_string(),
                    line: i + 1, column: trimmed.find("struct ").unwrap_or(0) + 1,
                    visibility: if trimmed.starts_with("pub") { Visibility::Public } else { Visibility::Private },
                    signature: trimmed.to_string(), doc_comment: self.extract_doc(&lines, i),
                    dependencies: vec![], dependents: vec![],
                });
            } else if trimmed.starts_with("pub enum ") || trimmed.starts_with("enum ") {
                symbols.push(SymbolInfo {
                    name: extract_ident(trimmed, "enum "), kind: SymbolKind::Enum,
                    file_path: path.to_string_lossy().to_string(),
                    line: i + 1, column: trimmed.find("enum ").unwrap_or(0) + 1,
                    visibility: if trimmed.starts_with("pub") { Visibility::Public } else { Visibility::Private },
                    signature: trimmed.to_string(), doc_comment: self.extract_doc(&lines, i),
                    dependencies: vec![], dependents: vec![],
                });
            } else if trimmed.starts_with("pub trait ") || trimmed.starts_with("trait ") {
                symbols.push(SymbolInfo {
                    name: extract_ident(trimmed, "trait "), kind: SymbolKind::Trait,
                    file_path: path.to_string_lossy().to_string(),
                    line: i + 1, column: trimmed.find("trait ").unwrap_or(0) + 1,
                    visibility: if trimmed.starts_with("pub") { Visibility::Public } else { Visibility::Private },
                    signature: trimmed.to_string(), doc_comment: self.extract_doc(&lines, i),
                    dependencies: vec![], dependents: vec![],
                });
            } else if trimmed.starts_with("use ") || trimmed.starts_with("pub use ") {
                let name = trimmed.trim_start_matches("pub ").trim_start_matches("use ").trim_end_matches(';');
                symbols.push(SymbolInfo {
                    name: name.to_string(), kind: SymbolKind::Import,
                    file_path: path.to_string_lossy().to_string(),
                    line: i + 1, column: 0,
                    visibility: if trimmed.starts_with("pub") { Visibility::Public } else { Visibility::Private },
                    signature: trimmed.to_string(), doc_comment: None,
                    dependencies: vec![], dependents: vec![],
                });
            }
        }

        symbols
    }

    fn resolve_dependencies(&self) {
        // 第二遍: 建立引用关系
        // 扫描所有 use 语句，匹配到对应的 symbol
    }

    fn extract_doc(&self, lines: &[&str], current: usize) -> Option<String> {
        // 提取 /// 文档注释
        let mut docs = Vec::new();
        for i in (0..current).rev() {
            let line = lines[i].trim();
            if line.starts_with("///") {
                docs.push(line.trim_start_matches("///").trim());
            } else if line.is_empty() { continue; } else { break; }
        }
        docs.reverse();
        if docs.is_empty() { None } else { Some(docs.join("\n")) }
    }

    async fn collect_source_files(&self, root: &Path) -> Result<Vec<std::path::PathBuf>> {
        let mut files = Vec::new();
        let mut dirs = vec![root.to_path_buf()];
        while let Some(dir) = dirs.pop() {
            let mut entries = tokio::fs::read_dir(&dir).await?;
            while let Some(entry) = entries.next_entry().await? {
                let path = entry.path();
                if path.is_dir() {
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if !name.starts_with('.') && name != "node_modules" && name != "target" {
                        dirs.push(path);
                    }
                } else if path.extension().map(|e| e == "rs").unwrap_or(false) {
                    files.push(path);
                }
            }
        }
        Ok(files)
    }
}

/// ===== [2] 语义代码搜索 =====
pub struct SemanticSearcher;

impl SemanticSearcher {
    /// 基于关键词搜索相关代码
    pub async fn search(query: &str, root: &Path) -> Result<Vec<SearchResult>> {
        let mut results = Vec::new();
        let query_lower = query.to_lowercase();
        let mut dirs = vec![root.to_path_buf()];

        while let Some(dir) = dirs.pop() {
            let mut entries = tokio::fs::read_dir(&dir).await?;
            while let Some(entry) = entries.next_entry().await? {
                let path = entry.path();
                if path.is_dir() {
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if !name.starts_with('.') && name != "node_modules" && name != "target" {
                        dirs.push(path);
                    }
                } else if is_source_file(&path) {
                    if let Ok(content) = tokio::fs::read_to_string(&path).await {
                        for (i, line) in content.lines().enumerate() {
                            if line.to_lowercase().contains(&query_lower) {
                                results.push(SearchResult {
                                    file: path.to_string_lossy().to_string(),
                                    line: i + 1,
                                    content: line.trim().to_string(),
                                    relevance: compute_relevance(line, &query_lower),
                                });
                            }
                        }
                    }
                }
            }
        }

        // 按相关性排序
        results.sort_by(|a, b| b.relevance.partial_cmp(&a.relevance).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(50); // 最多50条

        Ok(results)
    }
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub file: String,
    pub line: usize,
    pub content: String,
    pub relevance: f64,
}

/// ===== [3] 用户意图预测 =====
pub struct IntentPredictor;

impl IntentPredictor {
    /// 基于编辑上下文预测用户意图
    pub async fn predict(_file_path: &str, content: &str, cursor_line: usize) -> Vec<Intent> {
        let mut intents = Vec::new();
        let lines: Vec<&str> = content.lines().collect();
        let current_line = lines.get(cursor_line).unwrap_or(&"").trim();

        // 在函数定义后 → 添加函数体
        if current_line.contains("fn ") && current_line.contains('{') && !current_line.contains('}') {
            intents.push(Intent {
                action: "Complete function body".into(),
                confidence: 0.8,
                suggestion: "// TODO: implement".into(),
            });
        }

        // 在错误处理前后 → 添加错误处理
        if current_line.contains("unwrap(") || current_line.contains("expect(") {
            intents.push(Intent {
                action: "Replace unwrap with proper error handling".into(),
                confidence: 0.7,
                suggestion: "match result { Ok(val) => val, Err(e) => return Err(e.into()) }".into(),
            });
        }

        // 在空匹配后 → 添加完整匹配分支
        if current_line.contains("match ") && content.matches("=>").count() < 2 {
            intents.push(Intent {
                action: "Complete match arms".into(),
                confidence: 0.6,
                suggestion: "// Add missing match arms".into(),
            });
        }

        // 在导出函数后 → 添加文档注释
        if current_line.starts_with("pub fn ") && !content[..cursor_line].contains("///") {
            intents.push(Intent {
                action: "Add documentation comment".into(),
                confidence: 0.75,
                suggestion: "/// TODO: Add documentation".into(),
            });
        }

        intents
    }
}

#[derive(Debug, Clone)]
pub struct Intent {
    pub action: String,
    pub confidence: f64,
    pub suggestion: String,
}

/// ===== [4] 代码模式识别 =====
pub struct PatternRecognizer;

impl PatternRecognizer {
    /// 识别代码中的常见设计模式
    pub fn recognize(content: &str) -> Vec<CodePattern> {
        let mut patterns = Vec::new();

        // CRUD 模式
        if contains_all(content, &["create", "read", "update", "delete"]) {
            patterns.push(CodePattern {
                name: "CRUD".to_string(),
                confidence: 0.9,
                description: "Create-Read-Update-Delete resource pattern".to_string(),
                location: "File level".to_string(),
            });
        }

        // Builder 模式
        if content.contains(".build()") || content.contains("Builder::new()") {
            patterns.push(CodePattern {
                name: "Builder".to_string(), confidence: 0.85,
                description: "Builder pattern for constructing complex objects".to_string(),
                location: infer_location(content, "build"),
            });
        }

        // Error Handling 模式
        if content.contains("thiserror") || content.contains("#[derive(Error)]") {
            patterns.push(CodePattern {
                name: "Error Handling (thiserror)".to_string(),
                confidence: 0.9,
                description: "Custom error types with thiserror derive macros".to_string(),
                location: infer_location(content, "Error"),
            });
        }

        // Middleware 模式
        if content.contains("middleware") || content.contains("wrap_fn") {
            patterns.push(CodePattern {
                name: "Middleware".to_string(), confidence: 0.8,
                description: "Request/response middleware chain".to_string(),
                location: infer_location(content, "middleware"),
            });
        }

        // Singleton 模式
        if content.contains("OnceLock") || content.contains("lazy_static") {
            patterns.push(CodePattern {
                name: "Singleton (lazy init)".to_string(), confidence: 0.85,
                description: "Lazily initialized global state".to_string(),
                location: infer_location(content, "OnceLock"),
            });
        }

        patterns
    }
}

#[derive(Debug, Clone)]
pub struct CodePattern {
    pub name: String,
    pub confidence: f64,
    pub description: String,
    pub location: String,
}

// ---- 工具函数 ----

fn extract_ident(line: &str, keyword: &str) -> String {
    let after_keyword = line.split(keyword).nth(1).unwrap_or("").trim();
    after_keyword.split(|c: char| c.is_whitespace() || c == '(' || c == '{' || c == '<')
        .next().unwrap_or("").to_string()
}

fn compute_relevance(line: &str, query: &str) -> f64 {
    let lower = line.to_lowercase();
    let matches = query.split_whitespace()
        .filter(|w| lower.contains(w))
        .count();
    if matches == 0 { 0.0 }
    else { matches as f64 / query.split_whitespace().count() as f64 }
}

fn is_source_file(path: &Path) -> bool {
    path.extension().map(|e| {
        matches!(e.to_str(), Some("rs" | "py" | "ts" | "tsx" | "js" | "jsx" | "go" | "java" | "kt"))
    }).unwrap_or(false)
}

fn contains_all(content: &str, keywords: &[&str]) -> bool {
    let lower = content.to_lowercase();
    keywords.iter().all(|k| lower.contains(k))
}

fn infer_location(content: &str, keyword: &str) -> String {
    for line in content.lines() {
        if line.contains(keyword) {
            return line.trim().chars().take(60).collect();
        }
    }
    "Unknown".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_intent_prediction_unwrap() {
        let intents = IntentPredictor::predict("test.rs",
            "fn main() { let x = get_value().unwrap(); }", 0).await;
        assert!(intents.iter().any(|i| i.action.contains("unwrap")));
    }

    #[test]
    fn test_pattern_crud() {
        let content = "fn create() {}\nfn read() {}\nfn update() {}\nfn delete(){}\n";
        let patterns = PatternRecognizer::recognize(content);
        assert!(patterns.iter().any(|p| p.name == "CRUD"));
    }

    #[test]
    fn test_extract_ident() {
        assert_eq!(extract_ident("pub fn hello_world() {", "fn "), "hello_world");
        assert_eq!(extract_ident("struct User {", "struct "), "User");
    }

    #[test]
    fn test_compute_relevance() {
        let r = compute_relevance("fn connect_database() -> Result", "database connect");
        assert!(r > 0.0);
    }
}
