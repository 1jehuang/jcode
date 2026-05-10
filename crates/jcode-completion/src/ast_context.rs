use async_trait::async_trait;

/// AST 上下文 — 光标位置处编译器期望什么
#[derive(Debug, Clone)]
pub struct CompletionContext {
    pub file_path: String,
    pub line: usize,
    pub column: usize,
    pub prefix: String,
    pub expected_type: Option<String>,
    pub scope: ScopeKind,
    pub parent_symbol: Option<String>,
}

/// 光标所在的作用域类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeKind {
    /// 结构体字段内
    StructField,
    /// 函数调用参数内
    FunctionArg,
    /// 赋值表达式右侧
    Assignment,
    /// 方法链调用
    MethodChain,
    /// 导入语句内
    Import,
    /// 泛型参数内
    GenericParam,
    /// 普通表达式
    Expression,
}

/// AST 上下文提供者 trait
#[async_trait]
pub trait AstContextProvider: Send + Sync {
    async fn resolve_context(
        &self,
        content: &str,
        line: usize,
        column: usize,
    ) -> Option<CompletionContext>;
}

/// 基于正则的默认实现 (保底层)
pub struct RegexAstProvider;

impl RegexAstProvider {
    pub fn new() -> Self { Self }

    /// 通过正则推断光标位置的上下文
    fn infer_context(&self, content: &str, line: usize, column: usize) -> Option<CompletionContext> {
        let lines: Vec<&str> = content.lines().collect();
        let current_line = lines.get(line)?;
        let before_cursor = &current_line[..column.min(current_line.len())];
        let _after_cursor = &current_line[column.min(current_line.len())..];

        // 判断前缀（光标前最后一个词）
        let prefix = before_cursor
            .rsplit(|c: char| !c.is_alphanumeric() && c != '_' && c != '.')
            .next()
            .unwrap_or("")
            .to_string();

        // 判断作用域
        let scope = if before_cursor.contains(".") {
            ScopeKind::MethodChain
        } else if before_cursor.contains("::") {
            ScopeKind::Import
        } else if before_cursor.contains(": ") || before_cursor.ends_with("=") {
            ScopeKind::Assignment
        } else if before_cursor.ends_with('(') || before_cursor.ends_with(',') {
            ScopeKind::FunctionArg
        } else if content[..content.find(current_line).unwrap_or(0)].contains("struct ") {
            ScopeKind::StructField
        } else if before_cursor.contains('<') {
            ScopeKind::GenericParam
        } else {
            ScopeKind::Expression
        };

        Some(CompletionContext {
            file_path: String::new(),
            line, column,
            prefix,
            expected_type: None,
            scope,
            parent_symbol: None,
        })
    }
}

#[async_trait]
impl AstContextProvider for RegexAstProvider {
    async fn resolve_context(
        &self,
        content: &str,
        line: usize,
        column: usize,
    ) -> Option<CompletionContext> {
        self.infer_context(content, line, column)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_method_chain() {
        let provider = RegexAstProvider::new();
        let content = "let x = vec![1,2,3].\nfn main() {}";
        let ctx = provider.resolve_context(content, 0, 19).await.unwrap();
        assert_eq!(ctx.scope, ScopeKind::MethodChain);
        assert_eq!(ctx.prefix, "");
    }

    #[tokio::test]
    async fn test_assignment() {
        let provider = RegexAstProvider::new();
        let content = "let name: String = ";
        let ctx = provider.resolve_context(content, 0, 19).await.unwrap();
        assert_eq!(ctx.scope, ScopeKind::Assignment);
    }
}
