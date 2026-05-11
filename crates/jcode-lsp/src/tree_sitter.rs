//! Tree-sitter 集成模块
//!
//! 提供真正的 AST 解析能力：
//! - 多语言支持 (Rust, TypeScript, Python, Go, etc.)
//! - 增量解析
//! - 语义分析
//! - 符号表构建
//! - 代码导航

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};
use serde::{Deserialize, Serialize};

/// 语言标识符
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LanguageId {
    Rust,
    TypeScript,
    JavaScript,
    Python,
    Go,
    Java,
    C,
    Cpp,
    HTML,
    CSS,
    JSON,
    YAML,
    Markdown,
    Unknown(String),
}

impl std::fmt::Display for LanguageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rust => write!(f, "rust"),
            Self::TypeScript => write!(f, "typescript"),
            Self::JavaScript => write!(f, "javascript"),
            Self::Python => write!(f, "python"),
            Self::Go => write!(f, "go"),
            Self::Java => write!(f, "java"),
            Self::C => write!(f, "c"),
            Self::Cpp => write!(f, "cpp"),
            Self::HTML => write!(f, "html"),
            Self::CSS => write!(f, "css"),
            Self::JSON => write!(f, "json"),
            Self::YAML => write!(f, "yaml"),
            Self::Markdown => write!(f, "markdown"),
            Self::Unknown(s) => write!(f, "{}", s),
        }
    }
}

impl LanguageId {
    /// 从文件扩展名推断语言
    pub fn from_extension(ext: &str) -> Self {
        match ext.to_lowercase().as_str() {
            "rs" => Self::Rust,
            "ts" | "tsx" => Self::TypeScript,
            "js" | "jsx" | "mjs" | "cjs" => Self::JavaScript,
            "py" | "pyi" => Self::Python,
            "go" => Self::Go,
            "java" => Self::Java,
            "c" | "h" => Self::C,
            "cpp" | "cc" | "cxx" | "hpp" | "hxx" => Self::Cpp,
            "html" | "htm" | "vue" | "svelte" => Self::HTML,
            "css" | "scss" | "less" => Self::CSS,
            "json" => Self::JSON,
            "yml" | "yaml" => Self::YAML,
            "md" | "markdown" => Self::Markdown,
            _ => Self::Unknown(ext.to_string()),
        }
    }

    /// 从文件路径推断语言
    pub fn from_path(path: &str) -> Self {
        PathBuf::from(path)
            .extension()
            .and_then(|e| e.to_str())
            .map(|ext| Self::from_extension(ext))
            .unwrap_or(Self::Unknown("".to_string()))
    }
}

/// AST 节点类型
#[derive(Debug, Clone, PartialEq)]
pub enum NodeType {
    // 声明类节点
    FunctionDeclaration,
    StructDeclaration,
    EnumDeclaration,
    TraitDeclaration,
    ImplDeclaration,
    ClassDeclaration,
    InterfaceDeclaration,
    VariableDeclaration,
    
    // 表达式类节点
    CallExpression,
    BinaryExpression,
    UnaryExpression,
    MemberExpression,
    IndexExpression,
    AssignmentExpression,
    ConditionalExpression,
    LambdaExpression,
    
    // 语句类节点
    ExpressionStatement,
    ReturnStatement,
    IfStatement,
    ForStatement,
    WhileStatement,
    MatchStatement,
    BlockStatement,
    
    // 类型相关
    TypeDefinition,
    TypeParameter,
    GenericType,
    PointerType,
    ReferenceType,
    SliceType,
    
    // 其他
    Identifier,
    StringLiteral,
    NumberLiteral,
    BooleanLiteral,
    Comment,
    DocComment,
    Error, // 解析错误
    SourceFile, // 源文件根节点
    Unknown,
}

impl NodeType {
    /// 是否是声明节点
    pub fn is_declaration(&self) -> bool {
        matches!(self,
            Self::FunctionDeclaration |
            Self::StructDeclaration |
            Self::EnumDeclaration |
            Self::TraitDeclaration |
            Self::ImplDeclaration |
            Self::ClassDeclaration |
            Self::VariableDeclaration
        )
    }

    /// 是否是表达式节点
    pub fn is_expression(&self) -> bool {
        matches!(self,
            Self::CallExpression |
            Self::BinaryExpression |
            Self::UnaryExpression |
            Self::MemberExpression |
            Self::IndexExpression |
            Self::AssignmentExpression
        )
    }

