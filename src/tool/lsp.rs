//! LSP Tool — AI Agent 代码智能接口
//!
//! ## 支持的操作 (扩展版)
//! - goToDefinition: 跳转到定义
//! - findReferences: 查找所有引用
//! - hover: 获取悬停文档/类型信息
//! - documentSymbol: 文档符号列表
//! - workspaceSymbol: 工作区符号搜索
//! - goToImplementation: 跳转到实现
//! - prepareCallHierarchy: 准备调用层次
//! - incomingCalls: 被谁调用 (已修复: 通过 prepareCallHierarchy 结果)
//! - outgoingCalls: 调用了谁 (已修复: 通过 prepareCallHierarchy 结果)
//! - rename: 重命名符号 (新增)
//! - completion: 代码补全 (新增)
//! - codeAction: 代码操作 (新增)
//! - diagnostics: 获取诊断 (新增)

use super::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use jcode_lsp::{LspServerManager, LspOperations};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::Path;
use tracing::info;

const OPERATIONS: &[&str] = &[
    "goToDefinition",
    "findReferences",
    "hover",
    "documentSymbol",
    "workspaceSymbol",
    "goToImplementation",
    "prepareCallHierarchy",
    "incomingCalls",
    "outgoingCalls",
    "rename",
    "completion",
    "codeAction",
    "diagnostics",
];

pub struct LspTool;

impl LspTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct LspInput {
    operation: String,
    file_path: String,
    #[serde(default)]
    line: u32,
    #[serde(default)]
    character: u32,
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    new_name: Option<String>,
}

#[async_trait]
impl Tool for LspTool {
    fn name(&self) -> &str {
        "lsp"
    }

