//! 语义级重构引擎
//!
//! 对标 Claude Code 的语义编辑功能，提供：
//! - 符号重命名 (跨文件)
//! - 提取方法/函数
//! - 内联变量/函数
//! - 移动符号到其他文件
//! - 变更签名
//! - 代码格式化
//!
//! 使用正则匹配 + 文件扫描，不依赖 LSP（LSP 作为可选增强）

use anyhow::Result;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// 重构操作类型
#[derive(Debug, Clone)]
pub enum RefactorKind {
    Rename {
        old_name: String,
        new_name: String,
        file_path: String,
        symbol_kind: SymbolKind,
    },
    ExtractMethod {
        file_path: String,
        start_line: usize,
        end_line: usize,
        new_name: String,
    },
    Inline {
        file_path: String,
        symbol_name: String,
    },
    ChangeSignature {
        file_path: String,
        function_name: String,
        new_params: Vec<String>,
    },
    Format {
        file_path: String,
        formatter: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum SymbolKind {
    Variable,
    Function,
    Class,
    Struct,
    Trait,
    Module,
    Import,
}

/// 符号引用
#[derive(Debug, Clone)]
pub struct SymbolReference {
    pub file_path: String,
    pub line: usize,
    pub column: usize,
    pub context: String,
}

/// 重构结果
#[derive(Debug, Clone)]
pub struct RefactorResult {
    pub kind: String,
    pub success: bool,
    pub modified_files: Vec<ModifiedFile>,
    pub diff_summary: String,
    pub warnings: Vec<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ModifiedFile {
    pub path: String,
    pub changes: usize,
    pub diff_preview: String,
}

/// 语义重构引擎
pub struct RefactorEngine {
    workspace_root: std::path::PathBuf,
    dry_run: bool,
}

impl RefactorEngine {
    pub fn new(workspace_root: &std::path::Path) -> Self {
        Self {
            workspace_root: workspace_root.to_path_buf(),
            dry_run: false,
        }
    }

    /// 设置为预览模式（不实际修改文件）
    pub fn dry_run(mut self) -> Self {
        self.dry_run = true;
        self
    }

    /// 执行重构
    pub async fn execute(&self, kind: &RefactorKind) -> Result<RefactorResult> {
        match kind {
            RefactorKind::Rename { old_name, new_name, file_path, symbol_kind } => {
                self.rename(old_name, new_name, file_path, symbol_kind).await
            }
            RefactorKind::ExtractMethod { file_path, start_line, end_line, new_name } => {
                self.extract_method(file_path, *start_line, *end_line, new_name).await
            }
            RefactorKind::Inline { file_path, symbol_name } => {
                self.inline(file_path, symbol_name).await
            }
            RefactorKind::ChangeSignature { file_path, function_name, new_params } => {
                self.change_signature(file_path, function_name, new_params).await
            }
            RefactorKind::Format { file_path, formatter } => {
                self.format(file_path, formatter).await
            }
        }
    }

    /// 跨文件重命名符号
    async fn rename(&self, old_name: &str, new_name: &str, file_path: &str, _kind: &SymbolKind) -> Result<RefactorResult> {
        let start = std::time::Instant::now();
        let mut modified = Vec::new();
        let mut warnings = Vec::new();

        // 1. 收集所有引用
        let references = self.find_references(old_name, file_path).await?;

        if references.is_empty() {
            warnings.push(format!("No references found for '{}'", old_name));
        }

        // 2. 对每个引用文件执行替换
        let mut file_changes: HashMap<String, (String, usize)> = HashMap::new();

        for (file, content) in self.collect_file_contents(&references).await? {
            let new_content = content.replace(old_name, new_name);
            if new_content != content {
                let count = count_occurrences(&content, old_name);
                file_changes.insert(file.clone(), (new_content, count));
            }
        }

        // 3. 写入文件
        for (file, (new_content, changes)) in &file_changes {
            let full_path = self.workspace_root.join(file);
            if !self.dry_run {
                tokio::fs::write(&full_path, new_content).await?;
            }

            // 生成 diff 预览
            let old_content = tokio::fs::read_to_string(&full_path).await.unwrap_or_default();
            let diff_preview = simple_diff(&old_content, new_content, 10);

            modified.push(ModifiedFile {
                path: file.clone(),
                changes: *changes,
                diff_preview,
            });
        }

        let elapsed = start.elapsed();
        let diff_summary = format!("Renamed '{}' -> '{}' across {} file(s) in {:?}", old_name, new_name, modified.len(), elapsed);

        Ok(RefactorResult {
            kind: "rename".to_string(),
            success: true,
            modified_files: modified,
            diff_summary,
            warnings,
            error: None,
        })
    }

    /// 提取方法
    async fn extract_method(&self, file_path: &str, start_line: usize, end_line: usize, new_name: &str) -> Result<RefactorResult> {
        let full_path = self.workspace_root.join(file_path);
        let content = tokio::fs::read_to_string(&full_path).await?;
        let lines: Vec<&str> = content.lines().collect();

        if start_line >= lines.len() || end_line > lines.len() || start_line >= end_line {
            anyhow::bail!("Invalid line range: {}..{} (file has {} lines)", start_line, end_line, lines.len());
        }

        let extracted_code: Vec<&str> = lines[start_line..end_line].iter().map(|l| *l).collect();
        let indent = detect_indent(extracted_code.first().unwrap_or(&""));

        // 检测语言以确定函数语法
        let func_decl = match file_path.rsplit('.').next() {
            Some("rs") => format!("fn {}() {{\n{}\n}}", new_name, extracted_code.join("\n")),
            Some("py") => format!("def {}():\n    \"\"\"Extracted method.\"\"\"\n{}", new_name, extracted_code.join("\n")),
            Some("ts") | Some("js") | Some("tsx") | Some("jsx") => {
                format!("function {}() {{\n{}\n}}", new_name, extracted_code.join("\n"))
            }
            _ => format!("// TODO: extracted method '{}'\n{}", new_name, extracted_code.join("\n")),
        };

        // 生成调用
        let call = format!("{}{}()", indent, new_name);

        // 替换原代码为函数调用
        let mut new_lines = lines.clone();
        for i in start_line..end_line {
            new_lines[i] = "";
        }
        new_lines[start_line] = &call;

        // 在文件末尾添加函数定义
        let mut result = new_lines.join("\n");
        result.push_str("\n\n");
        result.push_str(&func_decl);

        if !self.dry_run {
            tokio::fs::write(&full_path, &result).await?;
        }

        let diff_preview = simple_diff(&content, &result, 15);

        Ok(RefactorResult {
            kind: "extract_method".to_string(),
            success: true,
            modified_files: vec![ModifiedFile {
                path: file_path.to_string(),
                changes: 2, // extracted block + new function
                diff_preview,
            }],
            diff_summary: format!("Extracted method '{}' from lines {}-{}", new_name, start_line + 1, end_line),
            warnings: vec![],
            error: None,
        })
    }

    /// 内联符号
    async fn inline(&self, file_path: &str, symbol_name: &str) -> Result<RefactorResult> {
        let full_path = self.workspace_root.join(file_path);
        let content = tokio::fs::read_to_string(&full_path).await?;

        // 简单的变量内联：找到定义并替换引用
        // 对于更复杂的内联，可以使用 AST 解析器
        let re = Regex::new(&format!(r"(?m)^\s*(?:let|const|var|val)\s+{}\s*=\s*(.+?);?$", regex::escape(symbol_name))).unwrap();

        if let Some(cap) = re.captures(&content) {
            let definition = cap.get(1).unwrap().as_str().trim().to_string();
            let new_content = content.replace(symbol_name, &definition);

            if !self.dry_run {
                tokio::fs::write(&full_path, &new_content).await?;
            }

        let diff_preview = simple_diff(&content, &new_content, 10);

            return Ok(RefactorResult {
                kind: "inline".to_string(),
                success: true,
                modified_files: vec![ModifiedFile {
                    path: file_path.to_string(),
                    changes: count_occurrences(&content, symbol_name),
                    diff_preview,
                }],
                diff_summary: format!("Inlined '{}'", symbol_name),
                warnings: vec![],
                error: None,
            });
        }

        anyhow::bail!("Could not find definition of '{}' to inline", symbol_name);
    }

    /// 变更函数签名
    async fn change_signature(&self, file_path: &str, function_name: &str, new_params: &[String]) -> Result<RefactorResult> {
        let full_path = self.workspace_root.join(file_path);
        let content = tokio::fs::read_to_string(&full_path).await?;

        // 查找函数定义行并替换参数
        let def_re = Regex::new(&format!(
            r"(?m)^(\s*(?:fn|def|function)\s+{}\s*\().*?\)",
            regex::escape(function_name)
        )).unwrap();

        let params_str = new_params.join(", ");
        let new_content = def_re.replace(&content, |caps: &regex::Captures| {
            format!("{}{})", &caps[1], params_str)
        });

        if new_content == content {
            anyhow::bail!("Could not find function '{}' in {}", function_name, file_path);
        }

        if !self.dry_run {
            tokio::fs::write(&full_path, new_content.as_ref()).await?;
        }

        let diff_preview = simple_diff(&content, new_content.as_ref(), 10);

        Ok(RefactorResult {
            kind: "change_signature".to_string(),
            success: true,
            modified_files: vec![ModifiedFile {
                path: file_path.to_string(),
                changes: 1,
                diff_preview,
            }],
            diff_summary: format!("Changed signature of '{}'", function_name),
            warnings: vec![],
            error: None,
        })
    }

    /// 格式化代码
    async fn format(&self, file_path: &str, formatter: &str) -> Result<RefactorResult> {
        let full_path = self.workspace_root.join(file_path);
        let content = tokio::fs::read_to_string(&full_path).await?;

        match formatter {
            "rustfmt" => {
                let output = std::process::Command::new("rustfmt")
                    .arg("--edition")
                    .arg("2021")
                    .arg(full_path.to_str().unwrap())
                    .output()?;

                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    anyhow::bail!("rustfmt failed: {}", stderr);
                }
            }
            "prettier" => {
                let output = std::process::Command::new("npx")
                    .args(["prettier", "--write", full_path.to_str().unwrap()])
                    .output()?;

                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    anyhow::bail!("prettier failed: {}", stderr);
                }
            }
            _ => anyhow::bail!("Unknown formatter: {}", formatter),
        }

        let new_content = tokio::fs::read_to_string(&full_path).await?;
        let diff_preview = simple_diff(&content, &new_content, 10);

        Ok(RefactorResult {
            kind: "format".to_string(),
            success: true,
            modified_files: vec![ModifiedFile {
                path: file_path.to_string(),
                changes: 1,
                diff_preview,
            }],
            diff_summary: format!("Formatted with {}", formatter),
            warnings: vec![],
            error: None,
        })
    }

    /// 查找符号的所有引用（当前文件 + 跨文件）
    pub async fn find_references(&self, symbol: &str, file_path: &str) -> Result<Vec<SymbolReference>> {
        let mut refs = Vec::new();

        // 1. 查找当前文件中的引用
        let full_path = self.workspace_root.join(file_path);
        if full_path.exists() {
            let content = tokio::fs::read_to_string(&full_path).await?;
            let file_refs = self.find_refs_in_content(&content, file_path, symbol);
            refs.extend(file_refs);
        }

        // 2. 跨文件查找引用
        let lang = file_path.rsplit('.').next().unwrap_or("");
        let relevant_extensions = get_relevant_extensions(lang);

        let mut dirs = vec![self.workspace_root.clone()];
        while let Some(dir) = dirs.pop() {
            let mut entries = tokio::fs::read_dir(&dir).await?;
            while let Some(entry) = entries.next_entry().await? {
                let path = entry.path();
                if path.is_dir() {
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if !name.starts_with('.') && name != "node_modules" && name != "target" && name != "__pycache__" {
                        dirs.push(path);
                    }
                } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    if relevant_extensions.contains(&ext) {
                        if let Ok(rel) = path.strip_prefix(&self.workspace_root) {
                            let rel_str = rel.to_string_lossy().to_string();
                            if rel_str != file_path {
                                if let Ok(content) = tokio::fs::read_to_string(&path).await {
                                    let file_refs = self.find_refs_in_content(&content, &rel_str, symbol);
                                    refs.extend(file_refs);
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(refs)
    }

    fn find_refs_in_content(&self, content: &str, file_path: &str, symbol: &str) -> Vec<SymbolReference> {
        let mut refs = Vec::new();
        // 使用单词边界匹配，避免部分匹配
        let pattern = format!(r"\b{}\b", regex::escape(symbol));
        if let Ok(re) = Regex::new(&pattern) {
            for (line_num, line) in content.lines().enumerate() {
                for mat in re.find_iter(line) {
                    refs.push(SymbolReference {
                        file_path: file_path.to_string(),
                        line: line_num + 1,
                        column: mat.start() + 1,
                        context: line.trim().to_string(),
                    });
                }
            }
        }
        refs
    }

    async fn collect_file_contents(&self, refs: &[SymbolReference]) -> Result<HashMap<String, String>> {
        let mut files = HashMap::new();
        for r in refs {
            if !files.contains_key(&r.file_path) {
                let full_path = self.workspace_root.join(&r.file_path);
                if let Ok(content) = tokio::fs::read_to_string(&full_path).await {
                    files.insert(r.file_path.clone(), content);
                }
            }
        }
        Ok(files)
    }
}

// --- Helpers ---

fn count_occurrences(content: &str, pattern: &str) -> usize {
    if let Ok(re) = Regex::new(&format!(r"\b{}\b", regex::escape(pattern))) {
        re.find_iter(content).count()
    } else {
        0
    }
}

fn detect_indent(line: &str) -> String {
    let trimmed = line.trim_start();
    let indent_len = line.len() - trimmed.len();
    if indent_len > 0 {
        line[..indent_len].to_string()
    } else {
        "    ".to_string() // Default 4 spaces
    }
}

fn get_relevant_extensions(lang: &str) -> HashSet<&'static str> {
    match lang {
        "rs" => HashSet::from(["rs"]),
        "py" => HashSet::from(["py"]),
        "ts" | "tsx" => HashSet::from(["ts", "tsx", "js", "jsx"]),
        "js" | "jsx" => HashSet::from(["js", "jsx", "ts", "tsx"]),
        "go" => HashSet::from(["go"]),
        "java" => HashSet::from(["java", "kt"]),
        "kt" | "kts" => HashSet::from(["kt", "kts", "java"]),
        _ => HashSet::from(["rs", "py", "ts", "tsx", "js", "jsx", "go", "java", "kt"]),
    }
}

/// Simple line-by-line diff (no external dependency)
fn simple_diff(old: &str, new: &str, max_lines: usize) -> String {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let mut summary = String::new();
    let mut count = 0;

    let max_len = old_lines.len().max(new_lines.len());
    let mut i = 0;

    while i < max_len && count < max_lines {
        let old_line = old_lines.get(i).copied().unwrap_or("");
        let new_line = new_lines.get(i).copied().unwrap_or("");

        if old_line != new_line {
            if !old_line.is_empty() {
                summary.push_str(&format!("-{}\n", old_line));
                count += 1;
            }
            if !new_line.is_empty() {
                summary.push_str(&format!("+{}\n", new_line));
                count += 1;
            }
        }
        i += 1;
    }

    if count >= max_lines && i < max_len {
        summary.push_str("... (truncated)\n");
    }

    summary
}

// 语义重构模块 (Tree-sitter AST 感知)
pub mod semantic;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_rename_refs_empty() {
        let engine = RefactorEngine::new(Path::new("/tmp"));
        let refs = engine.find_references("NonExistentSymbol", "test.rs").await.unwrap();
        assert!(refs.is_empty());
    }

    #[test]
    fn test_count_occurrences() {
        assert_eq!(count_occurrences("fn foo() { foo(); }", "foo"), 2);
        assert_eq!(count_occurrences("abcdef", "abc"), 1);
        assert_eq!(count_occurrences("aaaa", "aa"), 2); // overlapping?
    }

    #[test]
    fn test_detect_indent() {
        assert_eq!(detect_indent("    let x = 1;"), "    ");
        assert_eq!(detect_indent("fn main() {"), "");
        assert_eq!(detect_indent("\t\treturn 0;"), "\t\t");
    }

    #[test]
    fn test_get_relevant_extensions() {
        let exts = get_relevant_extensions("rs");
        assert!(exts.contains("rs"));
        assert!(!exts.contains("py"));

        let exts = get_relevant_extensions("ts");
        assert!(exts.contains("tsx"));
        assert!(exts.contains("js"));
    }
}
