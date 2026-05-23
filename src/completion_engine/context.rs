use lsp_types::*;
use std::collections::HashMap;
use std::path::PathBuf;
use tree_sitter::{Language, Parser};
use serde::{Serialize, Deserialize};
use tokio::sync::RwLock;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolInfo {
    pub name: String,
    pub kind: SymbolKind,
    pub location: Location,
    pub documentation: Option<String>,
    pub signature: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeContext {
    pub file_path: PathBuf,
    pub language: String,
    pub content: String,
    pub position: Position,
    pub surrounding_code: String,
    pub project_symbols: Vec<SymbolInfo>,
    pub local_symbols: Vec<SymbolInfo>,
    pub imports: Vec<String>,
    pub comments: Vec<(Range, String)>,
    pub syntax_tree: Option<TreeContext>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeContext {
    pub current_node_type: String,
    pub parent_node_types: Vec<String>,
    pub sibling_nodes: Vec<String>,
    pub expected_types: Vec<String>,
}

#[derive(Clone)]
pub struct ContextAnalyzer {
    parsers: Arc<RwLock<HashMap<String, Parser>>>,
    language_mappings: HashMap<String, Language>,
}

impl std::fmt::Debug for ContextAnalyzer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContextAnalyzer")
            .field("language_mappings", &self.language_mappings)
            .finish_non_exhaustive()
    }
}

impl ContextAnalyzer {
    pub fn new() -> Self {
        Self {
            parsers: Arc::new(RwLock::new(HashMap::new())),
            language_mappings: HashMap::new(),
        }
    }

    pub async fn analyze_context(
        &self,
        file_path: &PathBuf,
        content: &str,
        position: Position,
        project_symbols: &[SymbolInfo],
    ) -> CodeContext {
        let language = self.detect_language(file_path);
        let surrounding_code = self.extract_surrounding_code(content, position);
        let local_symbols = self.extract_local_symbols(content, position);
        let imports = self.extract_imports(content);
        let comments = self.extract_comments(content);
        let syntax_tree = self.analyze_syntax_tree(content, position, &language);

        CodeContext {
            file_path: file_path.clone(),
            language,
            content: content.to_string(),
            position,
            surrounding_code,
            project_symbols: project_symbols.to_vec(),
            local_symbols,
            imports,
            comments,
            syntax_tree,
        }
    }

    fn detect_language(&self, file_path: &PathBuf) -> String {
        if let Some(ext) = file_path.extension() {
            match ext.to_str().unwrap_or("") {
                "rs" => "rust".to_string(),
                "py" => "python".to_string(),
                "ts" | "tsx" => "typescript".to_string(),
                "js" | "jsx" => "javascript".to_string(),
                "go" => "go".to_string(),
                "java" => "java".to_string(),
                "cpp" | "hpp" => "cpp".to_string(),
                "c" | "h" => "c".to_string(),
                "json" => "json".to_string(),
                "yaml" | "yml" => "yaml".to_string(),
                "toml" => "toml".to_string(),
                "md" => "markdown".to_string(),
                "sql" => "sql".to_string(),
                "sh" => "shell".to_string(),
                _ => "plaintext".to_string(),
            }
        } else {
            "plaintext".to_string()
        }
    }

    fn extract_surrounding_code(&self, content: &str, position: Position) -> String {
        let lines: Vec<&str> = content.lines().collect();
        let start_line = std::cmp::max(0, position.line as i32 - 20) as usize;
        let end_line = std::cmp::min(lines.len(), position.line as usize + 20);
        
        let mut surrounding = String::new();
        for (i, line) in lines[start_line..end_line].iter().enumerate() {
            let actual_line = start_line + i;
            let prefix = if actual_line == position.line as usize { ">> " } else { "   " };
            surrounding.push_str(&format!("{}{}\n", prefix, line));
        }
        surrounding
    }

    fn extract_local_symbols(&self, content: &str, position: Position) -> Vec<SymbolInfo> {
        let mut symbols = Vec::new();
        let lines: Vec<&str> = content.lines().collect();
        
        let current_line = position.line as usize;
        let start_line = std::cmp::max(0, current_line as i32 - 100) as usize;
        
        for (i, line) in lines[start_line..current_line].iter().enumerate() {
            let line_num = start_line + i;
            self.parse_line_for_symbols(line, line_num, &mut symbols);
        }
        symbols
    }