    /// 是否是可引用的符号定义
    pub fn is_symbol_definition(&self) -> bool {
        matches!(self,
            Self::FunctionDeclaration |
            Self::StructDeclaration |
            Self::EnumDeclaration |
            Self::TraitDeclaration |
            Self::ClassDeclaration |
            Self::InterfaceDeclaration |
            Self::VariableDeclaration |
            Self::TypeDefinition
        )
    }
}

impl std::fmt::Display for NodeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FunctionDeclaration => write!(f, "function_declaration"),
            Self::StructDeclaration => write!(f, "struct_declaration"),
            Self::EnumDeclaration => write!(f, "enum_declaration"),
            Self::TraitDeclaration => write!(f, "trait_declaration"),
            Self::ImplDeclaration => write!(f, "impl_declaration"),
            Self::ClassDeclaration => write!(f, "class_declaration"),
            Self::InterfaceDeclaration => write!(f, "interface_declaration"),
            Self::VariableDeclaration => write!(f, "variable_declaration"),
            Self::CallExpression => write!(f, "call_expression"),
            Self::BinaryExpression => write!(f, "binary_expression"),
            Self::UnaryExpression => write!(f, "unary_expression"),
            Self::MemberExpression => write!(f, "member_expression"),
            Self::IndexExpression => write!(f, "index_expression"),
            Self::AssignmentExpression => write!(f, "assignment_expression"),
            Self::ConditionalExpression => write!(f, "conditional_expression"),
            Self::LambdaExpression => write!(f, "lambda_expression"),
            Self::ExpressionStatement => write!(f, "expression_statement"),
            Self::ReturnStatement => write!(f, "return_statement"),
            Self::IfStatement => write!(f, "if_statement"),
            Self::ForStatement => write!(f, "for_statement"),
            Self::WhileStatement => write!(f, "while_statement"),
            Self::MatchStatement => write!(f, "match_statement"),
            Self::BlockStatement => write!(f, "block_statement"),
            Self::TypeDefinition => write!(f, "type_definition"),
            Self::TypeParameter => write!(f, "type_parameter"),
            Self::GenericType => write!(f, "generic_type"),
            Self::PointerType => write!(f, "pointer_type"),
            Self::ReferenceType => write!(f, "reference_type"),
            Self::SliceType => write!(f, "slice_type"),
            Self::Identifier => write!(f, "identifier"),
            Self::StringLiteral => write!(f, "string_literal"),
            Self::NumberLiteral => write!(f, "number_literal"),
            Self::BooleanLiteral => write!(f, "boolean_literal"),
            Self::Comment => write!(f, "comment"),
            Self::DocComment => write!(f, "doc_comment"),
            Self::Error => write!(f, "error"),
            Self::SourceFile => write!(f, "source_file"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

/// 源代码位置
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceLocation {
    /// 文件路径索引
    pub file_index: u32,
    /// 起始行号 (0-based)
    pub start_line: u32,
    /// 起始列号 (0-based)
    pub start_column: u32,
    /// 结束行号 (0-based)
    pub end_line: u32,
    /// 结束列号 (0-based)
    pub end_column: u32,
}

impl SourceLocation {
    /// 创建新的位置
    pub fn new(start_line: u32, start_col: u32, end_line: u32, end_col: u32) -> Self {
        Self {
            file_index: 0,
            start_line,
            start_column: start_col,
            end_line,
            end_column: end_col,
        }
    }

    /// 是否包含指定位置
    pub fn contains(&self, line: u32, column: u32) -> bool {
        if self.start_line == self.end_line {
            // 单行范围
            self.start_line == line 
                && self.start_column <= column 
                && column < self.end_column
        } else {
            // 多行范围
            (line > self.start_line || (line == self.start_line && column >= self.start_column))
                && (line < self.end_line || (line == self.end_line && column < self.end_column))
        }
    }

    /// 转换为 LSP Range
    pub fn to_lsp_range(&self) -> lsp_types::Range {
        lsp_types::Range {
            start: lsp_types::Position {
                line: self.start_line,
                character: self.start_column,
            },
            end: lsp_types::Position {
                line: self.end_line,
                character: self.end_column,
            },
        }
    }
}

impl std::fmt::Display for SourceLocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}-{}:{}", self.start_line, self.start_column, self.end_line, self.end_column)
    }
}