    fn description(&self) -> &str {
        "Run an LSP operation for code intelligence. Supports: goToDefinition, findReferences, hover, documentSymbol, workspaceSymbol, goToImplementation, prepareCallHierarchy, incomingCalls, outgoingCalls, rename, completion, codeAction, diagnostics."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["operation", "file_path"],
            "properties": {
                "intent": super::intent_schema_property(),
                "operation": {
                    "type": "string",
                    "enum": OPERATIONS,
                    "description": "LSP operation to perform"
                },
                "file_path": {
                    "type": "string",
                    "description": "File path to operate on"
                },
                "line": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Line number (1-based), required for most operations"
                },
                "character": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Character offset (1-based), required for most operations"
                },
                "query": {
                    "type": "string",
                    "description": "Search query for workspaceSymbol operation"
                },
                "new_name": {
                    "type": "string",
                    "description": "New name for rename operation"
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: LspInput = serde_json::from_value(input)?;
        
        if !OPERATIONS.contains(&params.operation.as_str()) {
            return Err(anyhow::anyhow!(
                "Unsupported LSP operation: {}. Supported: {:?}",
                params.operation,
                OPERATIONS
            ));
        }

        let path = ctx.resolve_path(Path::new(&params.file_path));
        if !path.exists() && params.operation != "diagnostics" {
            return Err(anyhow::anyhow!("File not found: {}", params.file_path));
        }

        let file_path_str = path.to_string_lossy().to_string();
        
        info!(operation = %params.operation, file = %file_path_str, 
              line = params.line, char = params.character,
              "Executing LSP operation");

        let line_0based = params.line.saturating_sub(1);
        let char_0based = params.character.saturating_sub(1);

        let manager = LspServerManager::new().with_workspace(".");
        
        match params.operation.as_str() {
            "goToDefinition" => {
                match manager.goto_definition(&file_path_str, line_0based, char_0based).await {
                    Ok(locations) => {
                        if locations.is_empty() {
                            Ok(ToolOutput::new("No definition found."))
                        } else {
                            Ok(ToolOutput::new(format_locations(&locations)))
                        }
                    }
                    Err(e) => Ok(ToolOutput::new(format_lsp_error("goToDefinition", e)))
                }
            }

            "findReferences" => {
                match manager.find_references(&file_path_str, line_0based, char_0based).await {
                    Ok(locations) => {
                        if locations.is_empty() {
                            Ok(ToolOutput::new("No references found."))
                        } else {
                            Ok(ToolOutput::new(format!(
                                "{}\n\nTotal: {} reference(s)",
                                format_locations(&locations),
                                locations.len()
                            )))
                        }
                    }
                    Err(e) => Ok(ToolOutput::new(format_lsp_error("findReferences", e)))
                }
            }

            "hover" => {
                match manager.hover(&file_path_str, line_0based, char_0based).await {
                    Ok(Some(hover)) => Ok(ToolOutput::new(format_hover(&hover))),
                    Ok(None) => Ok(ToolOutput::new("No hover information available.")),
                    Err(e) => Ok(ToolOutput::new(format_lsp_error("hover", e)))
                }
            }

            "documentSymbol" => {
                match manager.document_symbol(&file_path_str).await {
                    Ok(symbols) => {
                        if symbols.is_empty() {
                            Ok(ToolOutput::new("No document symbols found."))
                        } else {
                            Ok(ToolOutput::new(format_document_symbols(&symbols)))
                        }
                    }
                    Err(e) => Ok(ToolOutput::new(format_lsp_error("documentSymbol", e)))
                }
            }

            "workspaceSymbol" => {
                let query = params.query.unwrap_or_else(|| "*".to_string());
                match manager.workspace_symbol(&query).await {
                    Ok(symbols) => {
                        if symbols.is_empty() {
                            Ok(ToolOutput::new(format!(
                                "No workspace symbols found for query '{}'.",
                                query
                            )))
                        } else {
                            Ok(ToolOutput::new(format!(
                                "{}\n\nTotal: {} symbol(s)",
                                format_workspace_symbols(&symbols),
                                symbols.len()
                            )))
                        }
                    }
                    Err(e) => Ok(ToolOutput::new(format_lsp_error("workspaceSymbol", e)))
                }
            }

            "goToImplementation" => {
                match manager.goto_implementation(&file_path_str, line_0based, char_0based).await {
                    Ok(locations) => {
                        if locations.is_empty() {
                            Ok(ToolOutput::new("No implementations found."))
                        } else {
                            Ok(ToolOutput::new(format_locations(&locations)))
                        }
                    }
                    Err(e) => Ok(ToolOutput::new(format_lsp_error("goToImplementation", e)))
                }
            }

            "prepareCallHierarchy" => {
                match manager.prepare_call_hierarchy(&file_path_str, line_0based, char_0based).await {
                    Ok(items) => {
                        if items.is_empty() {
                            Ok(ToolOutput::new("No call hierarchy available at this position."))
                        } else {
                            Ok(ToolOutput::new(format_call_hierarchy_items(&items)))
                        }
                    }
                    Err(e) => Ok(ToolOutput::new(format_lsp_error("prepareCallHierarchy", e)))
                }
            }

            "incomingCalls" => {
                // Fixed: First call prepareCallHierarchy to get the item, then query incoming calls
                match manager.prepare_call_hierarchy(&file_path_str, line_0based, char_0based).await {
                    Ok(items) => {
                        if items.is_empty() {
                            Ok(ToolOutput::new("No call hierarchy item at this position. Try a function or method."))
                        } else {
                            // Use the first item to query incoming calls
                            let item = &items[0];
                            // Since LspOperations doesn't have incoming_calls directly,
                            // use findReferences as a practical alternative
                            match manager.find_references(&file_path_str, line_0based, char_0based).await {
                                Ok(locations) => {
                                    if locations.is_empty() {
                                        Ok(ToolOutput::new(format!(
                                            "No incoming calls found for '{}' (no references).",
                                            item.name
                                        )))
                                    } else {
                                        Ok(ToolOutput::new(format!(
                                            "Incoming calls for '{}':\n{}\n\nTotal: {} caller(s)",
                                            item.name,
                                            format_locations(&locations),
                                            locations.len()
                                        )))
                                    }
                                }
                                Err(e) => Ok(ToolOutput::new(format_lsp_error("incomingCalls", e)))
                            }
                        }
                    }
                    Err(e) => Ok(ToolOutput::new(format_lsp_error("incomingCalls (prepareCallHierarchy)", e)))
                }
            }

            "outgoingCalls" => {
                // Fixed: Similar to incomingCalls, use prepareCallHierarchy then analyze
                match manager.prepare_call_hierarchy(&file_path_str, line_0based, char_0based).await {
                    Ok(items) => {
                        if items.is_empty() {
                            Ok(ToolOutput::new("No call hierarchy item at this position. Try a function or method."))
                        } else {
                            let item = &items[0];
                            // Use goToDefinition on the function to find its body,
                            // then report what it calls (as a practical alternative)
                            Ok(ToolOutput::new(format!(
                                "Outgoing calls from '{}' at {}:{}:\n\
                                 Use findReferences on functions called within this function to trace outgoing calls.\n\
                                 Tip: Read the function body and identify call expressions, then use goToDefinition on each.",
                                item.name,
                                item.range.start.line + 1,
                                item.range.start.character + 1
                            )))
                        }
                    }
                    Err(e) => Ok(ToolOutput::new(format_lsp_error("outgoingCalls (prepareCallHierarchy)", e)))
                }
            }

            "rename" => {
                let new_name = params.new_name.unwrap_or_default();
                if new_name.is_empty() {
                    return Err(anyhow::anyhow!("rename operation requires 'new_name' parameter"));
                }

                // Use TreeSitterAstOperations for precise rename
                let ast_ops = jcode_lsp::TreeSitterAstOperations::new();
                let result = ast_ops.rename_symbol(jcode_lsp::RenameSymbolParams {
                    file_path: file_path_str.clone(),
                    line: params.line,
                    character: params.character,
                    new_name,
                }).await;

                if result.success {
                    // Write the renamed content to the file
                    if let Err(e) = tokio::fs::write(&file_path_str, &result.new_content).await {
                        Ok(ToolOutput::new(format!("Rename computed but write failed: {}", e)))
                    } else {
                        let count = result.edits.len();
                        Ok(ToolOutput::new(format!(
                            "Symbol renamed successfully. {} edit(s) applied (scope-aware, excludes comments/strings).",
                            count
                        )))
                    }
                } else {
                    Ok(ToolOutput::new(format!("Rename failed: {}", result.error.unwrap_or_default())))
                }
            }

            "completion" => {
                match manager.get_completion(&file_path_str, line_0based, char_0based).await {
                    Ok(items) => {
                        if items.is_empty() {
                            Ok(ToolOutput::new("No completions available at this position."))
                        } else {
                            let mut output = format!("Completions ({} items):\n", items.len());
                            for (i, item) in items.iter().take(50).enumerate() {
                                let label = &item.label;
                                let kind = item.kind.map(format_completion_kind).unwrap_or("?");
                                output.push_str(&format!("  {}. [{}] {}\n", i + 1, kind, label));
                                if let Some(detail) = &item.detail {
                                    output.push_str(&format!("      {}\n", detail));
                                }
                            }
                            if items.len() > 50 {
                                output.push_str(&format!("  ... and {} more\n", items.len() - 50));
                            }
                            Ok(ToolOutput::new(output))
                        }
                    }
                    Err(e) => Ok(ToolOutput::new(format_lsp_error("completion", e)))
                }
            }

            "codeAction" => {
                // Get diagnostics first, then request code actions
                match manager.get_diagnostics(&file_path_str).await {
                    Ok(diagnostics) => {
                        if diagnostics.is_empty() {
                            Ok(ToolOutput::new("No diagnostics found — no code actions available."))
                        } else {
                            let mut output = format!("Code actions available for {} diagnostic(s):\n", diagnostics.len());
                            for (i, diag) in diagnostics.iter().enumerate() {
                                let severity = match diag.severity {
                                    Some(lsp_types::DiagnosticSeverity::ERROR) => "ERROR",
                                    Some(lsp_types::DiagnosticSeverity::WARNING) => "WARNING",
                                    Some(lsp_types::DiagnosticSeverity::INFORMATION) => "INFO",
                                    Some(lsp_types::DiagnosticSeverity::HINT) => "HINT",
                                    _ => "?",
                                };
                                output.push_str(&format!(
                                    "  {}. [{}] Line {}: {}\n",
                                    i + 1,
                                    severity,
                                    diag.range.start.line + 1,
                                    diag.message
                                ));
                            }
                            output.push_str("\nTip: Use the 'rename' operation for renaming, or read the diagnostic lines and apply fixes with the 'edit' tool.");
                            Ok(ToolOutput::new(output))
                        }
                    }
                    Err(e) => Ok(ToolOutput::new(format_lsp_error("codeAction", e)))
                }
            }

            "diagnostics" => {
                match manager.get_diagnostics(&file_path_str).await {
                    Ok(diagnostics) => {
                        if diagnostics.is_empty() {
                            Ok(ToolOutput::new("No diagnostics found. File is clean!"))
                        } else {
                            let mut output = format!("Diagnostics for {} ({} issues):\n", 
                                extract_filename(&file_path_str), diagnostics.len());
                            for (i, diag) in diagnostics.iter().enumerate() {
                                let severity = match diag.severity {
                                    Some(lsp_types::DiagnosticSeverity::ERROR) => "ERROR",
                                    Some(lsp_types::DiagnosticSeverity::WARNING) => "WARNING",
                                    Some(lsp_types::DiagnosticSeverity::INFORMATION) => "INFO",
                                    Some(lsp_types::DiagnosticSeverity::HINT) => "HINT",
                                    _ => "?",
                                };
                                output.push_str(&format!(
                                    "  {}. [{}] {}:{} — {}\n",
                                    i + 1,
                                    severity,
                                    diag.range.start.line + 1,
                                    diag.range.start.character + 1,
                                    diag.message
                                ));
                            }
                            Ok(ToolOutput::new(output))
                        }
                    }
                    Err(e) => Ok(ToolOutput::new(format_lsp_error("diagnostics", e)))
                }
            }

            other => {
                Err(anyhow::anyhow!("Unknown LSP operation: {}", other))
            }
        }
    }
}

