use std::path::Path;

/// Supported language kinds for AST analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LanguageKind {
    Rust, TypeScript, JavaScript, Python, Go, Java, Cpp, Generic,
}

impl LanguageKind {
    pub fn from_path(path: &Path) -> Self {
        match path.extension().and_then(|e| e.to_str()) {
            Some("rs") => Self::Rust,
            Some("ts") | Some("tsx") => Self::TypeScript,
            Some("js") | Some("jsx") => Self::JavaScript,
            Some("py") => Self::Python,
            Some("go") => Self::Go,
            Some("java") => Self::Java,
            Some("cpp") | Some("cxx") | Some("hpp") => Self::Cpp,
            _ => Self::Generic,
        }
    }
}

/// A single AST node with position info.
#[derive(Debug, Clone)]
pub struct AstNode {
    pub kind: String,
    pub name: Option<String>,
    pub start_line: usize,
    pub end_line: usize,
    pub children: Vec<AstNode>,
}

/// An edit operation derived from AST analysis.
#[derive(Debug, Clone)]
pub struct AstEdit {
    pub file_path: String,
    pub language: LanguageKind,
    pub operations: Vec<AstEditOp>,
}

/// A single AST-level edit operation.
#[derive(Debug, Clone)]
pub enum AstEditOp {
    ReplaceFunction { name: String, new_body: String },
    AddImport { import: String },
    RemoveImport { import: String },
    ChangeType { symbol: String, old_type: String, new_type: String },
    RenameSymbol { old_name: String, new_name: String, scope: String },
}

/// AST adapter trait — one implementation per language.
#[async_trait::async_trait]
pub trait AstAdapter: Send + Sync {
    fn language(&self) -> LanguageKind;
    async fn parse(&self, code: &str, path: &Path) -> anyhow::Result<Vec<AstNode>>;
    async fn apply_edit(&self, code: &str, edit: &AstEditOp) -> anyhow::Result<String>;
    async fn find_dependents(&self, code: &str, symbol: &str) -> Vec<(usize, String)>;
}