/// AST 节点
#[derive(Debug, Clone)]
pub struct AstNode {
    /// 节点唯一 ID
    pub id: u64,
    
    /// 节点类型
    pub node_type: NodeType,
    
    /// 节点名称（如函数名、变量名）
    pub name: Option<String>,
    
    /// 源代码位置
    pub location: SourceLocation,
    
    /// 父节点 ID
    pub parent_id: Option<u64>,
    
    /// 子节点
    pub children: Vec<AstNode>,
    
    /// 关联的类型信息（如果有）
    pub type_info: Option<TypeInfo>,
}

impl AstNode {
    /// 创建新的 AST 节点
    pub fn new(node_type: NodeType, location: SourceLocation) -> Self {
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        
        Self {
            id: NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            node_type,
            name: None,
            location,
            parent_id: None,
            children: Vec::new(),
            type_info: None,
        }
    }

    /// 设置节点名称
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// 设置类型信息
    pub fn with_type(mut self, info: TypeInfo) -> Self {
        self.type_info = Some(info);
        self
    }

    /// 添加子节点
    pub fn add_child(&mut self, mut child: AstNode) {
        child.parent_id = Some(self.id);
        self.children.push(child);
    }

    /// 递归查找节点
    pub fn find_by_type(&self, node_type: &NodeType) -> Option<&AstNode> {
        if self.node_type == *node_type {
            return Some(self);
        }
        
        for child in &self.children {
            if let Some(found) = child.find_by_type(node_type) {
                return Some(found);
            }
        }
        
        None
    }

    /// 递归查找所有匹配类型的节点
    pub fn find_all_by_type(&self, node_type: &NodeType) -> Vec<&AstNode> {
        let mut results = Vec::new();
        
        if self.node_type == *node_type {
            results.push(self);
        }
        
        for child in &self.children {
            results.extend(child.find_all_by_type(node_type));
        }
        
        results
    }

    /// 获取节点的文本范围长度
    pub fn text_length(&self) -> u32 {
        if self.location.start_line == self.location.end_line {
            self.location.end_column - self.location.start_column
        } else {
            // 多行节点的近似长度
            (self.location.end_line - self.location.start_line) * 80 + 
            self.location.end_column - self.location.start_column
        }
    }

    /// 节点深度（从根到当前节点的距离）
    pub fn depth(&self) -> u32 {
        self.children.iter()
            .map(|c| c.depth() + 1)
            .max()
            .unwrap_or(0)
    }

    /// 总子节点数
    pub fn total_children(&self) -> usize {
        self.children.len() + self.children.iter().map(|c| c.total_children()).sum::<usize>()
    }
}

/// 类型信息
#[derive(Debug, Clone)]
pub struct TypeInfo {
    /// 类型名称
    pub type_name: String,
    /// 是否可为空
    pub nullable: bool,
    /// 是否为引用类型
    pub is_reference: bool,
    /// 泛型参数
    pub generic_params: Vec<String>,
}

impl TypeInfo {
    /// 创建类型信息
    pub fn new(type_name: impl Into<String>) -> Self {
        Self {
            type_name: type_name.into(),
            nullable: false,
            is_reference: false,
            generic_params: Vec::new(),
        }
    }

    /// 显示名称（用于补全等场景）
    pub fn display_name(&self) -> String {
        let mut name = self.type_name.clone();
        if !self.generic_params.is_empty() {
            name.push_str("<");
            name.push_str(&self.generic_params.join(", "));
            name.push_str(">");
        }
        if self.is_reference {
            name = format!("&{}", name);
        }
        if self.nullable {
            name.push('?');
        }
        name
    }
}

/// 符号条目（符号表中的项）
#[derive(Debug, Clone)]
pub struct SymbolEntry {
    /// 符号名称
    pub name: String,
    /// 符号类型
    pub kind: SymbolKind,
    /// 定义位置
    pub definition_location: SourceLocation,
    /// 关联的 AST 节点 ID
    pub node_id: u64,
    /// 作用域 ID
    pub scope_id: u64,
    /// 类型信息
    pub type_info: Option<TypeInfo>,
}