    fn parse_line_for_symbols(&self, line: &str, line_num: usize, symbols: &mut Vec<SymbolInfo>) {
        let trimmed = line.trim();
        
        if trimmed.starts_with("fn ") || trimmed.starts_with("async fn ") {
            if let Some(name) = trimmed.split_whitespace().nth(if trimmed.starts_with("async") { 2 } else { 1 }) {
                let name = name.split(['(', '{', ' ']).next().unwrap_or(name);
                symbols.push(SymbolInfo {
                    name: name.to_string(),
                    kind: SymbolKind::FUNCTION,
                    location: Location {
                        uri: Url::parse("file:///").unwrap(),
                        range: Range {
                            start: Position { line: line_num as u32, character: 0 },
                            end: Position { line: line_num as u32, character: 0 },
                        },
                    },
                    documentation: None,
                    signature: None,
                });
            }
        } else if trimmed.starts_with("struct ") {
            if let Some(name) = trimmed.split_whitespace().nth(1) {
                let name = name.split(['{', ' ']).next().unwrap_or(name);
                symbols.push(SymbolInfo {
                    name: name.to_string(),
                    kind: SymbolKind::STRUCT,
                    location: Location {
                        uri: Url::parse("file:///").unwrap(),
                        range: Range {
                            start: Position { line: line_num as u32, character: 0 },
                            end: Position { line: line_num as u32, character: 0 },
                        },
                    },
                    documentation: None,
                    signature: None,
                });
            }
        } else if trimmed.starts_with("let ") || trimmed.starts_with("const ") || trimmed.starts_with("static ") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() > 1 {
                let name = parts[1].split(['=', ':', ';']).next().unwrap_or(parts[1]);
                symbols.push(SymbolInfo {
                    name: name.to_string(),
                    kind: SymbolKind::VARIABLE,
                    location: Location {
                        uri: Url::parse("file:///").unwrap(),
                        range: Range {
                            start: Position { line: line_num as u32, character: 0 },
                            end: Position { line: line_num as u32, character: 0 },
                        },
                    },
                    documentation: None,
                    signature: None,
                });
            }
        }
    }

    fn extract_imports(&self, content: &str) -> Vec<String> {
        let mut imports = Vec::new();
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("use ") || trimmed.starts_with("import ") || 
               trimmed.starts_with("from ") || trimmed.starts_with("require(") {
                imports.push(line.to_string());
            }
        }
        imports
    }

    fn extract_comments(&self, content: &str) -> Vec<(Range, String)> {
        let mut comments = Vec::new();
        let lines: Vec<&str> = content.lines().collect();
        
        for (i, line) in lines.iter().enumerate() {
            if let Some(comment_start) = line.find("//") {
                comments.push((
                    Range {
                        start: Position { line: i as u32, character: comment_start as u32 },
                        end: Position { line: i as u32, character: line.len() as u32 },
                    },
                    line[comment_start..].to_string(),
                ));
            }
        }
        comments
    }

    fn analyze_syntax_tree(&self, _content: &str, _position: Position, language: &str) -> Option<TreeContext> {
        Some(TreeContext {
            current_node_type: "expression".to_string(),
            parent_node_types: vec!["function".to_string(), "block".to_string()],
            sibling_nodes: vec![],
            expected_types: self.predict_expected_types(language),
        })
    }

    fn predict_expected_types(&self, language: &str) -> Vec<String> {
        match language {
            "rust" => vec!["String", "Vec", "Result", "Option", "HashMap", "VecDeque"],
            "python" => vec!["str", "list", "dict", "int", "bool", "None", "Any"],
            "typescript" | "javascript" => vec!["string", "number", "boolean", "array", "object", "Promise"],
            "go" => vec!["string", "[]byte", "error", "map", "slice", "struct"],
            _ => vec![],
        }
        .into_iter()
        .map(|s| s.to_string())
        .collect()
    }
}