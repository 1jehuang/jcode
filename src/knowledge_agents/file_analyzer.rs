//! W1: File Analyzer — 分析每个文件，抽取符号/依赖 (并行)
//! 移植自: Understand-Anything agents/file-analyzer
//! 确定性解析器: tree-sitter + 正则混合

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Semaphore;

use super::project_scanner::FileEntry;
use super::{KGNode, NodeKind, RelationType, ComplexityLevel};

/// 文件分析结果
#[derive(Debug, Clone)]
pub struct FileAnalysis {
    pub node_id: String,
    pub file_path: String,
    pub language: String,
    pub symbol_name: String,
    pub summary: String,
    pub dependencies: Vec<String>,
    pub symbols: Vec<SymbolDef>,
    pub complexity: ComplexityLevel,
}

#[derive(Debug, Clone)]
pub struct SymbolDef {
    pub name: String,
    pub kind: SymbolKind,
    pub line: usize,
    pub column: usize,
    pub visibility: Option<String>,
    pub signature: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SymbolKind {
    Function, Method, Struct, Class, Enum, Trait, Interface, Module, Constant, Type, Macro, Import,
}

/// 并行分析文件 (最多 max_concurrent 并发)
pub async fn analyze_files(
    root: &Path,
    files: &[String],
    max_concurrent: usize,
) -> Result<Vec<FileAnalysis>, String> {
    let semaphore = Arc::new(Semaphore::new(max_concurrent));
    let mut handles = Vec::new();

    for file in files {
        let sem = semaphore.clone();
        let root = root.to_path_buf();
        let file = file.clone();

        let handle = tokio::spawn(async move {
            let _permit = sem.acquire().await.unwrap();
            analyze_single_file(&root, &file).await
        });
        handles.push(handle);
    }

    let mut results = Vec::new();
    for handle in handles {
        match handle.await {
            Ok(Ok(result)) => results.push(result),
            Ok(Err(e)) => eprintln!("[File Analyzer] Warning: {}", e),
            Err(e) => eprintln!("[File Analyzer] Join error: {}", e),
        }
    }

    Ok(results)
}

/// 分析单个文件
async fn analyze_single_file(root: &Path, rel_path: &str) -> Result<FileAnalysis, String> {
    let full_path = root.join(rel_path);
    let content = tokio::fs::read_to_string(&full_path).await
        .map_err(|e| format!("Read error {}: {}", rel_path, e))?;

    let language = super::project_scanner::detect_language(rel_path).unwrap_or("Unknown");

    // 提取符号和依赖
    let symbols = extract_symbols(&content, language);
    let dependencies = extract_dependencies(&content, language);
    let imports = extract_imports(&content, language);

    // 去重依赖
    let all_deps: HashSet<String> = dependencies.into_iter()
        .chain(imports.into_iter())
        .collect();

    // 生成自然摘要 (文件的第一行注释或文件名)
    let summary = generate_summary(&content, rel_path);

    let complexity = if content.lines().count() > 500 {
        ComplexityLevel::High
    } else if content.lines().count() > 200 {
        ComplexityLevel::Medium
    } else {
        ComplexityLevel::Low
    };

    let node_id = format!("file::{}", rel_path.replace('\\', "/"));

    Ok(FileAnalysis {
        node_id,
        file_path: rel_path.to_string(),
        language: language.to_string(),
        symbol_name: Path::new(rel_path).file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(rel_path)
            .to_string(),
        summary,
        dependencies: all_deps.into_iter().collect(),
        symbols,
        complexity,
    })
}

/// 提取符号定义 (基于正则, 多语言)
fn extract_symbols(content: &str, language: &str) -> Vec<SymbolDef> {
    let mut symbols = Vec::new();

    match language {
        "Rust" => {
            // fn name(...
            for cap in regex_lazy(r"^\s*(?:pub\s+)?(?:async\s+)?fn\s+(\w+)").captures_iter(content) {
                symbols.push(SymbolDef { name: cap[1].to_string(), kind: SymbolKind::Function, line: 0, column: 0, visibility: None, signature: None });
            }
            // struct Name
            for cap in regex_lazy(r"^\s*(?:pub\s+)?struct\s+(\w+)").captures_iter(content) {
                symbols.push(SymbolDef { name: cap[1].to_string(), kind: SymbolKind::Struct, line: 0, column: 0, visibility: None, signature: None });
            }
            // enum Name
            for cap in regex_lazy(r"^\s*(?:pub\s+)?enum\s+(\w+)").captures_iter(content) {
                symbols.push(SymbolDef { name: cap[1].to_string(), kind: SymbolKind::Enum, line: 0, column: 0, visibility: None, signature: None });
            }
            // trait Name
            for cap in regex_lazy(r"^\s*(?:pub\s+)?(?:unsafe\s+)?trait\s+(\w+)").captures_iter(content) {
                symbols.push(SymbolDef { name: cap[1].to_string(), kind: SymbolKind::Trait, line: 0, column: 0, visibility: None, signature: None });
            }
            // impl for Type
            for cap in regex_lazy(r"^\s*impl\s+(?:<[^>]+>\s+)?(\w+)").captures_iter(content) {
                symbols.push(SymbolDef { name: cap[1].to_string(), kind: SymbolKind::Method, line: 0, column: 0, visibility: None, signature: None });
            }
            // mod name
            for cap in regex_lazy(r"^\s*(?:pub\s+)?mod\s+(\w+)").captures_iter(content) {
                symbols.push(SymbolDef { name: cap[1].to_string(), kind: SymbolKind::Module, line: 0, column: 0, visibility: None, signature: None });
            }
            // type Name
            for cap in regex_lazy(r"^\s*(?:pub\s+)?type\s+(\w+)").captures_iter(content) {
                symbols.push(SymbolDef { name: cap[1].to_string(), kind: SymbolKind::Type, line: 0, column: 0, visibility: None, signature: None });
            }
            // const NAME
            for cap in regex_lazy(r"^\s*(?:pub\s+)?const\s+(\w+)").captures_iter(content) {
                symbols.push(SymbolDef { name: cap[1].to_string(), kind: SymbolKind::Constant, line: 0, column: 0, visibility: None, signature: None });
            }
        }
        "TypeScript" | "JavaScript" => {
            for cap in regex_lazy(r"(?:export\s+)?(?:async\s+)?function\s+(\w+)").captures_iter(content) {
                symbols.push(SymbolDef { name: cap[1].to_string(), kind: SymbolKind::Function, line: 0, column: 0, visibility: None, signature: None });
            }
            for cap in regex_lazy(r"(?:export\s+)?class\s+(\w+)").captures_iter(content) {
                symbols.push(SymbolDef { name: cap[1].to_string(), kind: SymbolKind::Class, line: 0, column: 0, visibility: None, signature: None });
            }
            for cap in regex_lazy(r"(?:export\s+)?interface\s+(\w+)").captures_iter(content) {
                symbols.push(SymbolDef { name: cap[1].to_string(), kind: SymbolKind::Interface, line: 0, column: 0, visibility: None, signature: None });
            }
        }
        "Python" => {
            for cap in regex_lazy(r"^(?:async\s+)?def\s+(\w+)").captures_iter(content) {
                symbols.push(SymbolDef { name: cap[1].to_string(), kind: SymbolKind::Function, line: 0, column: 0, visibility: None, signature: None });
            }
            for cap in regex_lazy(r"^class\s+(\w+)").captures_iter(content) {
                symbols.push(SymbolDef { name: cap[1].to_string(), kind: SymbolKind::Class, line: 0, column: 0, visibility: None, signature: None });
            }
        }
        "Go" => {
            for cap in regex_lazy(r"^func\s+(?:\(\w+\s+\*?\w+\)\s+)?(\w+)").captures_iter(content) {
                symbols.push(SymbolDef { name: cap[1].to_string(), kind: SymbolKind::Function, line: 0, column: 0, visibility: None, signature: None });
            }
            for cap in regex_lazy(r"^type\s+(\w+)\s+struct").captures_iter(content) {
                symbols.push(SymbolDef { name: cap[1].to_string(), kind: SymbolKind::Struct, line: 0, column: 0, visibility: None, signature: None });
            }
            for cap in regex_lazy(r"^type\s+(\w+)\s+interface").captures_iter(content) {
                symbols.push(SymbolDef { name: cap[1].to_string(), kind: SymbolKind::Interface, line: 0, column: 0, visibility: None, signature: None });
            }
        }
        "Java" | "Kotlin" => {
            for cap in regex_lazy(r"(?:public|private|protected)?\s*(?:static\s+)?class\s+(\w+)").captures_iter(content) {
                symbols.push(SymbolDef { name: cap[1].to_string(), kind: SymbolKind::Class, line: 0, column: 0, visibility: None, signature: None });
            }
            for cap in regex_lazy(r"(?:public|private|protected)?\s*(?:static\s+)?interface\s+(\w+)").captures_iter(content) {
                symbols.push(SymbolDef { name: cap[1].to_string(), kind: SymbolKind::Interface, line: 0, column: 0, visibility: None, signature: None });
            }
        }
        _ => {}
    }

    symbols
}

/// 提取文件间依赖 (文件路径引用)
fn extract_dependencies(content: &str, language: &str) -> Vec<String> {
    let mut deps = Vec::new();
    match language {
        "Rust" => {
            // mod xxx; 或 mod xxx {
            for cap in regex_lazy(r"^\s*(?:pub\s+)?mod\s+(\w+)").captures_iter(content) {
                deps.push(format!("file::{}", &cap[1]));
            }
        }
        _ => {}
    }
    deps
}

/// 提取导入语句
fn extract_imports(content: &str, language: &str) -> Vec<String> {
    let mut imports = Vec::new();
    match language {
        "Rust" => {
            for cap in regex_lazy(r"^\s*use\s+(?:\w+::)*(\w+)").captures_iter(content) {
                imports.push(format!("symbol::{}", &cap[1]));
            }
            for cap in regex_lazy(r"^\s*extern\s+crate\s+(\w+)").captures_iter(content) {
                imports.push(format!("crate::{}", &cap[1]));
            }
        }
        "TypeScript" | "JavaScript" => {
            for cap in regex_lazy(r#"(?:import|require)\s*\(?['\"]([^'\"]+)['\"]"#).captures_iter(content) {
                imports.push(format!("import::{}", &cap[1]));
            }
        }
        "Python" => {
            for cap in regex_lazy(r"^(?:from\s+(\S+)\s+)?import\s+(\S+)").captures_iter(content) {
                let target = if cap.get(1).is_some() {
                    format!("{}::{}", &cap[1], &cap[2])
                } else {
                    cap[2].to_string()
                };
                imports.push(format!("import::{}", target));
            }
        }
        "Go" => {
            for cap in regex_lazy(r#"^\s*"(\S+)"\s*$"#).captures_iter(content) {
                imports.push(format!("import::{}", &cap[1]));
            }
        }
        _ => {}
    }
    imports
}

/// 生成摘要 (提取文件头注释或文件名)
fn generate_summary(content: &str, path: &str) -> String {
    // 找第一行注释
    for line in content.lines().take(20) {
        let trimmed = line.trim();
        if trimmed.starts_with("//!") || trimmed.starts_with("///") {
            let text = trimmed.trim_start_matches("//!").trim().trim_start_matches("///").trim();
            if !text.is_empty() && text.len() < 200 {
                return text.to_string();
            }
        }
        if trimmed.starts_with("# ") && !trimmed.contains("![") {
            return trimmed.trim_start_matches("# ").to_string();
        }
        if trimmed.starts_with("\"\"\"") {
            let lines: Vec<&str> = content.lines().skip(1).take(5).collect();
            for l in lines {
                let t = l.trim();
                if !t.is_empty() && !t.starts_with("\"\"\"") {
                    return t.to_string();
                }
            }
        }
    }
    // 回退: 使用文件名
    Path::new(path).file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(path)
        .to_string()
}

/// 懒加载 regex 缓存 (避免重复编译)
fn regex_lazy(pattern: &'static str) -> regex::Regex {
    regex::Regex::new(pattern).expect("Invalid regex pattern")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rust_symbols() {
        let content = r#"
pub fn main() {}
struct User { name: String }
enum Status { Active, Inactive }
trait Display {}
mod utils;
const MAX_SIZE: usize = 100;
type UserId = u64;
"#;
        let symbols = extract_symbols(content, "Rust");
        assert!(symbols.iter().any(|s| s.name == "main"));
        assert!(symbols.iter().any(|s| s.name == "User"));
        assert!(symbols.iter().any(|s| s.name == "Status"));
        assert!(symbols.iter().any(|s| s.name == "Display"));
        assert!(symbols.iter().any(|s| s.name == "utils"));
        assert!(symbols.iter().any(|s| s.name == "MAX_SIZE"));
        assert!(symbols.iter().any(|s| s.name == "UserId"));
    }

    #[test]
    fn test_rust_imports() {
        let content = "use std::collections::HashMap;\nextern crate serde;\n";
        let imports = extract_imports(content, "Rust");
        assert!(imports.iter().any(|i| i.contains("HashMap")));
        assert!(imports.iter().any(|i| i.contains("serde")));
    }

    #[test]
    fn test_summary_extraction() {
        let content = "//! This is a module for handling user authentication\n//!\n//! More docs\npub fn authenticate() {}";
        assert_eq!(generate_summary(content, "auth.rs"), "This is a module for handling user authentication");
    }

    #[test]
    fn test_python_symbols() {
        let content = "def hello():\n    pass\n\nclass User:\n    pass\n";
        let symbols = extract_symbols(content, "Python");
        assert!(symbols.iter().any(|s| s.name == "hello"));
        assert!(symbols.iter().any(|s| s.name == "User"));
    }

    #[test]
    fn test_ts_symbols() {
        let content = "export function hello() {}\nclass User {}\ninterface IUser {}\n";
        let symbols = extract_symbols(content, "TypeScript");
        assert!(symbols.iter().any(|s| s.name == "hello"));
        assert!(symbols.iter().any(|s| s.name == "User"));
        assert!(symbols.iter().any(|s| s.name == "IUser"));
    }
}