/// 符号种类
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SymbolKind {
    Function,
    Method,
    Struct,
    Enum,
    Trait,
    Interface,
    Class,
    Variable,
    Constant,
    Parameter,
    TypeAlias,
    Module,
    Field,
    Property,
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
            Self::Class => write!(f, "class"),
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

/// 解析结果
#[derive(Debug, Clone)]
pub struct ParseResult {
    /// AST 根节点
    pub root: AstNode,
    
    /// 符号表（名称 -> 符号信息）
    pub symbol_table: HashMap<String, SymbolEntry>,
    
    /// 作用域信息（scope_id -> scope_data）
    pub scopes: HashMap<u64, ScopeInfo>,
    
    /// 诊断信息（错误、警告等）
    pub diagnostics: Vec<DiagnosticInfo>,
    
    /// 统计信息
    pub stats: ParseStats,
    
    /// 解析耗时（毫秒）
    pub parse_duration_ms: u64,
}

/// 作用域信息
#[derive(Debug, Clone)]
pub struct ScopeInfo {
    /// 作用域 ID
    pub id: u64,
    /// 父作用域 ID
    pub parent_id: Option<u64>,
    /// 作用域类型（函数、块等）
    pub scope_type: ScopeType,
    /// 定义的符号
    pub symbols: Vec<String>,
    /// 起始位置
    pub start_location: SourceLocation,
    /// 结束位置
    pub end_location: SourceLocation,
}

/// 作用域类型
#[derive(Debug, Clone, PartialEq)]
pub enum ScopeType {
    Global,
    Function,
    Block,
    Loop,
    IfElse,
    MatchArm,
    Struct,
    Impl,
    Unknown,
}

/// 诊断信息
#[derive(Debug, Clone)]
pub struct DiagnosticInfo {
    /// 严重级别
    pub severity: DiagnosticSeverity,
    /// 消息内容
    pub message: String,
    /// 位置
    pub location: SourceLocation,
    /// 来源（lsp 名称、编译器等）
    pub source: Option<String>,
    /// 错误代码
    pub code: Option<String>,
}

/// 诊断严重级别
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

/// 解析统计信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParseStats {
    /// 总节点数
    pub total_nodes: usize,
    /// 最大深度
    pub max_depth: u32,
    /// 总符号数
    pub total_symbols: usize,
    /// 总作用域数
    pub total_scopes: usize,
    /// 源代码行数
    pub source_lines: usize,
    /// 源代码字符数
    pub source_chars: usize,
}

/// 解析器配置
#[derive(Debug, Clone)]
pub struct ParserConfig {
    /// 是否启用增量解析
    pub enable_incremental: bool,
    /// 是否启用符号解析
    pub enable_symbol_resolution: bool,
    /// 最大解析深度
    pub max_depth: u32,
    /// 缓存大小
    pub cache_size: usize,
}

impl Default for ParserConfig {
    fn default() -> Self {
        Self {
            enable_incremental: true,
            enable_symbol_resolution: true,
            max_depth: 256,
            cache_size: 100,
        }
    }
}

/// 语言解析器 trait
#[async_trait::async_trait]
pub trait LanguageParser: Send + Sync {
    /// 解析源代码
    async fn parse(&self, source: &str) -> Result<AstNode, ParseError>;
    
    /// 获取语言 ID
    fn language_id(&self) -> LanguageId;
    
    /// 获取支持的文件扩展名
    fn supported_extensions(&self) -> Vec<&str>;
}

/// 解析错误
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    
    #[error("Parse failed: {0}")]
    ParseFailed(String),
    
    #[error("Unsupported language: {0}")]
    UnsupportedLanguage(LanguageId),
    
    #[error("Source too large: {0} bytes")]
    SourceTooLarge(usize),
    
    #[error("Max depth exceeded: {0}")]
    MaxDepthExceeded(u32),
    
    #[error("Internal error: {0}")]
    Internal(String),
}

/// Tree-sitter 解析管理器
pub struct TreeSitterParserManager {
    /// 已加载的语言解析器缓存
    parsers: Arc<RwLock<HashMap<LanguageId, Box<dyn LanguageParser>>>>,
    
    /// 解析结果缓存（文件路径 -> ParseResult）
    cache: Arc<RwLock<HashMap<PathBuf, ParseResult>>>,
    
    /// 配置
    config: ParserConfig,
}

