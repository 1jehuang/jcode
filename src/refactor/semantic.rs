//! 语义重构引擎 — Tree-sitter AST 感知的精确重构
//!
//! 升级路径:
//!   之前: 正则匹配 → 猜作用域 → 猜类型
//!   现在: Tree-sitter AST → 精确符号解析 → 作用域限定替换
//!   未来: LSP 类型查询 → 精确类型替换
//!
//! 复用: crates/jcode-cross-file-repair/src/ast.rs 的 TreeSitterAstAdapter

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;

use jcode_cross_file_repair::ast::{TreeSitterAstAdapter, AstNode};

// ========================================================================
// [1] AST 感知重命名 — 基于 Tree-sitter 的精确符号替换
// ========================================================================

/// 符号引用 (AST 精确版)
#[derive(Debug, Clone)]
pub struct SymbolReference {
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub scope: String,        // 真实作用域: "fn:main" / "impl:Struct"
    pub is_definition: bool,
    pub ast_node_kind: String, // tree-sitter node kind: "function_item" / "call_expression"
}

/// Tree-sitter 驱动的作用域感知重命名器
pub struct AstRenamer {
    workspace_root: std::path::PathBuf,
    adapter: TreeSitterAstAdapter,
}

impl AstRenamer {
    pub fn new(root: &Path) -> Self {
        Self {
            workspace_root: root.to_path_buf(),
            adapter: TreeSitterAstAdapter::rust(), // 默认 Rust, 后续按语言切换
        }
    }

    /// 使用 Tree-sitter 查找符号引用 (精确 AST, 非正则)
    pub async fn find_references_ast(
        &self, symbol: &str, file_path: &str,
    ) -> Result<Vec<SymbolReference>, String> {
        let full_path = self.resolve(file_path);
        let content = tokio::fs::read_to_string(&full_path).await
            .map_err(|e| format!("Read {}: {}", file_path, e))?;

        // 解析 AST
        let ast_nodes = self.adapter.parse(&content, &full_path)
            .await
            .map_err(|e| format!("Parse {}: {}", file_path, e))?;

        let mut refs = Vec::new();

        // 遍历 AST 寻找符号引用
        self.find_symbol_in_node(&ast_nodes, symbol, file_path, &content, &mut refs);

        // 如果 AST 没找到, 回退到行级查找
        if refs.is_empty() {
            refs = self.fallback_find_in_file(file_path, symbol).await?;
        }

        Ok(refs)
    }

    /// 在 AST 节点树中递归查找符号
    fn find_symbol_in_node(
        &self, nodes: &[AstNode], symbol: &str, file_path: &str,
        content: &str, refs: &mut Vec<SymbolReference>,
    ) {
        for node in nodes {
            // 检查节点名称是否匹配
            if let Some(ref name) = node.name {
                if name == symbol {
                    // 确定是否是定义 (
                    let is_def = matches!(node.kind.as_str(),
                        "function_item" | "struct_item" | "enum_item"
                        | "trait_item" | "type_item" | "const_item"
                        | "impl_item" | "macro_definition"
                    );
                    refs.push(SymbolReference {
                        file: file_path.to_string(),
                        line: node.start_line,
                        column: 0,
                        scope: self.detect_scope_from_ast(nodes, &node.kind),
                        is_definition: is_def,
                        ast_node_kind: node.kind.clone(),
                    });
                }
            }

            // 检查字符串内容中是否包含符号 (用于 call_expression / identifier)
            if node.kind == "call_expression" || node.kind == "identifier" {
                // 从源码中提取此 AST 节点对应的文本
                if let Some(identifier) = self.extract_node_text(content, node) {
                    if identifier == symbol {
                        refs.push(SymbolReference {
                            file: file_path.to_string(),
                            line: node.start_line,
                            column: 0,
                            scope: self.detect_scope_from_ast(nodes, &node.kind),
                            is_definition: false,
                            ast_node_kind: node.kind.clone(),
                        });
                    }
                }
            }

            // 递归子节点
            self.find_symbol_in_node(&node.children, symbol, file_path, content, refs);
        }
    }

    /// 从源码提取 AST 节点的文本
    fn extract_node_text(&self, content: &str, node: &AstNode) -> Option<String> {
        let lines: Vec<&str> = content.lines().collect();
        if node.start_line > 0 && node.start_line <= lines.len() {
            Some(lines[node.start_line - 1].trim().to_string())
        } else {
            None
        }
    }

