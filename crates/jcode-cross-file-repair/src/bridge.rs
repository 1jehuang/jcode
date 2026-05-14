//! Bridge module: convert `AstEdit` → `FileSet` for the `jcode-multi-file-edit` crate.
//!
//! This module bridges the semantic-level operations of the cross-file repair engine
//! (symbol names, import paths) to the line-level operations of the multi-file atomic
//! edit engine (line numbers, content strings).

use std::collections::HashMap;
use std::path::PathBuf;

use crate::ast::{AstAdapter, AstEdit, AstEditOp, AstNode, LanguageKind, TreeSitterAstAdapter};
use jcode_multi_file_edit::{FileSet, FileOperation, FileEditOp};

/// Bridge converter: AstEdit → FileSet
pub struct EditBridge {
    ast_adapter: TreeSitterAstAdapter,
}

impl EditBridge {
    pub fn new(ast_adapter: TreeSitterAstAdapter) -> Self {
        Self { ast_adapter }
    }

    /// Convert a batch of `AstEdit`s into a single `FileSet` for atomic commit.
    ///
    /// Each `AstEdit` maps to one `FileOperation`. Within each, `AstEditOp` variants
    /// are resolved to line-number-based `FileEditOp`s using the AST adapter.
    pub async fn convert(&self, edits: Vec<AstEdit>) -> anyhow::Result<FileSet> {
        let mut files: Vec<FileOperation> = Vec::new();

        for edit in &edits {
            let file_path = PathBuf::from(&edit.file_path);
            let file_ops = self.resolve_operations(edit).await?;
            files.push(FileOperation {
                file_path,
                edits: file_ops,
            });
        }

        Ok(FileSet {
            files,
            description: format!(
                "Cross-file repair: {} files, {} operations",
                edits.len(),
                edits.iter().map(|e| e.operations.len()).sum::<usize>()
            ),
        })
    }

    /// Resolve all `AstEditOp`s for a single file into `FileEditOp`s.
    async fn resolve_operations(&self, edit: &AstEdit) -> anyhow::Result<Vec<FileEditOp>> {
        let source = tokio::fs::read_to_string(&edit.file_path).await?;
        let ast = self.ast_adapter.parse(&source, edit.language)?;

        let mut ops = Vec::new();

        for op in &edit.operations {
            match op {
                AstEditOp::ReplaceFunction { name, new_body } => {
                    if let Some(node) = find_function_node(&ast, name) {
                        ops.push(FileEditOp::Replace {
                            start_line: node.start_line,
                            end_line: node.end_line,
                            new_content: new_body.clone(),
                        });
                    } else {
                        tracing::warn!(
                            "ReplaceFunction: function '{}' not found in AST, appending at EOF",
                            name
                        );
                        ops.push(FileEditOp::Insert {
                            line: count_lines(&source) + 1,
                            content: format!("\n{new_body}"),
                        });
                    }
                }

                AstEditOp::AddImport { import } => {
                    let insert_line = find_last_import_line(&ast).unwrap_or(1);
                    ops.push(FileEditOp::Insert {
                        line: insert_line + 1,
                        content: format!("{import}\n"),
                    });
                }

                AstEditOp::RemoveImport { import } => {
                    if let Some(node) = find_import_node(&ast, import) {
                        ops.push(FileEditOp::Delete {
                            start_line: node.start_line,
                            end_line: node.end_line,
                        });
                    } else {
                        tracing::warn!(
                            "RemoveImport: import '{}' not found in AST, skipping",
                            import
                        );
                    }
                }

                AstEditOp::ChangeType {
                    symbol,
                    old_type,
                    new_type,
                } => {
                    if let Some((line, line_content)) =
                        find_symbol_with_type(&source, symbol, old_type)
                    {
                        let replaced = line_content.replace(old_type, new_type);
                        ops.push(FileEditOp::Replace {
                            start_line: line,
                            end_line: line,
                            new_content: replaced,
                        });
                    } else {
                        tracing::warn!(
                            "ChangeType: symbol '{}' with type '{}' not found, skipping",
                            symbol,
                            old_type
                        );
                    }
                }

                AstEditOp::RenameSymbol {
                    old_name,
                    new_name,
                    scope,
                } => {
                    let occurrences = find_symbol_occurrences(&source, old_name, scope);
                    if occurrences.is_empty() {
                        tracing::warn!(
                            "RenameSymbol: '{}' not found in scope '{}', skipping",
                            old_name,
                            scope
                        );
                    }
                    // Deduplicate lines (multiple occurrences on same line)
                    let mut seen_lines: HashMap<usize, String> = HashMap::new();
                    for (line_num, line_content) in &occurrences {
                        let replaced = line_content.replace(old_name, new_name);
                        seen_lines.entry(*line_num).or_insert(replaced);
                    }
                    for (line_num, new_content) in seen_lines {
                        ops.push(FileEditOp::Replace {
                            start_line: line_num,
                            end_line: line_num,
                            new_content,
                        });
                    }
                }
            }
        }

        Ok(ops)
    }
}

// ── Helper functions ──────────────────────────────────────────

fn count_lines(source: &str) -> usize {
    source.lines().count()
}

/// Find a function/class definition node by name in the AST.
fn find_function_node<'a>(node: &'a AstNode, name: &str) -> Option<&'a AstNode> {
    if node.kind == "function_item"
        || node.kind == "function_declaration"
        || node.kind == "function_definition"
        || node.kind == "method_definition"
    {
        if node.name.as_deref() == Some(name) {
            return Some(node);
        }
    }
    for child in &node.children {
        if let Some(found) = find_function_node(child, name) {
            return Some(found);
        }
    }
    None
}

/// Find the line number of the last import/use statement in the AST.
fn find_last_import_line(ast: &AstNode) -> Option<usize> {
    let mut max_line: Option<usize> = None;
    find_last_import_line_inner(ast, &mut max_line);
    max_line
}

fn find_last_import_line_inner(node: &AstNode, max_line: &mut Option<usize>) {
    if node.kind == "use_declaration"
        || node.kind == "import_statement"
        || node.kind == "import_declaration"
    {
        *max_line = Some(match *max_line {
            Some(current) => current.max(node.end_line),
            None => node.end_line,
        });
    }
    for child in &node.children {
        find_last_import_line_inner(child, max_line);
    }
}

/// Find an import node matching the given import path.
fn find_import_node<'a>(node: &'a AstNode, import: &str) -> Option<&'a AstNode> {
    if (node.kind == "use_declaration"
        || node.kind == "import_statement"
        || node.kind == "import_declaration")
        && node.name.as_deref() == Some(import)
    {
        return Some(node);
    }
    for child in &node.children {
        if let Some(found) = find_import_node(child, import) {
            return Some(found);
        }
    }
    None
}

/// Find a line containing `symbol: old_type` pattern (simple text-based).
fn find_symbol_with_type(source: &str, symbol: &str, old_type: &str) -> Option<(usize, String)> {
    for (i, line) in source.lines().enumerate() {
        if line.contains(symbol) && line.contains(old_type) {
            return Some((i + 1, line.to_string()));
        }
    }
    None
}

/// Find all occurrences of a symbol name within a scope (file-level or function-level).
fn find_symbol_occurrences(source: &str, symbol: &str, _scope: &str) -> Vec<(usize, String)> {
    let mut results = Vec::new();
    for (i, line) in source.lines().enumerate() {
        if line.contains(symbol) {
            results.push((i + 1, line.to_string()));
        }
    }
    results
}