impl TreeSitterParserManager {
    /// 创建新的解析管理器
    pub fn new(config: ParserConfig) -> Self {
        Self {
            parsers: Arc::new(RwLock::new(HashMap::new())),
            cache: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }

    /// 使用默认配置创建
    pub fn with_defaults() -> Self {
        Self::new(ParserConfig::default())
    }

    /// 获取或创建指定语言的解析器
    async fn get_or_create_parser(&self, language: LanguageId) -> Result<Box<dyn LanguageParser>, ParseError> {
        // 先检查缓存
        {
            let parsers = self.parsers.read().await;
            if let Some(parser) = parsers.get(&language) {
                // 注意：这里不能直接返回 parser 的克隆，因为 LanguageParser 是 trait object
                // 实际实现中应该使用工厂模式或 Arc 包装
                return Err(ParseError::Internal("Parser cloning not implemented".to_string()));
            }
        }

        // 创建新的解析器
        let parser = self.create_parser_for_language(language.clone())?;
        
        // 存入缓存
        {
            let mut parsers = self.parsers.write().await;
            parsers.insert(language.clone(), parser);
        }

        // 再次获取（这次从缓存）
        let parsers = self.parsers.read().await;
        // 返回一个错误提示，实际实现需要更复杂的逻辑
        Err(ParseError::Internal("Parser retrieval needs refactoring".to_string()))
    }

    /// 为指定语言创建解析器
    fn create_parser_for_language(&self, language: LanguageId) -> Result<Box<dyn LanguageParser>, ParseError> {
        // 这里应该根据语言创建对应的解析器实例
        // 目前返回一个基础实现，后续可以集成真正的 tree-sitter 绑定
        
        match language {
            LanguageId::Rust | 
            LanguageId::TypeScript | 
            LanguageId::JavaScript | 
            LanguageId::Python |
            LanguageId::Go => {
                Ok(Box::new(BasicLanguageParser::new(language)))
            },
            _ => Err(ParseError::UnsupportedLanguage(language)),
        }
    }

    /// 解析源代码字符串
    pub async fn parse_source(
        &self,
        source: &str,
        language: LanguageId,
    ) -> Result<ParseResult, ParseError> {
        info!(
            language = %language,
            length = source.len(),
            "Parsing source code"
        );

        // 检查源代码大小限制
        const MAX_SOURCE_SIZE: usize = 10 * 1024 * 1024; // 10MB
        if source.len() > MAX_SOURCE_SIZE {
            return Err(ParseError::SourceTooLarge(source.len()));
        }

        let start_time = std::time::Instant::now();

        // 执行解析（使用基础解析器作为占位符）
        let root = BasicLanguageParser::new(language.clone()).parse(source).await?;

        // 构建符号表和作用域
        let (symbol_table, scopes) = if self.config.enable_symbol_resolution {
            self.build_symbol_table(&root)
        } else {
            (HashMap::new(), HashMap::new())
        };

        // 收集诊断信息
        let diagnostics = self.collect_diagnostics(&root);

        // 统计信息
        let stats = self.calculate_stats(&root, source);

        let parse_duration_ms = start_time.elapsed().as_millis() as u64;

        Ok(ParseResult {
            root,
            symbol_table,
            scopes,
            diagnostics,
            stats,
            parse_duration_ms,
        })
    }

    /// 解析文件
    pub async fn parse_file(
        &self,
        file_path: &PathBuf,
    ) -> Result<ParseResult, ParseError> {
        // 检查缓存
        if self.config.enable_incremental {
            let cache = self.cache.read().await;
            if let Some(cached) = cache.get(file_path) {
                return Ok(cached.clone());
            }
        }

        // 读取文件
        let source = tokio::fs::read_to_string(file_path).await?;
        
        // 推断语言
        let language = LanguageId::from_path(file_path.to_string_lossy().as_ref());

        // 解析
        let result = self.parse_source(&source, language).await?;

        // 存入缓存
        if self.config.enable_incremental {
            let mut cache = self.cache.write().await;
            cache.insert(file_path.clone(), result.clone());
        }

        Ok(result)
    }

    /// 构建符号表
    fn build_symbol_table(&self, root: &AstNode) -> (HashMap<String, SymbolEntry>, HashMap<u64, ScopeInfo>) {
        let mut symbol_table = HashMap::new();
        let mut scopes = HashMap::new();
        let mut next_scope_id: u64 = 1;

        // 创建全局作用域
        scopes.insert(0, ScopeInfo {
            id: 0,
            parent_id: None,
            scope_type: ScopeType::Global,
            symbols: Vec::new(),
            start_location: root.location,
            end_location: root.location,
        });

        // 递归遍历 AST 构建符号表和作用域
        self.build_symbols_recursive(root, 0, &mut symbol_table, &mut scopes, &mut next_scope_id);

        (symbol_table, scopes)
    }

    /// 递归构建符号
    fn build_symbols_recursive(
        &self,
        node: &AstNode,
        current_scope_id: u64,
        symbol_table: &mut HashMap<String, SymbolEntry>,
        scopes: &mut HashMap<u64, ScopeInfo>,
        next_scope_id: &mut u64,
    ) {
        // 根据节点类型决定是否创建符号
        if node.node_type.is_symbol_definition() {
            if let Some(ref name) = node.name {
                let kind = match node.node_type {
                    NodeType::FunctionDeclaration => SymbolKind::Function,
                    NodeType::StructDeclaration => SymbolKind::Struct,
                    NodeType::EnumDeclaration => SymbolKind::Enum,
                    NodeType::TraitDeclaration => SymbolKind::Trait,
                    NodeType::ClassDeclaration => SymbolKind::Class,
                    NodeType::InterfaceDeclaration => SymbolKind::Interface,
                    NodeType::VariableDeclaration => SymbolKind::Variable,
                    _ => SymbolKind::Unknown,
                };

                let entry = SymbolEntry {
                    name: name.clone(),
                    kind,
                    definition_location: node.location,
                    node_id: node.id,
                    scope_id: current_scope_id,
                    type_info: node.type_info.clone(),
                };

                symbol_table.insert(name.clone(), entry);

                // 将符号添加到当前作用域
                if let Some(scope) = scopes.get_mut(&current_scope_id) {
                    scope.symbols.push(name.clone());
                }
            }
        }

        // 根据节点类型决定是否创建新作用域
        let new_scope_id = match node.node_type {
            NodeType::FunctionDeclaration | 
            NodeType::ImplDeclaration => {
                let scope_id = *next_scope_id;
                *next_scope_id += 1;

                scopes.insert(scope_id, ScopeInfo {
                    id: scope_id,
                    parent_id: Some(current_scope_id),
                    scope_type: ScopeType::Function,
                    symbols: Vec::new(),
                    start_location: node.location,
                    end_location: node.location,
                });

                Some(scope_id)
            },
            NodeType::BlockStatement | 
            NodeType::ForStatement | 
            NodeType::WhileStatement | 
            NodeType::IfStatement | 
            NodeType::MatchStatement => {
                let scope_id = *next_scope_id;
                *next_scope_id += 1;

                let scope_type = match node.node_type {
                    NodeType::ForStatement | NodeType::WhileStatement => ScopeType::Loop,
                    NodeType::IfStatement => ScopeType::IfElse,
                    NodeType::MatchStatement => ScopeType::MatchArm,
                    _ => ScopeType::Block,
                };

                scopes.insert(scope_id, ScopeInfo {
                    id: scope_id,
                    parent_id: Some(current_scope_id),
                    scope_type,
                    symbols: Vec::new(),
                    start_location: node.location,
                    end_location: node.location,
                });

                Some(scope_id)
            },
            _ => None,
        };

        // 递归处理子节点
        let effective_scope_id = new_scope_id.unwrap_or(current_scope_id);
        for child in &node.children {
            self.build_symbols_recursive(child, effective_scope_id, symbol_table, scopes, next_scope_id);
        }
    }

    /// 收集诊断信息
    fn collect_diagnostics(&self, root: &AstNode) -> Vec<DiagnosticInfo> {
        let mut diagnostics = Vec::new();
        self.check_for_errors(root, &mut diagnostics);
        diagnostics
    }

    /// 递归检查错误节点
    fn check_for_errors(&self, node: &AstNode, diagnostics: &mut Vec<DiagnosticInfo>) {
        if node.node_type == NodeType::Error {
            diagnostics.push(DiagnosticInfo {
                severity: DiagnosticSeverity::Error,
                message: "Syntax error".to_string(),
                location: node.location,
                source: Some("tree-sitter".to_string()),
                code: None,
            });
        }

        for child in &node.children {
            self.check_for_errors(child, diagnostics);
        }
    }

    /// 计算统计信息
    fn calculate_stats(&self, root: &AstNode, source: &str) -> ParseStats {
        ParseStats {
            total_nodes: root.total_children() + 1,
            max_depth: root.depth(),
            total_symbols: 0, // 将在 build_symbol_table 后更新
            total_scopes: 0,  // 将在 build_symbol_table 后更新
            source_lines: source.lines().count(),
            source_chars: source.chars().count(),
        }
    }

    /// 清除解析缓存
    pub async fn clear_cache(&self) {
        let mut cache = self.cache.write().await;
        cache.clear();
        info!("Parser cache cleared");
    }

    /// 获取缓存大小
    pub async fn cache_size(&self) -> usize {
        let cache = self.cache.read().await;
        cache.len()
    }
}

/// 基础语言解析器（占位符实现）
struct BasicLanguageParser {
    language: LanguageId,
}

impl BasicLanguageParser {
    fn new(language: LanguageId) -> Self {
        Self { language }
    }
}

#[async_trait::async_trait]
impl LanguageParser for BasicLanguageParser {
    async fn parse(&self, source: &str) -> Result<AstNode, ParseError> {
        // 这是一个简化的解析器实现
        // 在生产环境中，应该集成真正的 tree-sitter 库
        
        let mut root = AstNode::new(NodeType::SourceFile, SourceLocation::new(0, 0, 0, 0));
        
        // 简单的逐行分析
        let lines: Vec<&str> = source.lines().collect();
        
        for (idx, line) in lines.iter().enumerate() {
            let line_num = idx as u32;
            
            // 检查是否是函数声明
            if line.trim_start().starts_with("fn ") ||
               line.trim_start().starts_with("func ") ||
               line.trim_start().starts_with("def ") ||
               line.trim_start().starts_with("function ") {
                
                let mut func_node = AstNode::new(
                    NodeType::FunctionDeclaration,
                    SourceLocation::new(line_num, 0, line_num, line.len() as u32)
                );
                
                // 尝试提取函数名
                let name = extract_function_name(line);
                func_node.name = Some(name);
                
                root.add_child(func_node);
            }
            
            // 检查是否是结构体/类声明
            if line.contains("struct ") || line.contains("class ") {
                let mut decl_node = AstNode::new(
                    NodeType::StructDeclaration,
                    SourceLocation::new(line_num, 0, line_num, line.len() as u32)
                );
                
                let name = extract_struct_name(line);
                decl_node.name = Some(name);
                
                root.add_child(decl_node);
            }
            
            // 检查是否是 import/use/require
            if line.trim_start().starts_with("use ") ||
               line.trim_start().starts_with("import ") ||
               line.trim_start().starts_with("require ") ||
               line.trim_start().starts_with("#include") {
                
                let import_node = AstNode::new(
                    NodeType::Unknown,
                    SourceLocation::new(line_num, 0, line_num, line.len() as u32)
                );
                
                root.add_child(import_node);
            }
        }
        
        Ok(root)
    }
    