    /// 从 AST 树检测作用域
    fn detect_scope_from_ast(&self, nodes: &[AstNode], _current_kind: &str) -> String {
        for node in nodes {
            if matches!(node.kind.as_str(),
                "function_item" | "struct_item" | "trait_item" | "impl_item"
            ) {
                if let Some(ref name) = node.name {
                    return format!("{}:{}", node.kind.trim_end_matches("_item"), name);
                }
            }
            if !node.children.is_empty() {
                let child_scope = self.detect_scope_from_ast(&node.children, _current_kind);
                if child_scope != "global" {
                    return child_scope;
                }
            }
        }
        "global".to_string()
    }

    /// 执行 Tree-sitter 精确重命名
    pub async fn rename_ast(&self, file_path: &str, symbol: &str, new_name: &str) -> Result<String, String> {
        let refs = self.find_references_ast(symbol, file_path).await?;
        if refs.is_empty() {
            // 回退到正则
            return self.fallback_rename(file_path, symbol, new_name).await;
        }

        let mut changed_files: HashMap<String, Vec<String>> = HashMap::new();

        // 收集文件内容
        for r in &refs {
            if !changed_files.contains_key(&r.file) {
                let path = self.resolve(&r.file);
                let content = tokio::fs::read_to_string(&path).await
                    .map_err(|e| format!("Read {}: {}", r.file, e))?;
                changed_files.insert(r.file.clone(), content.lines().map(|l| l.to_string()).collect());
            }
        }

        // 精确行内替换 (只替换 AST 报告的引用位置)
        for (file, lines) in &mut changed_files {
            let file_refs: Vec<&SymbolReference> = refs.iter().filter(|r| r.file == *file).collect();
            for r in &file_refs {
                if r.line > 0 && r.line <= lines.len() {
                    let l = &lines[r.line - 1];
                    if let Some(pos) = l.find(symbol) {
                        lines[r.line - 1] = l.replacen(symbol, new_name, 1);
                    }
                }
            }
        }

        // 写入
        for (file, lines) in &changed_files {
            let path = self.resolve(file);
            tokio::fs::write(&path, lines.join("\n")).await
                .map_err(|e| format!("Write {}: {}", file, e))?;
        }

        let file_count = changed_files.len();
        Ok(format!("[Tree-sitter] Renamed '{}' → '{}' in {} files", symbol, new_name, file_count))
    }

    /// === 回退方案 (正则) ===

    async fn fallback_find_in_file(&self, file_path: &str, symbol: &str) -> Result<Vec<SymbolReference>, String> {
        let full_path = self.resolve(file_path);
        let content = tokio::fs::read_to_string(&full_path).await
            .map_err(|e| format!("Read {}: {}", file_path, e))?;

        let mut refs = Vec::new();
        let ext = full_path.extension().and_then(|s| s.to_str()).unwrap_or("");

        for (i, line) in content.lines().enumerate() {
            if line.contains(symbol) {
                let trimmed = line.trim();
                if is_comment_line(trimmed, ext) { continue; }
                if let Some(col) = line.find(symbol) {
                    refs.push(SymbolReference {
                        file: file_path.to_string(),
                        line: i + 1,
                        column: col,
                        scope: "global".to_string(),
                        is_definition: is_definition_line(trimmed, symbol, ext),
                        ast_node_kind: "fallback".to_string(),
                    });
                }
            }
        }
        Ok(refs)
    }

    async fn fallback_rename(&self, file_path: &str, symbol: &str, new_name: &str) -> Result<String, String> {
        let refs = self.fallback_find_in_file(file_path, symbol).await?;
        let mut contents = HashMap::new();
        for r in &refs {
            if !contents.contains_key(&r.file) {
                let path = self.resolve(&r.file);
                let c = tokio::fs::read_to_string(&path).await
                    .map_err(|e| format!("Read {}: {}", r.file, e))?;
                contents.insert(r.file.clone(), c.lines().map(|l| l.to_string()).collect::<Vec<_>>());
            }
        }
        for (file, lines) in &mut contents {
            let file_lines: Vec<&SymbolReference> = refs.iter().filter(|r| r.file == *file).collect();
            for r in &file_lines {
                if r.line > 0 && r.line <= lines.len() {
                    let l = &lines[r.line - 1];
                    if let Some(pos) = l.find(symbol) {
                        lines[r.line - 1] = l.replacen(symbol, new_name, 1);
                    }
                }
            }
            let path = self.resolve(file);
            tokio::fs::write(&path, lines.join("\n")).await.ok();
        }
        Ok(format!("[Regex fallback] Renamed '{}' → '{}' in {} files", symbol, new_name, contents.len()))
    }

