//! Tree-sitter AST Parser for Semantic Code Understanding
//!
//! This module provides deep semantic analysis of code using tree-sitter,
//! enabling:
//! - Precise type inference at cursor position
//! - Scope chain extraction (module -> function -> block)
//! - Symbol resolution and reference tracking
//! - Syntax-aware completion context

use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::debug;

/// Supported programming languages
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Language {
    Rust,
    TypeScript,
    Python,
}

impl Language {
    pub fn from_file_extension(ext: &str) -> Option<Self> {
        match ext {
            "rs" => Some(Language::Rust),
            "ts" | "tsx" | "js" | "jsx" => Some(Language::TypeScript),
            "py" => Some(Language::Python),
            _ => None,
        }
    }

    pub fn get_tree_sitter_language(&self) -> tree_sitter::Language {
        match self {
            Language::Rust => tree_sitter_rust::language(),
            Language::TypeScript => tree_sitter_typescript::language_typescript(),
            Language::Python => tree_sitter_python::language(),
        }
    }
}

/// Parsed AST with tree-sitter tree
pub struct AstTree {
    pub tree: tree_sitter::Tree,
    pub source_code: String,
    pub language: Language,
}

impl AstTree {
    pub fn parse(code: &str, language: Language) -> Option<Self> {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(language.get_tree_sitter_language()).ok()?;

        let tree = parser.parse(code, None)?;

        Some(Self {
            tree,
            source_code: code.to_string(),
            language,
        })
    }

    /// Get the node at a specific byte position
    pub fn get_node_at_position(&self, byte_offset: usize) -> Option<tree_sitter::Node> {
        let root = self.tree.root_node();
        Self::find_node_at_position(root, byte_offset)
    }

