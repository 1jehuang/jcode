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
            Some("py") | Some("pyi") => Self::Python,
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
    /// Replace a function's body
    ReplaceFunction { name: String, new_body: String },
    /// Add an import statement
    AddImport { import: String },
    /// Remove an import statement
    RemoveImport { import: String },
    /// Change a type annotation
    ChangeType { symbol: String, old_type: String, new_type: String },
    /// Rename a symbol across scope
    RenameSymbol { old_name: String, new_name: String, scope: String },
    /// Insert raw text at a position
    Insert { content: String, line: usize, column: usize },
    /// Delete a range of lines
    Delete { start_line: usize, end_line: usize },
    /// Replace a range of lines with new content
    Replace { start_line: usize, end_line: usize, content: String },
}

/// AST adapter trait — one implementation per language.
#[async_trait::async_trait]
pub trait AstAdapter: Send + Sync {
    fn language(&self) -> LanguageKind;
    async fn parse(&self, code: &str, path: &Path) -> anyhow::Result<Vec<AstNode>>;
    async fn apply_edit(&self, code: &str, edit: &AstEditOp) -> anyhow::Result<String>;
    async fn find_dependents(&self, code: &str, symbol: &str) -> Vec<(usize, String)>;
}

// ════════════════════════════════════════════════════════════════
// TreeSitterAstAdapter — 基于 tree-sitter 的真实 AST 适配器
// ════════════════════════════════════════════════════════════════

/// 基于 tree-sitter 的 Rust 语言 AST 适配器
///
/// 实现 AstAdapter trait, 让 CrossFileRepairEngine<A> 可以实例化
pub struct TreeSitterAstAdapter {
    language: LanguageKind,
}

impl TreeSitterAstAdapter {
    pub fn new(language: LanguageKind) -> Self {
        Self { language }
    }

    pub fn rust() -> Self {
        Self::new(LanguageKind::Rust)
    }

    /// Parse Rust source using tree-sitter
    fn parse_rust(&self, code: &str) -> anyhow::Result<Vec<AstNode>> {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&tree_sitter_rust::LANGUAGE.into())
            .map_err(|e| anyhow::anyhow!("Failed to set Rust language: {}", e))?;

        let tree = parser.parse(code, None)
            .ok_or_else(|| anyhow::anyhow!("tree-sitter parse returned None"))?;