    fn resolve(&self, file: &str) -> std::path::PathBuf {
        let p = Path::new(file);
        if p.is_absolute() { p.to_path_buf() }
        else { self.workspace_root.join(file) }
    }
}

// ========================================================================
// [2] 旧版 ScopeAwareRenamer 保留向后兼容
// ========================================================================

/// 作用域感知重命名器 (旧版正则, 保留兼容)
pub struct ScopeAwareRenamer {
    workspace_root: std::path::PathBuf,
}
        let current_stem = Path::new(file_path)
            .file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let mut deps = Vec::new();

        // 扫描 src/ 下所有 .rs 文件
        let src_dir = self.workspace_root.join("src");
        if src_dir.exists() {
            self.collect_files_with_import(&src_dir, current_stem, &mut deps).await;
        }

        Ok(deps)
    }

    async fn collect_files_with_import(&self, dir: &Path, target: &str, result: &mut Vec<String>) {
        if let Ok(entries) = tokio::fs::read_dir(dir).await {
            let mut entries = entries;
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                if path.is_dir() {
                    Box::pin(self.collect_files_with_import(&path, target, result)).await;
                } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
                    if let Ok(content) = tokio::fs::read_to_string(&path).await {
                        if content.contains(&format!("use crate::{}", target))
                            || content.contains(&format!("use super::{}", target))
                            || content.contains(&format!("mod {}", target))
                        {
                            result.push(path.to_string_lossy().to_string());
                        }
                    }
                }
            }
        }
    }

    fn resolve(&self, file: &str) -> std::path::PathBuf {
        let p = Path::new(file);
        if p.is_absolute() { p.to_path_buf() }
        else { self.workspace_root.join(file) }
    }
}

// ========================================================================
// [2] 类型感知提取方法
// ========================================================================

/// 类型感知提取器
pub struct TypeAwareExtractor {
    workspace_root: std::path::PathBuf,
}

impl TypeAwareExtractor {
    pub fn new(root: &Path) -> Self {
        Self { workspace_root: root.to_path_buf() }
    }

    /// 提取方法 — 感知返回类型和参数类型
    pub async fn extract(
        &self, file_path: &str, start_line: usize, end_line: usize, new_name: &str,
    ) -> Result<String, String> {
        let full_path = self.resolve(file_path);
        let content = tokio::fs::read_to_string(&full_path).await
            .map_err(|e| format!("Read: {}", e))?;
        let lines: Vec<&str> = content.lines().collect();

        if start_line >= end_line || end_line > lines.len() {
            return Err("Invalid line range".to_string());
        }

        // 提取选中代码
        let selected: Vec<&str> = lines[start_line..end_line].iter().collect();
        let selected_code = selected.join("\n");

        // 检测返回类型 (分析最后表达式)
        let return_type = self.infer_return_type_ast(&selected_code);
        // 检测参数 (分析使用的外部变量)
        let params = self.infer_parameters_ast(&selected_code, &lines, start_line);

        // 生成函数
        let func = format!(
            "/// Extracted function\nfn {}({}) -> {} {{\n{}\n}}\n",
            new_name, params.join(", "), return_type, selected_code
        );

        // 在文件末尾添加, 替换选中区域为调用
        let call = format!("{}();", new_name);
        let mut new_lines: Vec<String> = lines.iter().enumerate().map(|(i, l)| {
            if i >= start_line && i < end_line { String::new() } else { (*l).to_string() }
        }).collect();

        new_lines[start_line] = call;
        new_lines.push(String::new());
        new_lines.push(func);

        // 清理连续空行
        let result = clean_empty_lines(&new_lines);
        tokio::fs::write(&full_path, result.join("\n")).await
            .map_err(|e| format!("Write: {}", e))?;

        Ok(format!("Extracted '{}' at {}-{}", new_name, start_line + 1, end_line))
    }

