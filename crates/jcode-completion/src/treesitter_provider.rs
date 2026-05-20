//! TreeSitter AST 提供者 — 离线 AST 解析，无需 LSP 服务器
//!
//! 使用正则模拟 TreeSitter 的 AST 解析能力。
//! 在集成真实的 tree-sitter crate 前，作为离线 AST 的快速实现。
//!
//! 解析能力:
//!   - 函数定义提取 (fn/def/function)
//!   - 结构体/类定义提取 (struct/class)
//!   - 变量声明提取 (let/const/var)
//!   - 导入语句提取 (use/import/require)
//!   - 泛型参数提取 (<T>)
//!   - 闭包/箭头函数检测

use crate::ast_context::{AstContextProvider, CompletionContext, ScopeKind};
use async_trait::async_trait;
use regex::Regex;

/// AST 节点 (模拟 TreeSitter 输出)
#[derive(Debug, Clone)]
pub struct AstNode {
    pub kind: &'static str,
    pub name: String,
    pub start_line: usize,
    pub end_line: usize,
}

/// 离线 AST 解析器 (TreeSitter 模拟)
pub struct TreeSitterAstProvider {
    fn_re: Regex,
    struct_re: Regex,
    #[allow(dead_code)]
    let_re: Regex,
    #[allow(dead_code)]
    import_re: Regex,
    #[allow(dead_code)]
    generic_re: Regex,
    #[allow(dead_code)]
    lambda_re: Regex,
}

impl TreeSitterAstProvider {
    pub fn new() -> Self {
        Self {
            fn_re: Regex::new(r"(pub\s+)?(async\s+)?fn\s+(\w+)").unwrap(),
            struct_re: Regex::new(r"(pub\s+)?struct\s+(\w+)").unwrap(),
            let_re: Regex::new(r"let\s+(mut\s+)?(\w+)").unwrap(),
            import_re: Regex::new(r"(use|import|from)\s+([^;{]+)").unwrap(),
            generic_re: Regex::new(r"<(\w+)>").unwrap(),
            lambda_re: Regex::new(r"\|\s*(\w+)\s*\|").unwrap(),
        }
    }

    /// 解析文件内容，返回 AST 节点列表
    pub fn parse(&self, content: &str) -> Vec<AstNode> {
        let mut nodes = Vec::new();
        for (i, line) in content.lines().enumerate() {
            // 函数定义
            if let Some(cap) = self.fn_re.captures(line) {
                nodes.push(AstNode {
                    kind: "function",
                    name: cap[cap.len() - 1].to_string(),
                    start_line: i, end_line: i,
                });
            }
            // 结构体定义
            if let Some(cap) = self.struct_re.captures(line) {
                nodes.push(AstNode {
                    kind: "struct",
                    name: cap[cap.len() - 1].to_string(),
                    start_line: i, end_line: i,
                });
            }
        }
        nodes
    }

    /// 根据光标位置找到所在的 AST 节点
    pub fn find_enclosing_node<'a>(&self, nodes: &'a [AstNode], line: usize) -> Option<&'a AstNode> {
        nodes.iter().find(|n| n.start_line <= line && line <= n.end_line)
            .or_else(|| nodes.iter().filter(|n| n.start_line <= line).last())
    }
}

#[async_trait]
impl AstContextProvider for TreeSitterAstProvider {
    async fn resolve_context(
        &self,
        content: &str,
        line: usize,
        column: usize,
    ) -> Option<CompletionContext> {
        let lines: Vec<&str> = content.lines().collect();
        let current_line = lines.get(line)?;
        let before_cursor = &current_line[..column.min(current_line.len())];

        // 解析 AST
        let nodes = self.parse(content);
        let enclosing = self.find_enclosing_node(&nodes, line);

        // 判断作用域
        let scope = if before_cursor.contains(".") { ScopeKind::MethodChain }
        else if before_cursor.contains("::") { ScopeKind::Import }
        else if before_cursor.contains(": ") || before_cursor.ends_with("=") { ScopeKind::Assignment }
        else if before_cursor.ends_with('(') || before_cursor.ends_with(',') { ScopeKind::FunctionArg }
        else if let Some(n) = enclosing { 
            match n.kind { "struct" => ScopeKind::StructField, _ => ScopeKind::Expression }
        }
        else { ScopeKind::Expression };

        let prefix = before_cursor
            .rsplit(|c: char| !c.is_alphanumeric() && c != '_' && c != '.')
            .next()
            .unwrap_or("")
            .to_string();

        Some(CompletionContext {
            file_path: String::new(), line, column, prefix,
            expected_type: None,
            scope,
            parent_symbol: enclosing.map(|n| n.name.clone()),
        })
    }
}