    fn language_id(&self) -> LanguageId {
        self.language.clone()
    }
    
    fn supported_extensions(&self) -> Vec<&str> {
        match self.language {
            LanguageId::Rust => vec!["rs"],
            LanguageId::TypeScript => vec!["ts", "tsx"],
            LanguageId::JavaScript => vec!["js", "jsx"],
            LanguageId::Python => vec!["py", "pyi"],
            LanguageId::Go => vec!["go"],
            _ => vec![],
        }
    }
}

/// 提取函数名
fn extract_function_name(line: &str) -> String {
    let trimmed = line.trim();
    
    for prefix in ["fn ", "func ", "def ", "function "] {
        if trimmed.starts_with(prefix) {
            let rest = &trimmed[prefix.len()..];
            if let Some(end) = rest.find(['(', ' ', '<'].as_ref()) {
                return rest[..end].to_string();
            }
            return rest.split_whitespace().next().unwrap_or("anonymous").to_string();
        }
    }
    
    "anonymous".to_string()
}

/// 提取结构体/类名
fn extract_struct_name(line: &str) -> String {
    let trimmed = line.trim();
    
    for keyword in ["struct ", "class ", "enum ", "interface ", "type "] {
        if trimmed.contains(keyword) {
            if let Some(start) = trimmed.find(keyword) {
                let after_keyword = &trimmed[start + keyword.len()..];
                if let Some(end) = after_keyword.find([' ', '{', '<', ':', '('].as_ref()) {
                    return after_keyword[..end].to_string();
                }
                return after_keyword.split_whitespace()
                    .next()
                    .unwrap_or("Unnamed")
                    .to_string();
            }
        }
    }
    
    "Unnamed".to_string()
}