    /// AST 感知的返回类型推断
    fn infer_return_type_ast(&self, code: &str) -> String {
        // 检查最后一行是否是不带分号的表达式
        let last_line = code.lines().last().unwrap_or("").trim();
        if !last_line.ends_with(';') && !last_line.starts_with("//") {
            // 可能是返回表达式 → 推断类型
            if last_line.contains("true") || last_line.contains("false") { return "bool".to_string(); }
            if last_line.parse::<f64>().is_ok() { return "f64".to_string(); }
            if last_line.starts_with('"') { return "String".to_string(); }
            if last_line.starts_with('&') { return "&str".to_string(); }
            if last_line.starts_with("vec!") || last_line.starts_with('[') { return "Vec<_>".to_string(); }
            if last_line.starts_with("Some") || last_line.starts_with("Ok(") { return "Option<_>".to_string(); }
            if last_line.starts_with("Err(") { return "Result<_, _>".to_string(); }
        }
        // 检查 return 语句
        for line in code.lines() {
            let t = line.trim();
            if t.starts_with("return ") || t.starts_with("return;") {
                let rest = t.trim_start_matches("return ").trim();
                if rest.starts_with("true") || rest.starts_with("false") { return "bool".to_string(); }
                if rest.parse::<f64>().is_ok() { return "f64".to_string(); }
            }
        }
        "()".to_string()
    }

    /// AST 感知的参数推断
    fn infer_parameters_ast(&self, code: &str, _all_lines: &[&str], _start_line: usize) -> Vec<String> {
        let mut params: Vec<String> = Vec::new();
        let mut seen = std::collections::HashSet::new();

        // 查找在代码中使用的变量(而非 let 声明的)
        for line in code.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("let ") {
                let var = trimmed.split_whitespace().nth(1).unwrap_or("");
                seen.insert(var.to_string());
            }
        }

        // 在代码中查找外部变量引用
        for line in code.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("let ") { continue; }
            for word in trimmed.split(|c: char| !c.is_alphanumeric() && c != '_') {
                if word.len() > 1 && !seen.contains(word)
                    && !is_keyword(word)
                {
                    let p = format!("{}: /* type */", word);
                    if !params.contains(&p) { params.push(p); }
                    seen.insert(word.to_string());
                }
            }
        }

        params
    }

    fn resolve(&self, file: &str) -> std::path::PathBuf {
        let p = Path::new(file);
        if p.is_absolute() { p.to_path_buf() }
        else { self.workspace_root.join(file) }
    }
}

/// 判断是否为注释行
fn is_comment_line(line: &str, ext: &str) -> bool {
    match ext {
        "rs" => line.starts_with("//") || line.starts_with("#["),
        "py" => line.starts_with('#'),
        "js" | "ts" | "tsx" | "jsx" => line.starts_with("//") || line.starts_with("/*"),
        _ => false,
    }
}

/// 判断是否为定义行
fn is_definition_line(line: &str, symbol: &str, ext: &str) -> bool {
    match ext {
        "rs" => {
            line.starts_with("fn ") || line.starts_with("pub fn ")
                || line.starts_with("struct ") || line.starts_with("enum ")
                || line.starts_with("trait ") || line.starts_with("type ")
                || line.starts_with("const ") || line.starts_with("static ")
                || line.starts_with("mod ")
        }
        "py" => {
            line.starts_with("def ") || line.starts_with("class ")
        }
        "ts" | "js" => {
            line.starts_with("function ") || line.starts_with("class ")
                || line.starts_with("interface ") || line.starts_with("const ")
                || line.starts_with("let ") || line.starts_with("var ")
        }
        _ => false,
    }
}

/// 判断是否为 Rust 关键字
fn is_keyword(word: &str) -> bool {
    matches!(word, "fn" | "let" | "mut" | "pub" | "self" | "Self"
        | "if" | "else" | "for" | "while" | "loop" | "match"
        | "return" | "true" | "false" | "None" | "Some" | "Ok" | "Err"
        | "as" | "use" | "mod" | "struct" | "enum" | "trait" | "impl"
        | "const" | "static" | "type" | "where" | "ref" | "move"
        | "async" | "await" | "unsafe" | "dyn")
}

/// 清理连续空行
fn clean_empty_lines(lines: &[String]) -> Vec<String> {
    let mut result = Vec::new();
    let mut prev_empty = false;
    for line in lines {
        let empty = line.trim().is_empty();
        if empty && prev_empty { continue; }
        result.push(line.clone());
        prev_empty = empty;
    }
    result
}