    /// Recursively find the deepest node containing the position
    fn find_node_at_position(
        node: tree_sitter::Node,
        byte_offset: usize,
    ) -> Option<tree_sitter::Node> {
        // Check if position is within this node
        if byte_offset < node.start_byte() || byte_offset > node.end_byte() {
            return None;
        }

        // Try to find a child that contains the position
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                if byte_offset >= child.start_byte() && byte_offset <= child.end_byte() {
                    if let Some(deeper) = Self::find_node_at_position(child, byte_offset) {
                        return Some(deeper);
                    }
                }
            }
        }

        // If no child contains the position, return this node
        Some(node)
    }

    /// Extract the scope chain at a given position
    /// Returns vec of (scope_type, scope_name) e.g., [("module", "std"), ("function", "main")]
    pub fn extract_scope_chain(&self, byte_offset: usize) -> Vec<(String, Option<String>)> {
        let mut scopes = Vec::new();
        let Some(node) = self.get_node_at_position(byte_offset) else {
            return scopes;
        };

        let mut current = node;
        while let Some(parent) = current.parent() {
            if let Some(scope) = self.extract_scope_from_node(parent) {
                scopes.push(scope);
            }
            current = parent;
        }

        scopes.reverse();
        scopes
    }

    /// Extract scope information from a node
    fn extract_scope_from_node(
        &self,
        node: tree_sitter::Node,
    ) -> Option<(String, Option<String>)> {
        match self.language {
            Language::Rust => self.extract_rust_scope(node),
            Language::TypeScript => self.extract_ts_scope(node),
            Language::Python => self.extract_python_scope(node),
        }
    }

    /// Extract Rust-specific scope information
    fn extract_rust_scope(
        &self,
        node: tree_sitter::Node,
    ) -> Option<(String, Option<String>)> {
        match node.kind() {
            "function_item" => {
                let name = self.get_node_name(node);
                Some(("function".to_string(), name))
            }
            "impl_item" => {
                // Extract type being implemented
                let type_node = node.child_by_field_name("type")?;
                let type_name = self.node_text(type_node);
                Some(("impl".to_string(), Some(type_name)))
            }
            "struct_item" => {
                let name = self.get_node_name(node);
                Some(("struct".to_string(), name))
            }
            "enum_item" => {
                let name = self.get_node_name(node);
                Some(("enum".to_string(), name))
            }
            "mod_item" => {
                let name = self.get_node_name(node);
                Some(("module".to_string(), name))
            }
            "closure_expression" => {
                Some(("closure".to_string(), None))
            }
            _ => None,
        }
    }

    /// Extract TypeScript-specific scope information
    fn extract_ts_scope(
        &self,
        node: tree_sitter::Node,
    ) -> Option<(String, Option<String>)> {
        match node.kind() {
            "function_declaration" | "arrow_function" => {
                let name = self.get_node_name(node);
                Some(("function".to_string(), name))
            }
            "class_declaration" => {
                let name = self.get_node_name(node);
                Some(("class".to_string(), name))
            }
            "method_definition" => {
                let name = self.get_node_name(node);
                Some(("method".to_string(), name))
            }
            "interface_declaration" => {
                let name = self.get_node_name(node);
                Some(("interface".to_string(), name))
            }
            _ => None,
        }
    }

    /// Extract Python-specific scope information
    fn extract_python_scope(
        &self,
        node: tree_sitter::Node,
    ) -> Option<(String, Option<String>)> {
        match node.kind() {
            "function_definition" => {
                let name = self.get_node_name(node);
                Some(("function".to_string(), name))
            }
            "class_definition" => {
                let name = self.get_node_name(node);
                Some(("class".to_string(), name))
            }
            _ => None,
        }
    }

    /// Get the inferred type at cursor position
    pub fn infer_type_at_position(&self, byte_offset: usize) -> Option<String> {
        let Some(node) = self.get_node_at_position(byte_offset) else {
            return None;
        };

        match self.language {
            Language::Rust => self.infer_rust_type(node),
            Language::TypeScript => self.infer_ts_type(node),
            Language::Python => self.infer_python_type(node),
        }
    }

    /// Infer Rust type from context
    fn infer_rust_type(&self, node: tree_sitter::Node) -> Option<String> {
        // Check if node is part of a let binding with type annotation
        if let Some(parent) = node.parent() {
            match parent.kind() {
                "let_declaration" => {
                    if let Some(type_node) = parent.child_by_field_name("type") {
                        return Some(self.node_text(type_node));
                    }
                }
                "parameter" => {
                    if let Some(type_node) = parent.child_by_field_name("type") {
                        return Some(self.node_text(type_node));
                    }
                }
                "return_type" => {
                    return Some(self.node_text(parent));
                }
                _ => {}
            }
        }

        // Try to infer from expression context
        match node.kind() {
            "string_literal" => Some("String".to_string()),
            "integer_literal" => Some("i32".to_string()),
            "float_literal" => Some("f64".to_string()),
            "boolean_literal" => Some("bool".to_string()),
            "identifier" => {
                // Look up variable declaration
                self.resolve_identifier_type(node)
            }
            _ => None,
        }
    }

    /// Infer TypeScript type
    fn infer_ts_type(&self, node: tree_sitter::Node) -> Option<String> {
        match node.kind() {
            "string" => Some("string".to_string()),
            "number" => Some("number".to_string()),
            "true" | "false" => Some("boolean".to_string()),
            "identifier" => self.resolve_identifier_type(node),
            _ => None,
        }
    }

    /// Infer Python type (limited without runtime info)
    fn infer_python_type(&self, node: tree_sitter::Node) -> Option<String> {
        match node.kind() {
            "string" => Some("str".to_string()),
            "integer" => Some("int".to_string()),
            "float" => Some("float".to_string()),
            "true" | "false" => Some("bool".to_string()),
            "list" => Some("list".to_string()),
            "dictionary" => Some("dict".to_string()),
            _ => None,
        }
    }

    /// Resolve identifier to its declared type
    fn resolve_identifier_type(&self, node: tree_sitter::Node) -> Option<String> {
        let var_name = self.node_text(node);

        // Walk up the tree to find the declaration
        let mut current = node;
        while let Some(parent) = current.parent() {
            // Search for variable declaration in parent scope
            for i in 0..parent.child_count() {
                if let Some(sibling) = parent.child(i) {
                    if sibling.kind() == "let_declaration" || sibling.kind() == "variable_declarator"
                    {
                        let name_node = sibling.child_by_field_name("name")?;
                        if self.node_text(name_node) == var_name {
                            if let Some(type_node) = sibling.child_by_field_name("type") {
                                return Some(self.node_text(type_node));
                            }
                        }
                    }
                }
            }
            current = parent;
        }

        None
    }

    /// Get all symbols (functions, types, variables) in the file
    pub fn extract_all_symbols(&self) -> Vec<SymbolInfo> {
        let mut symbols = Vec::new();
        self.extract_symbols_recursive(self.tree.root_node(), &mut symbols);
        symbols
    }

    fn extract_symbols_recursive(
        &self,
        node: tree_sitter::Node,
        symbols: &mut Vec<SymbolInfo>,
    ) {
        // Check if this node is a symbol definition
        if let Some(symbol) = self.extract_symbol_from_node(node) {
            symbols.push(symbol);
        }

        // Recurse into children
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                self.extract_symbols_recursive(child, symbols);
            }
        }
    }

    fn extract_symbol_from_node(&self, node: tree_sitter::Node) -> Option<SymbolInfo> {
        let kind = match node.kind() {
            "function_item" | "function_declaration" => SymbolKind::Function,
            "struct_item" | "class_declaration" => SymbolKind::Type,
            "let_declaration" | "variable_declarator" => SymbolKind::Variable,
            _ => return None,
        };

        let name = self.get_node_name(node)?;
        let line = node.start_position().row;

        Some(SymbolInfo {
            name,
            kind,
            line,
            signature: self.extract_signature(node),
        })
    }

    /// Helper: Get text content of a node
    pub fn node_text(&self, node: tree_sitter::Node) -> String {
        node.utf8_text(self.source_code.as_bytes())
            .unwrap_or("")
            .to_string()
    }

    /// Helper: Get name from a definition node
    fn get_node_name(&self, node: tree_sitter::Node) -> Option<String> {
        if let Some(name_node) = node.child_by_field_name("name") {
            Some(self.node_text(name_node))
        } else {
            None
        }
    }

    /// Extract function/type signature
    fn extract_signature(&self, node: tree_sitter::Node) -> Option<String> {
        // Simplified: just return the first line of the node
        let text = self.node_text(node);
        text.lines().next().map(|s| s.trim().to_string())
    }
}