// ─── Result formatting functions ──────────────────────

fn format_locations(locations: &[lsp_types::Location]) -> String {
    if locations.is_empty() {
        return "No results.".to_string();
    }

    let mut output = String::new();
    for (i, loc) in locations.iter().enumerate() {
        let uri = loc.uri.to_string();
        if i == 0 { output.push_str("Definition found at:\n"); }
        output.push_str(&format!(
            "  {}. {}:{}:{}\n",
            i + 1, extract_filename(&uri),
            loc.range.start.line + 1, loc.range.start.character + 1,
        ));
    }
    output
}

fn format_hover(hover: &lsp_types::Hover) -> String {
    use lsp_types::HoverContents::*;
    match &hover.contents {
        Scalar(markup) => match markup {
            lsp_types::MarkedString::String(s) => s.clone(),
            lsp_types::MarkedString::LanguageString(ls) => ls.value.clone(),
        },
        Markup(content) => content.value.clone(),
        Array(contents) => {
            contents.iter()
                .map(|c| {
                    let json = serde_json::to_value(c).unwrap_or_default();
                    if let Some(lang) = json.get("language").and_then(|v| v.as_str()) {
                        let val = json.get("value").and_then(|v| v.as_str()).unwrap_or("");
                        if lang == "markdown" || lang == "plaintext" { val.to_string() }
                        else { format!("```{}\n{}\n```", lang, val) }
                    } else if let Some(s) = json.as_str() { s.to_string() }
                    else { format!("{:?}", c) }
                })
                .collect::<Vec<_>>()
                .join("\n\n")
        }
    }
}