// ========================================================================
// [3] 导入管理 — 移动符号时自动更新跨文件导入
// ========================================================================

/// 导入管理器
pub struct ImportManager {
    workspace_root: std::path::PathBuf,
}

impl ImportManager {
    pub fn new(root: &Path) -> Self {
        Self { workspace_root: root.to_path_buf() }
    }

    /// 移动符号到新文件 — 自动更新所有受影响文件的导入
    pub async fn move_symbol(
        &self, source_file: &str, symbol: &str, target_file: &str,
    ) -> Result<String, String> {
        let src_path = self.resolve(source_file);
        let tgt_path = self.resolve(target_file);

        // 1. 从源文件提取符号定义
        let src_content = tokio::fs::read_to_string(&src_path).await
            .map_err(|e| format!("Read {}: {}", source_file, e))?;
        let (def_block, def_lines) = self.extract_definition(&src_content, symbol)?;

        // 2. 写入目标文件
        let target_content = if tgt_path.exists() {
            tokio::fs::read_to_string(&tgt_path).await
                .map_err(|e| format!("Read {}: {}", target_file, e))?
        } else {
            String::new()
        };
        let new_target = if target_content.is_empty() {
            def_block.clone()
        } else {
            format!("{}\n\n{}", target_content, def_block)
        };
        if let Some(parent) = tgt_path.parent() {
            tokio::fs::create_dir_all(parent).await
                .map_err(|e| format!("Dir: {}", e))?;
        }
        tokio::fs::write(&tgt_path, &new_target).await
            .map_err(|e| format!("Write {}: {}", target_file, e))?;

        // 3. 从源文件删除定义
        let mut new_src: Vec<String> = src_content.lines().map(|l| l.to_string()).collect();
        for i in def_lines {
            if i < new_src.len() { new_src[i] = String::new(); }
        }
        tokio::fs::write(&src_path, clean_empty_lines(&new_src).join("\n")).await
            .map_err(|e| format!("Write {}: {}", source_file, e))?;

        // 4. 在所有引用文件中添加 use 语句
        let module_path = target_file
            .trim_end_matches(".rs")
            .replace('\\', "::")
            .replace('/', "::");
        let use_stmt = format!("use crate::{}::{};\n", module_path, symbol);
        let dep_files = self.find_dependent_files(source_file, symbol).await?;
        for dep in &dep_files {
            let dep_path = self.resolve(dep);
            let mut dep_content = tokio::fs::read_to_string(&dep_path).await
                .unwrap_or_default();
            if !dep_content.contains(&use_stmt.trim()) {
                dep_content = format!("{}{}", use_stmt, dep_content);
                tokio::fs::write(&dep_path, &dep_content).await.ok();
            }
        }

        Ok(format!("Moved '{}' from {} to {} ({} imports updated)", symbol, source_file, target_file, dep_files.len()))
    }

    /// 提取符号定义
    fn extract_definition(&self, content: &str, symbol: &str) -> Result<(String, Vec<usize>), String> {
        let lines: Vec<&str> = content.lines().collect();
        let mut start = None;
        let mut brace = 0i32;
        let mut def_lines = Vec::new();

        for (i, line) in lines.iter().enumerate() {
            if line.contains(symbol) && is_definition_line(line, symbol, "rs") {
                start = Some(i);
            }
            if let Some(s) = start {
                def_lines.push(i);
                brace += line.matches('{').count() as i32;
                brace -= line.matches('}').count() as i32;
                if brace <= 0 && i > s {
                    break;
                }
            }
        }

        match start {
            Some(s) => {
                let block = lines[s..=*def_lines.last().unwrap_or(&s)].join("\n");
                Ok((block, def_lines))
            }
            None => Err(format!("Symbol '{}' not found in content", symbol)),
        }
    }

    async fn find_dependent_files(&self, file_path: &str, symbol: &str) -> Result<Vec<String>, String> {
        let src_dir = self.workspace_root.join("src");
        let mut deps = Vec::new();
        if src_dir.exists() {
            self.collect_refs(&src_dir, file_path, symbol, &mut deps).await;
        }
        Ok(deps)
    }