        let root = tree.root_node();
        let mut nodes = Vec::new();
        self.walk_node(&root, code, &mut nodes);
        Ok(nodes)
    }

    /// Parse TypeScript/TSX source using tree-sitter
    #[cfg(feature = "multi-lang")]
    fn parse_typescript(&self, code: &str, is_tsx: bool) -> anyhow::Result<Vec<AstNode>> {
        let mut parser = tree_sitter::Parser::new();
        let lang = if is_tsx {
            tree_sitter_typescript::LANGUAGE_TSX.into()
        } else {
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
        };
        parser.set_language(&lang)
            .map_err(|e| anyhow::anyhow!("Failed to set TypeScript language: {}", e))?;

        let tree = parser.parse(code, None)
            .ok_or_else(|| anyhow::anyhow!("tree-sitter parse returned None"))?;

        let root = tree.root_node();
        let mut nodes = Vec::new();
        self.walk_node(&root, code, &mut nodes);
        Ok(nodes)
    }

    /// Parse Python source using tree-sitter
    #[cfg(feature = "multi-lang")]
    fn parse_python(&self, code: &str) -> anyhow::Result<Vec<AstNode>> {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&tree_sitter_python::LANGUAGE.into())
            .map_err(|e| anyhow::anyhow!("Failed to set Python language: {}", e))?;

        let tree = parser.parse(code, None)
            .ok_or_else(|| anyhow::anyhow!("tree-sitter parse returned None"))?;

        let root = tree.root_node();
        let mut nodes = Vec::new();
        self.walk_node(&root, code, &mut nodes);
        Ok(nodes)
    }

    /// Parse Go source using tree-sitter
    #[cfg(feature = "multi-lang")]
    fn parse_go(&self, code: &str) -> anyhow::Result<Vec<AstNode>> {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&tree_sitter_go::LANGUAGE.into())
            .map_err(|e| anyhow::anyhow!("Failed to set Go language: {}", e))?;

        let tree = parser.parse(code, None)
            .ok_or_else(|| anyhow::anyhow!("tree-sitter parse returned None"))?;

        let root = tree.root_node();
        let mut nodes = Vec::new();
        self.walk_node(&root, code, &mut nodes);
        Ok(nodes)
    }

    /// Walk tree-sitter node tree and convert to our AstNode
    fn walk_node(&self, node: &tree_sitter::Node, source: &str, output: &mut Vec<AstNode>) {
        // Only collect top-level declarations
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if !child.is_named() { continue; }

            let kind = child.kind();
            let name = self.extract_name(&child, source);
            let start_line = child.start_position().row + 1; // 1-based
            let end_line = child.end_position().row + 1;

            // Collect children recursively for nested structures
            let mut children = Vec::new();
            self.walk_children(&child, source, &mut children);

            output.push(AstNode {
                kind: kind.to_string(),
                name,
                start_line,
                end_line,
                children,
            });
        }
    }

    fn walk_children(&self, node: &tree_sitter::Node, source: &str, output: &mut Vec<AstNode>) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if !child.is_named() { continue; }
            // Skip trivia
            if child.is_extra() { continue; }

            let name = self.extract_name(&child, source);
            let start_line = child.start_position().row + 1;
            let end_line = child.end_position().row + 1;

            let mut children = Vec::new();
            self.walk_children(&child, source, &mut children);

            output.push(AstNode {
                kind: child.kind().to_string(),
                name,
                start_line,
                end_line,
                children,
            });
        }
    }

    fn extract_name(&self, node: &tree_sitter::Node, source: &str) -> Option<String> {
        #[allow(unreachable_patterns)]
        match node.kind() {
            // Rust
            "function_item" | "function_signature_item" => {
                node.child_by_field_name("name")
                    .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                    .map(|s| s.to_string())
            }
            "struct_item" | "enum_item" | "trait_item" | "type_item" | "union_item" => {
                node.child_by_field_name("name")
                    .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                    .map(|s| s.to_string())
            }
            "impl_item" => {
                node.child_by_field_name("trait")
                    .or_else(|| node.child_by_field_name("type"))
                    .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                    .map(|s| s.to_string())
            }
            "let_declaration" => {
                node.child_by_field_name("pattern")
                    .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                    .map(|s| s.to_string())
            }
            "field_declaration" => {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if child.kind() == "field_identifier" {
                        return child.utf8_text(source.as_bytes()).ok().map(|s| s.to_string());
                    }
                }
                None
            }
            // TypeScript/JavaScript
            "function_declaration" | "method_definition" | "class_declaration" |
            "interface_declaration" | "type_alias_declaration" | "enum_declaration" => {
                node.child_by_field_name("name")
                    .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                    .map(|s| s.to_string())
            }
            "variable_declarator" => {
                node.child_by_field_name("name")
                    .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                    .map(|s| s.to_string())
            }
            // Python
            "function_definition" | "class_definition" => {
                node.child_by_field_name("name")
                    .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                    .map(|s| s.to_string())
            }
            // Go
            "method_declaration" | "type_declaration" => {
                node.child_by_field_name("name")
                    .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                    .map(|s| s.to_string())
            }
            _ => None,
        }
    }

    /// Apply a rename edit using AST-aware replacement
    fn apply_rename(&self, code: &str, old_name: &str, new_name: &str) -> String {
        let mut parser = tree_sitter::Parser::new();
        if parser.set_language(&tree_sitter_rust::LANGUAGE.into()).is_err() {
            // Fallback to regex
            let re = regex::Regex::new(&format!(r"\b{}\b", regex::escape(old_name))).unwrap();
            return re.replace_all(code, new_name).to_string();
        }

        let tree = match parser.parse(code, None) {
            Some(t) => t,
            None => return code.to_string(),
        };

        let root = tree.root_node();
        let mut edits: Vec<(usize, usize)> = Vec::new();

        self.find_identifier_refs_ast(&root, code, old_name, &mut edits);

        // Apply in reverse order
        edits.sort_by(|a, b| b.0.cmp(&a.0));
        let mut result = code.to_string();
        for (start, end) in edits {
            result.replace_range(start..end, new_name);
        }
        result
    }

    fn find_identifier_refs_ast(
        &self,
        node: &tree_sitter::Node,
        source: &str,
        name: &str,
        edits: &mut Vec<(usize, usize)>,
    ) {
        // Skip comments and strings
        if matches!(node.kind(),
            "line_comment" | "block_comment" | "string_literal" |
            "raw_string_literal" | "char_literal" | "string_content"
        ) {
            return;
        }

        if node.kind() == "identifier" || node.kind() == "type_identifier" {
            if let Ok(text) = node.utf8_text(source.as_bytes()) {
                if text == name {
                    edits.push((node.start_byte(), node.end_byte()));
                }
            }
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.is_named() {
                self.find_identifier_refs_ast(&child, source, name, edits);
            }
        }
    }

    /// Apply import addition
    fn apply_add_import(&self, code: &str, import: &str) -> String {
        // Find the last use statement
        let mut last_use_line = 0;
        for (i, line) in code.lines().enumerate() {
            if line.trim().starts_with("use ") {
                last_use_line = i + 1;
            }
        }

        let use_stmt = format!("use {};", import);
        if last_use_line > 0 {
            // Insert after the last use statement
            let lines: Vec<&str> = code.lines().collect();
            let mut new_lines = lines[..last_use_line].to_vec();
            new_lines.push(&use_stmt);
            new_lines.extend_from_slice(&lines[last_use_line..]);
            new_lines.join("\n")
        } else {
            // Insert at the top (after any comments/attributes)
            format!("{}\n\n{}", use_stmt, code)
        }
    }

    /// Apply import removal
    fn apply_remove_import(&self, code: &str, import: &str) -> String {
        let lines: Vec<&str> = code.lines().collect();
        let use_stmt_prefix = format!("use {};", import);
        let use_stmt_alt = format!("use {}", import);

        lines.iter()
            .filter(|line| {
                let trimmed = line.trim();
                !trimmed.starts_with(&use_stmt_prefix) && !trimmed.starts_with(&use_stmt_alt)
            })
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Find dependents using AST — returns (line_number, dependency_description)
    fn find_dependents_ast(&self, code: &str, symbol: &str) -> Vec<(usize, String)> {
        let mut dependents = Vec::new();

        let mut parser = tree_sitter::Parser::new();
        let lang_set = match self.language {
            LanguageKind::Rust => {
                if parser.set_language(&tree_sitter_rust::LANGUAGE.into()).is_err() {
                    return dependents;
                }
                true
            }
            #[cfg(feature = "multi-lang")]
            LanguageKind::TypeScript | LanguageKind::JavaScript => {
                if parser.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).is_err() {
                    return dependents;
                }
                true
            }
            #[cfg(feature = "multi-lang")]
            LanguageKind::Python => {
                if parser.set_language(&tree_sitter_python::LANGUAGE.into()).is_err() {
                    return dependents;
                }
                true
            }
            #[cfg(feature = "multi-lang")]
            LanguageKind::Go => {
                if parser.set_language(&tree_sitter_go::LANGUAGE.into()).is_err() {
                    return dependents;
                }
                true
            }
            _ => false,
        };

        if !lang_set { return dependents; }

        let tree = match parser.parse(code, None) {
            Some(t) => t,
            None => return dependents,
        };

        let root = tree.root_node();
        self.find_symbol_uses(&root, code, symbol, &mut dependents);
        dependents
    }

    fn find_symbol_uses(
        &self,
        node: &tree_sitter::Node,
        source: &str,
        symbol: &str,
        results: &mut Vec<(usize, String)>,
    ) {
        if matches!(node.kind(),
            "line_comment" | "block_comment" | "string_literal" |
            "raw_string_literal" | "char_literal"
        ) {
            return;
        }

        if node.kind() == "identifier" || node.kind() == "type_identifier" {
            if let Ok(text) = node.utf8_text(source.as_bytes()) {
                if text == symbol {
                    let line = node.start_position().row + 1;
                    let context = self.get_line_context(source, line);
                    results.push((line, format!("use of '{}' at line {}: {}", symbol, line, context)));
                }
            }
        }

        // Also check call expressions that reference the symbol
        if node.kind() == "call_expression" {
            if let Some(func) = node.child(0) {
                if let Ok(text) = func.utf8_text(source.as_bytes()) {
                    if text.contains(symbol) {
                        let line = node.start_position().row + 1;
                        let context = self.get_line_context(source, line);
                        results.push((line, format!("call to '{}' at line {}: {}", symbol, line, context)));
                    }
                }
            }
        }

        // Check impl blocks
        if node.kind() == "impl_item" {
            if let Some(trait_name) = node.child_by_field_name("trait") {
                if let Ok(text) = trait_name.utf8_text(source.as_bytes()) {
                    if text == symbol {
                        let line = node.start_position().row + 1;
                        results.push((line, format!("impl for '{}' at line {}", symbol, line)));
                    }
                }
            }
            if let Some(type_name) = node.child_by_field_name("type") {
                if let Ok(text) = type_name.utf8_text(source.as_bytes()) {
                    if text == symbol {
                        let line = node.start_position().row + 1;
                        results.push((line, format!("impl block for '{}' at line {}", symbol, line)));
                    }
                }
            }
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.is_named() {
                self.find_symbol_uses(&child, source, symbol, results);
            }
        }
    }

    fn get_line_context(&self, source: &str, line: usize) -> String {
        source.lines()
            .nth(line - 1)
            .unwrap_or("")
            .trim()
            .to_string()
    }
}