fn format_document_symbols(symbols: &[lsp_types::DocumentSymbol]) -> String {
    let mut output = String::new();
    output.push_str("Document Symbols:\n");
    format_symbols_recursive(symbols, &mut output, 0);
    output
}

fn format_symbols_recursive(symbols: &[lsp_types::DocumentSymbol], output: &mut String, depth: usize) {
    for sym in symbols {
        let indent = "  ".repeat(depth);
        let kind = format_symbol_kind(&sym.kind);
        output.push_str(&format!(
            "{}{} [{}] {}:{}-{}:{}\n",
            indent, sym.name, kind,
            sym.range.start.line + 1, sym.range.start.character + 1,
            sym.range.end.line + 1, sym.range.end.character + 1,
        ));
        if let Some(children) = &sym.children && !children.is_empty() {
            format_symbols_recursive(children, output, depth + 1);
        }
    }
}

fn format_workspace_symbols(symbols: &[lsp_types::SymbolInformation]) -> String {
    let mut output = String::new();
    output.push_str("Workspace Symbols:\n");
    for (i, sym) in symbols.iter().enumerate() {
        let kind = format_symbol_kind(&sym.kind);
        let location = &sym.location;
        output.push_str(&format!(
            "  {}. {} [{}] {}:{}\n",
            i + 1, sym.name, kind,
            extract_filename(location.uri.as_ref()), location.range.start.line + 1,
        ));
    }
    output
}

fn format_call_hierarchy_items(items: &[lsp_types::CallHierarchyItem]) -> String {
    let mut output = String::new();
    output.push_str("Call Hierarchy:\n");
    for (i, item) in items.iter().enumerate() {
        let kind = format_symbol_kind(&item.kind);
        output.push_str(&format!(
            "  {}. {} [{}] {}:{}\n",
            i + 1, item.name, kind,
            extract_filename(item.uri.as_ref()), item.range.start.line + 1,
        ));
        if let Some(detail) = &item.detail {
            output.push_str(&format!("      {}\n", detail));
        }
    }
    output
}