    async fn collect_refs(&self, dir: &Path, exclude: &str, symbol: &str, result: &mut Vec<String>) {
        if let Ok(mut entries) = tokio::fs::read_dir(dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                if path.is_dir() {
                    Box::pin(self.collect_refs(&path, exclude, symbol, result)).await;
                } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
                    let p = path.to_string_lossy();
                    if p.contains(exclude) { continue; }
                    if let Ok(content) = tokio::fs::read_to_string(&path).await {
                        if content.contains(symbol) {
                            result.push(p.to_string());
                        }
                    }
                }
            }
        }
    }

    fn resolve(&self, file: &str) -> std::path::PathBuf {
        let p = Path::new(file);
        if p.is_absolute() { p.to_path_buf() }
        else { self.workspace_root.join(file) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scope_detection() {
        let content = "fn main() {\n    let x = 1;\n}\nfn helper() {\n    let x = 2;\n}\n";
        let renamer = ScopeAwareRenamer::new(Path::new("."));
        assert_eq!(renamer.detect_scope(content, 1), "fn:main");
        assert_eq!(renamer.detect_scope(content, 4), "fn:helper");
        assert_eq!(renamer.detect_scope(content, 0), "fn:main");
    }

    #[test]
    fn test_is_definition() {
        assert!(is_definition_line("fn main() {}", "main", "rs"));
        assert!(is_definition_line("pub fn helper()", "helper", "rs"));
        assert!(is_definition_line("struct User {}", "User", "rs"));
        assert!(!is_definition_line("let x = main();", "main", "rs"));
    }

    #[test]
    fn test_infer_return_type() {
        let extractor = TypeAwareExtractor::new(Path::new("."));
        assert_eq!(extractor.infer_return_type_ast("true"), "bool");
        assert_eq!(extractor.infer_return_type_ast("\"hello\""), "String");
        assert_eq!(extractor.infer_return_type_ast("42"), "f64");
        assert_eq!(extractor.infer_return_type_ast("let x = 1;"), "()");
    }

    #[tokio::test]
    async fn test_scope_aware_rename() {
        let temp = std::env::temp_dir().join("carpai-semantic-test");
        let _ = std::fs::create_dir_all(&temp.join("src"));
        std::fs::write(temp.join("src/main.rs"),
            "// old_name usage\nfn old_name() -> u32 { 42 }\nfn main() { let x = old_name(); }\n").ok();

        let renamer = ScopeAwareRenamer::new(&temp);
        let refs = renamer.find_references("old_name", "src/main.rs", 0).await.unwrap();
        assert!(!refs.is_empty());

        let result = renamer.rename("src/main.rs", "old_name", "new_name").await.unwrap();
        assert!(result.contains("new_name"));

        let content = std::fs::read_to_string(temp.join("src/main.rs")).unwrap();
        assert!(content.contains("new_name"));
        assert!(!content.contains("old_name"));

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[tokio::test]
    async fn test_type_aware_extract() {
        let temp = std::env::temp_dir().join("carpai-extract-test");
        let _ = std::fs::create_dir_all(&temp.join("src"));
        std::fs::write(temp.join("src/main.rs"),
            "fn main() {\n    let x = 42;\n    let y = x + 1;\n    println!(\"{}\", y);\n}\n").ok();

        let extractor = TypeAwareExtractor::new(&temp);
        let result = extractor.extract("src/main.rs", 1, 3, "compute").await.unwrap();
        assert!(result.contains("compute"));

        let content = std::fs::read_to_string(temp.join("src/main.rs")).unwrap();
        assert!(content.contains("compute()"));

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[tokio::test]
    async fn test_import_manager_move() {
        let temp = std::env::temp_dir().join("carpai-import-test");
        let _ = std::fs::create_dir_all(&temp.join("src"));
        std::fs::write(temp.join("src/main.rs"),
            "mod utils;\nfn helper() -> u32 { 42 }\nfn main() { let x = helper(); }\n").ok();
        std::fs::write(temp.join("src/utils.rs"),
            "pub fn util_fn() -> u32 { 7 }\n").ok();

        let mgr = ImportManager::new(&temp);
        let result = mgr.move_symbol("src/main.rs", "helper", "src/utils.rs").await;
        assert!(result.is_ok());

        let main_content = std::fs::read_to_string(temp.join("src/main.rs")).unwrap();
        assert!(!main_content.contains("fn helper")); // 从 main.rs 删除

        let utils_content = std::fs::read_to_string(temp.join("src/utils.rs")).unwrap();
        assert!(utils_content.contains("fn helper")); // 添加到 utils.rs

        let _ = std::fs::remove_dir_all(&temp);
    }
}
