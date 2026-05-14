//! AST (Abstract Syntax Tree) 解析模块
//!
//! 提供 Tree-sitter 增量式解析能力：
//! - **多语言支持** - Rust/Python/JS/TS/Go 等
//! - **增量解析** - 只重新解析修改部分
//! - **符号提取** - 函数/结构体/枚举等
//! - **调用图** - 函数调用关系分析
//! - **错误恢复** - 语法错误时仍能继续

pub mod tree_sitter;

pub use tree_sitter::{
    AstParser, CodeAnalyzer, FileAnalysis, NodeType, ParserConfig,
    ProjectAnalysis, SupportedLanguage, SymbolInfo, Visibility,
};