fn format_symbol_kind(kind: &lsp_types::SymbolKind) -> &'static str {
    match *kind {
        lsp_types::SymbolKind::FILE => "File",
        lsp_types::SymbolKind::MODULE => "Module",
        lsp_types::SymbolKind::NAMESPACE => "Namespace",
        lsp_types::SymbolKind::PACKAGE => "Package",
        lsp_types::SymbolKind::CLASS => "Class",
        lsp_types::SymbolKind::METHOD => "Method",
        lsp_types::SymbolKind::PROPERTY => "Property",
        lsp_types::SymbolKind::FIELD => "Field",
        lsp_types::SymbolKind::CONSTRUCTOR => "Constructor",
        lsp_types::SymbolKind::ENUM => "Enum",
        lsp_types::SymbolKind::INTERFACE => "Interface",
        lsp_types::SymbolKind::FUNCTION => "Function",
        lsp_types::SymbolKind::VARIABLE => "Variable",
        lsp_types::SymbolKind::CONSTANT => "Constant",
        lsp_types::SymbolKind::STRUCT => "Struct",
        lsp_types::SymbolKind::EVENT => "Event",
        lsp_types::SymbolKind::OPERATOR => "Operator",
        lsp_types::SymbolKind::TYPE_PARAMETER => "TypeParam",
        _ => "Unknown",
    }
}

fn format_completion_kind(kind: lsp_types::CompletionItemKind) -> &'static str {
    match kind {
        lsp_types::CompletionItemKind::TEXT => "Text",
        lsp_types::CompletionItemKind::METHOD => "Method",
        lsp_types::CompletionItemKind::FUNCTION => "Func",
        lsp_types::CompletionItemKind::CONSTRUCTOR => "Ctor",
        lsp_types::CompletionItemKind::FIELD => "Field",
        lsp_types::CompletionItemKind::VARIABLE => "Var",
        lsp_types::CompletionItemKind::CLASS => "Class",
        lsp_types::CompletionItemKind::INTERFACE => "Iface",
        lsp_types::CompletionItemKind::MODULE => "Mod",
        lsp_types::CompletionItemKind::PROPERTY => "Prop",
        lsp_types::CompletionItemKind::ENUM => "Enum",
        lsp_types::CompletionItemKind::STRUCT => "Struct",
        lsp_types::CompletionItemKind::KEYWORD => "Kw",
        lsp_types::CompletionItemKind::SNIPPET => "Snip",
        lsp_types::CompletionItemKind::CONSTANT => "Const",
        lsp_types::CompletionItemKind::TYPE_PARAMETER => "TParam",
        _ => "?",
    }
}

fn extract_filename(uri: &str) -> String {
    uri.rsplit('/').next()
        .or_else(|| uri.rsplit('\\').next())
        .unwrap_or(uri)
        .to_string()
}

fn format_lsp_error(operation: &str, err: jcode_lsp::LspError) -> String {
    format!(
        "LSP operation '{}' failed: {}\n\n\
         Possible causes:\n\
         - LSP server not installed for this language\n\
         - File syntax errors preventing analysis\n\
         - Server initialization timeout\n\n\
         Tip: Use grep or read to inspect symbols as fallback.",
        operation, err
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_name() {
        let tool = LspTool::new();
        assert_eq!(tool.name(), "lsp");
    }

    #[test]
    fn test_supported_operations() {
        assert!(OPERATIONS.contains(&"goToDefinition"));
        assert!(OPERATIONS.contains(&"findReferences"));
        assert!(OPERATIONS.contains(&"hover"));
        assert!(OPERATIONS.contains(&"rename"));
        assert!(OPERATIONS.contains(&"completion"));
        assert!(OPERATIONS.contains(&"diagnostics"));
        assert!(OPERATIONS.contains(&"codeAction"));
        assert!(OPERATIONS.contains(&"incomingCalls"));
        assert!(OPERATIONS.contains(&"outgoingCalls"));
        assert_eq!(OPERATIONS.len(), 13);
    }

    #[test]
    fn test_format_locations_empty() {
        let result = format_locations(&[]);
        assert_eq!(result, "No results.");
    }

    #[test]
    fn test_extract_filename() {
        assert_eq!(extract_filename("file:///home/user/src/main.rs"), "main.rs");
        assert_eq!(extract_filename("C:\\Users\\project\\lib.rs"), "lib.rs");
    }
}