impl Default for TreeSitterAstAdapter {
    fn default() -> Self {
        Self::rust()
    }
}

#[async_trait::async_trait]
impl AstAdapter for TreeSitterAstAdapter {
    fn language(&self) -> LanguageKind {
        self.language
    }

    async fn parse(&self, code: &str, path: &Path) -> anyhow::Result<Vec<AstNode>> {
        // Use the configured language, or auto-detect from file path
        let lang = if self.language == LanguageKind::Generic {
            LanguageKind::from_path(path)
        } else {
            self.language
        };

        match lang {
            LanguageKind::Rust => self.parse_rust(code),
            #[cfg(feature = "multi-lang")]
            LanguageKind::TypeScript => self.parse_typescript(code, false),
            #[cfg(feature = "multi-lang")]
            LanguageKind::JavaScript => self.parse_typescript(code, true),
            #[cfg(feature = "multi-lang")]
            LanguageKind::Python => self.parse_python(code),
            #[cfg(feature = "multi-lang")]
            LanguageKind::Go => self.parse_go(code),
            _ => {
                // Fallback: simple line-based parsing for unsupported languages
                let mut nodes = Vec::new();
                for (i, line) in code.lines().enumerate() {
                    let trimmed = line.trim();
                    if trimmed.starts_with("function ") || trimmed.starts_with("def ") || trimmed.starts_with("fn ") {
                        nodes.push(AstNode {
                            kind: "function".to_string(),
                            name: Some(trimmed.split('(').next().unwrap_or("").split_whitespace().last().unwrap_or("").to_string()),
                            start_line: i + 1,
                            end_line: i + 1,
                            children: Vec::new(),
                        });
                    }
                }
                Ok(nodes)
            }
        }
    }