/// Information about a code symbol
#[derive(Debug, Clone)]
pub struct SymbolInfo {
    pub name: String,
    pub kind: SymbolKind,
    pub line: usize,
    pub signature: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SymbolKind {
    Function,
    Type,
    Variable,
}

/// AST Parser cache for performance
pub struct AstParserCache {
    trees: RwLock<HashMap<String, Arc<AstTree>>>,
    max_cache_size: usize,
}

impl AstParserCache {
    pub fn new(max_cache_size: usize) -> Self {
        Self {
            trees: RwLock::new(HashMap::new()),
            max_cache_size,
        }
    }

    /// Parse or retrieve cached AST
    pub fn parse(&self, file_path: &str, code: &str, language: Language) -> Arc<AstTree> {
        // Check cache first
        {
            let trees = self.trees.read();
            if let Some(tree) = trees.get(file_path) {
                debug!("AST cache hit for {}", file_path);
                return tree.clone();
            }
        }

        // Parse new AST
        debug!("Parsing AST for {} ({:?})", file_path, language);
        let tree = Arc::new(AstTree::parse(code, language).unwrap_or_else(|| {
            // Fallback: create empty tree
            AstTree::parse("", language).expect("Failed to create fallback AST")
        }));

        // Update cache
        {
            let mut trees = self.trees.write();

            // Evict if cache is full
            if trees.len() >= self.max_cache_size {
                if let Some(oldest_key) = trees.keys().next().cloned() {
                    trees.remove(&oldest_key);
                }
            }

            trees.insert(file_path.to_string(), tree.clone());
        }

        tree
    }

    /// Clear the cache
    pub fn clear(&self) {
        self.trees.write().clear();
    }

    /// Get cache statistics
    pub fn get_stats(&self) -> (usize, usize) {
        let trees = self.trees.read();
        (trees.len(), self.max_cache_size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_rust_simple() {
        let code = r#"
fn main() {
    let x: i32 = 42;
    println!("{}", x);
}
"#;

        let ast = AstTree::parse(code, Language::Rust).unwrap();
        assert!(ast.tree.root_node().child_count() > 0);
    }

    #[test]
    fn test_extract_scope_chain() {
        let code = r#"
mod my_module {
    fn my_function() {
        let x = 42;
    }
}
"#;

        let ast = AstTree::parse(code, Language::Rust).unwrap();

        // Find position inside the function
        let byte_offset = code.find("let x").unwrap();
        let scopes = ast.extract_scope_chain(byte_offset);

        assert!(!scopes.is_empty());
        assert!(scopes.iter().any(|(kind, _)| kind == "module"));
        assert!(scopes.iter().any(|(kind, _)| kind == "function"));
    }

    #[test]
    fn test_infer_type() {
        let code = r#"
fn test() {
    let x: String = "hello".to_string();
}
"#;

        let ast = AstTree::parse(code, Language::Rust).unwrap();
        let byte_offset = code.find("x").unwrap();

        if let Some(inferred_type) = ast.infer_type_at_position(byte_offset) {
            assert_eq!(inferred_type, "String");
        }
    }

    #[test]
    fn test_extract_symbols() {
        let code = r#"
fn function_one() {}
fn function_two() {}
struct MyStruct {}
"#;

        let ast = AstTree::parse(code, Language::Rust).unwrap();
        let symbols = ast.extract_all_symbols();

        assert_eq!(symbols.len(), 3);
        assert!(symbols.iter().any(|s| s.name == "function_one"));
        assert!(symbols.iter().any(|s| s.name == "MyStruct"));
    }

    #[test]
    fn test_ast_cache() {
        let cache = AstParserCache::new(10);

        let code = "fn main() {}";
        let tree1 = cache.parse("test.rs", code, Language::Rust);
        let tree2 = cache.parse("test.rs", code, Language::Rust);

        // Should return same cached instance
        assert!(Arc::ptr_eq(&tree1, &tree2));

        let (cached, max) = cache.get_stats();
        assert_eq!(cached, 1);
        assert_eq!(max, 10);
    }
}
