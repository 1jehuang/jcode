//! 基于 tree-sitter 的多语言 AST 解析器

use anyhow::Result;
use tree_sitter::{Parser, Tree};
use std::collections::HashMap;

/// 代码解析器
pub struct CodeParser {
    parser: Parser,
    languages: HashMap<String, tree_sitter::Language>,
}

impl CodeParser {
    pub fn new() -> Result<Self> {
        let parser = Parser::new();
        let mut languages = HashMap::new();

        // 注册支持的语言
        languages.insert("rust".to_string(), tree_sitter_rust::LANGUAGE.into());
        languages.insert("typescript".to_string(), tree_sitter_typescript::LANGUAGE_TSX.into());
        languages.insert("python".to_string(), tree_sitter_python::LANGUAGE.into());
        languages.insert("go".to_string(), tree_sitter_go::LANGUAGE.into());

        Ok(Self { parser, languages })
    }

    /// 提取文件中的关键符号（函数、类、接口）
    pub fn extract_symbols(&mut self, file_path: &str, content: &str) -> Result<Vec<Symbol>> {
        let ext = std::path::Path::new(file_path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");

        let lang_name = match ext {
            "rs" => "rust",
            "ts" | "tsx" | "js" | "jsx" => "typescript",
            "py" => "python",
            "go" => "go",
            _ => return Ok(vec![]),
        };

        if let Some(lang) = self.languages.get(lang_name) {
            self.parser.set_language(lang)?;
            if let Some(tree) = self.parser.parse(content, None) {
                return Ok(self.walk_tree(&tree, content));
            }
        }

        Ok(vec![])
    }

    /// 遍历 AST 树提取符号
    fn walk_tree(&self, tree: &Tree, source: &str) -> Vec<Symbol> {
        let mut symbols = Vec::new();
        let root = tree.root_node();
        let mut cursor = root.walk();

        for child in root.children(&mut cursor) {
            self.extract_from_node(child, source, &mut symbols);
        }

        symbols
    }

    /// 从节点中提取信息
    fn extract_from_node(&self, node: tree_sitter::Node, source: &str, symbols: &mut Vec<Symbol>) {
        match node.kind() {
            "function_declaration" | "method_definition" | "function_item" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = name_node.utf8_text(source.as_bytes()).unwrap_or("");
                    let body = node.utf8_text(source.as_bytes()).unwrap_or("");
                    symbols.push(Symbol {
                        name: name.to_string(),
                        kind: SymbolKind::Function,
                        content: body.to_string(),
                        start_line: node.start_position().row,
                        end_line: node.end_position().row,
                    });
                }
            }
            "class_declaration" | "struct_item" | "interface_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = name_node.utf8_text(source.as_bytes()).unwrap_or("");
                    symbols.push(Symbol {
                        name: name.to_string(),
                        kind: SymbolKind::Class,
                        content: node.utf8_text(source.as_bytes()).unwrap_or("").to_string(),
                        start_line: node.start_position().row,
                        end_line: node.end_position().row,
                    });
                }
            }
            _ => {}
        }

        // 递归处理子节点
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.extract_from_node(child, source, symbols);
        }
    }
}

/// 代码符号
#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub content: String,
    pub start_line: usize,
    pub end_line: usize,
}

/// 符号类型
#[derive(Debug, Clone)]
pub enum SymbolKind {
    Function,
    Class,
    Interface,
    Variable,
}