    async fn apply_edit(&self, code: &str, edit: &AstEditOp) -> anyhow::Result<String> {
        match edit {
            AstEditOp::RenameSymbol { old_name, new_name, .. } => {
                Ok(self.apply_rename(code, old_name, new_name))
            }
            AstEditOp::AddImport { import } => {
                Ok(self.apply_add_import(code, import))
            }
            AstEditOp::RemoveImport { import } => {
                Ok(self.apply_remove_import(code, import))
            }
            AstEditOp::ReplaceFunction { name, new_body } => {
                // Use tree-sitter to find and replace function body
                let mut parser = tree_sitter::Parser::new();
                if parser.set_language(&tree_sitter_rust::LANGUAGE.into()).is_err() {
                    // Fallback: regex replace
                    let re = regex::Regex::new(&format!(
                        r"(?s)(?:pub\s+)?(?:async\s+)?fn\s+{}\s*\([^)]*\)\s*(?:->\s*[^{{]+)?\s*\{{",
                        regex::escape(name)
                    ))?;
                    if let Some(cap) = re.find(code) {
                        let mut result = code.to_string();
                        // Find the matching closing brace
                        let start = cap.end();
                        let mut depth = 1;
                        let mut end = start;
                        for (i, c) in code[start..].chars().enumerate() {
                            match c {
                                '{' => depth += 1,
                                '}' => { depth -= 1; if depth == 0 { end = start + i; break; } }
                                _ => {}
                            }
                        }
                        result.replace_range(start..end, &format!("\n{}\n    ", new_body));
                        return Ok(result);
                    }
                    return Ok(code.to_string());
                }

                let tree = parser.parse(code, None)
                    .ok_or_else(|| anyhow::anyhow!("Parse failed"))?;
                let root = tree.root_node();

                // Find the function
                let mut cursor = root.walk();
                for node in root.children(&mut cursor) {
                    if node.kind() == "function_item" {
                        if let Some(name_node) = node.child_by_field_name("name") {
                            if let Ok(func_name) = name_node.utf8_text(code.as_bytes()) {
                                if func_name == name {
                                    // Find the body node
                                    if let Some(body) = node.child_by_field_name("body") {
                                        let start = body.start_byte() + 1; // skip {
                                        let end = body.end_byte() - 1; // skip }
                                        let mut result = code.to_string();
                                        result.replace_range(start..end, &format!("\n{}\n    ", new_body));
                                        return Ok(result);
                                    }
                                }
                            }
                        }
                    }
                }

                Ok(code.to_string())
            }
            AstEditOp::ChangeType { symbol, old_type, new_type, .. } => {
                let pattern = format!("{}: {}", symbol, old_type);
                let replacement = format!("{}: {}", symbol, new_type);
                Ok(code.replace(&pattern, &replacement))
            }

            AstEditOp::Insert { content, line, .. } => {
                let mut result = code.to_string();
                if *line > 0 && *line <= code.lines().count() {
                    result.insert_str(
                        code.lines().take(*line).map(|l| l.len()).sum::<usize>() + *line,
                        content,
                    );
                }
                Ok(result)
            }

            AstEditOp::Delete { start_line, end_line } => {
                let lines: Vec<&str> = code.lines().collect();
                let mut result = String::new();
                for (i, line) in lines.iter().enumerate() {
                    let line_num = i + 1;
                    if line_num < *start_line || line_num > *end_line {
                        result.push_str(line);
                        result.push('\n');
                    }
                }
                Ok(result)
            }

            AstEditOp::Replace { start_line, end_line, content } => {
                let lines: Vec<&str> = code.lines().collect();
                let mut result = String::new();
                for (i, line) in lines.iter().enumerate() {
                    let line_num = i + 1;
                    if line_num == *start_line {
                        result.push_str(content);
                        result.push('\n');
                    } else if line_num < *start_line || line_num > *end_line {
                        result.push_str(line);
                        result.push('\n');
                    }
                }
                Ok(result)
            }
        }
    }

    async fn find_dependents(&self, code: &str, symbol: &str) -> Vec<(usize, String)> {
        match self.language {
            LanguageKind::Rust
            | LanguageKind::TypeScript
            | LanguageKind::JavaScript
            | LanguageKind::Python
            | LanguageKind::Go => self.find_dependents_ast(code, symbol),
            _ => {
                // Simple text search fallback for unsupported languages
                let mut results = Vec::new();
                for (i, line) in code.lines().enumerate() {
                    if line.contains(symbol) {
                        results.push((i + 1, format!("Reference to '{}' at line {}", symbol, i + 1)));
                    }
                }
                results
            }
        }
    }
}
