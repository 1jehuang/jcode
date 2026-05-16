//! Code Intelligence Tool — unified LSP + AST + search for AI Agent
//!
//! Combines multiple code understanding capabilities in one tool:
//! - Symbol navigation (go-to-def, find-refs, hover)
//! - Symbol search (document, workspace)
//! - Code structure (call hierarchy, implementation)
//! - Diagnostics with quick-fix suggestions
//! - AST-level analysis via tree-sitter

use super::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;

pub struct CodeIntelTool;

impl CodeIntelTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct CodeIntelInput {
    /// Operation: navigate, search, diagnose, ast, structure
    mode: String,
    /// File path
    file: Option<String>,
    /// Line number
    line: Option<u32>,
    /// Column
    column: Option<u32>,
    /// Search query
    query: Option<String>,
    /// Symbol name to analyze
    symbol: Option<String>,
}

// Lazy global LSP server manager
fn lsp_manager() -> &'static std::sync::Mutex<Option<Arc<jcode_lsp::LspServerManager>>> {
    static MGR: std::sync::OnceLock<std::sync::Mutex<Option<Arc<jcode_lsp::LspServerManager>>>> =
        std::sync::OnceLock::new();
    MGR.get_or_init(|| std::sync::Mutex::new(None))
}

async fn ensure_lsp() -> Result<Arc<jcode_lsp::LspServerManager>> {
    let cell = lsp_manager();
    let mut guard = cell.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(ref mgr) = *guard {
        return Ok(mgr.clone());
    }
    let mgr = Arc::new(jcode_lsp::LspServerManager::new());
    *guard = Some(mgr.clone());
    Ok(mgr)
}

async fn with_client<F>(file: &str, f: F) -> Result<String>
where
    F: Fn(&jcode_lsp::LspClient) -> std::pin::Pin<Box<dyn std::future::Future<Output = jcode_lsp::LspResult<serde_json::Value>> + Send>>,
{
    let mgr = ensure_lsp().await?;
    let client_lock = mgr.get_or_start_server_for_file(file).await
        .ok_or_else(|| anyhow::anyhow!("Could not start LSP server for '{}'", file))?;
    let client = client_lock.read().await;
    let result = f(&*client).await.map_err(|e| anyhow::anyhow!("LSP error: {}", e))?;
    Ok(serde_json::to_string_pretty(&result)?)
}

#[async_trait]
impl Tool for CodeIntelTool {
    fn name(&self) -> &str {
        "code_intel"
    }

    fn description(&self) -> &str {
        "Advanced code intelligence: navigate symbols, search codebase, analyze structure, get diagnostics, and perform AST-level queries. Unifies LSP (go-to-def, find-refs, hover), workspace search, and error diagnostics."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["mode"],
            "properties": {
                "intent": super::intent_schema_property(),
                "mode": {
                    "type": "string",
                    "enum": ["navigate", "search", "diagnose", "structure", "ast_query"],
                    "description": "Mode of operation"
                },
                "file": {
                    "type": "string",
                    "description": "File path (required for navigate, diagnose, structure)"
                },
                "line": {
                    "type": "integer",
                    "description": "Line number (required for navigate)"
                },
                "column": {
                    "type": "integer",
                    "description": "Column number (required for navigate)"
                },
                "query": {
                    "type": "string",
                    "description": "Search query (required for search)"
                },
                "symbol": {
                    "type": "string",
                    "description": "Symbol name (for ast_query or structure)"
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: CodeIntelInput = serde_json::from_value(input)?;

        match params.mode.as_str() {
            // Navigate: go-to-definition + hover info combined
            "navigate" => {
                let file = params.file.as_deref()
                    .ok_or_else(|| anyhow::anyhow!("'file' required for navigate mode"))?;
                let line = params.line.unwrap_or(1).saturating_sub(1);
                let col = params.column.unwrap_or(1).saturating_sub(1);
                let resolved = ctx.resolve_path(Path::new(file));

                let mgr = ensure_lsp().await?;
                let client_lock = mgr.get_or_start_server_for_file(&resolved.to_string_lossy()).await
                    .ok_or_else(|| anyhow::anyhow!("Cannot start LSP for '{}'", file))?;
                let client = client_lock.read().await;

                let mut output = format!("# Code Navigation: {}:{}:{}\n\n", file, line + 1, col + 1);

                // Hover info
                if let Ok(Some(hover)) = client.hover(&resolved.to_string_lossy(), line, col).await {
                    output.push_str("## Hover Info\n");
                    match &hover.contents {
                        lsp_types::HoverContents::Scalar(s) => match s {
                            lsp_types::MarkedString::String(t) => output.push_str(&format!("{}\n", t)),
                            lsp_types::MarkedString::LanguageString(ls) => {
                                output.push_str(&format!("```{}\n{}\n```\n", ls.language, ls.value));
                            }
                        },
                        lsp_types::HoverContents::Array(arr) => {
                            for s in arr {
                                match s {
                                    lsp_types::MarkedString::String(t) => output.push_str(&format!("{}\n", t)),
                                    lsp_types::MarkedString::LanguageString(ls) => {
                                        output.push_str(&format!("```{}\n{}\n```\n", ls.language, ls.value));
                                    }
                                }
                            }
                        }
                        lsp_types::HoverContents::Markup(m) => output.push_str(&format!("{}\n", m.value)),
                    }
                }

                // Go to definition
                if let Ok(defs) = client.goto_definition(&resolved.to_string_lossy(), line, col).await {
                    output.push_str("\n## Definitions\n");
                    if defs.is_empty() {
                        output.push_str("  (no definition found)\n");
                    } else {
                        for loc in &defs {
                            output.push_str(&format!("- {}:{}:{}\n",
                                loc.uri.as_str(), loc.range.start.line + 1, loc.range.start.character + 1));
                        }
                    }
                }

                Ok(ToolOutput::new(output).with_title("code_intel: navigate"))
            }

            // Search: workspace-wide symbol search
            "search" => {
                let query = params.query.as_deref().unwrap_or("");
                if query.is_empty() {
                    return Ok(ToolOutput::new("Query required for search mode.")
                        .with_title("code_intel: no query"));
                }

                let mgr = ensure_lsp().await?;
                let servers = mgr.list_running_servers().await;
                if servers.is_empty() {
                    return Ok(ToolOutput::new(
                        "No LSP server running. Navigate to a file first."
                    ).with_title("code_intel: no server"));
                }

                let mut output = format!("# Symbol Search: \"{}\"\n\n", query);
                for (server_name, _) in &servers {
                    let client_lock = mgr.get_or_start_server_for_file(".").await;
                    if let Some(cl) = client_lock {
                        let client = cl.read().await;
                        if let Ok(symbols) = client.workspace_symbol(query).await {
                            if !symbols.is_empty() {
                                output.push_str(&format!("## {} ({} results)\n", server_name, symbols.len()));
                                for sym in &symbols {
                                    output.push_str(&format!("- `{}` — {}:{}:{}\n",
                                        sym.name,
                                        sym.location.uri.as_str(),
                                        sym.location.range.start.line + 1,
                                        sym.location.range.start.character + 1,
                                    ));
                                }
                            }
                        }
                    }
                }

                if output.lines().count() <= 2 {
                    output.push_str("  (no matching symbols)\n");
                }
                Ok(ToolOutput::new(output).with_title("code_intel: search"))
            }

            // Diagnose: get diagnostics for a file
            "diagnose" => {
                let file = params.file.as_deref()
                    .ok_or_else(|| anyhow::anyhow!("'file' required for diagnose mode"))?;
                let resolved = ctx.resolve_path(Path::new(file));

                let mgr = ensure_lsp().await?;
                let client_lock = mgr.get_or_start_server_for_file(&resolved.to_string_lossy()).await
                    .ok_or_else(|| anyhow::anyhow!("Cannot start LSP for '{}'", file))?;
                let client = client_lock.read().await;

                let diags = client.get_diagnostics(&resolved.to_string_lossy()).await
                    .map_err(|e| anyhow::anyhow!("LSP diagnostics error: {}", e))?;

                let mut output = format!("# Diagnostics: {}\n\n", file);
                if diags.is_empty() {
                    output.push_str("✅ No errors or warnings.\n");
                } else {
                    let errors = diags.iter().filter(|d| d.severity == Some(lsp_types::DiagnosticSeverity::ERROR)).count();
                    let warnings = diags.iter().filter(|d| d.severity == Some(lsp_types::DiagnosticSeverity::WARNING)).count();
                    output.push_str(&format!("{} errors, {} warnings, {} others\n\n",
                        errors, warnings, diags.len() - errors - warnings));

                    for d in &diags {
                        let icon = match d.severity {
                            Some(lsp_types::DiagnosticSeverity::ERROR) => "❌",
                            Some(lsp_types::DiagnosticSeverity::WARNING) => "⚠️",
                            _ => "ℹ️",
                        };
                        output.push_str(&format!("{} {}:{} — {}\n",
                            icon, d.range.start.line + 1, d.range.start.character + 1, d.message));
                        if let Some(code) = &d.code {
                            output.push_str(&format!("   Code: {:?}\n", code));
                        }
                    }
                }
                Ok(ToolOutput::new(output).with_title("code_intel: diagnostics"))
            }

            // Structure: document symbols / call hierarchy
            "structure" => {
                let file = params.file.as_deref()
                    .ok_or_else(|| anyhow::anyhow!("'file' required for structure mode"))?;
                let resolved = ctx.resolve_path(Path::new(file));

                let mgr = ensure_lsp().await?;
                let client_lock = mgr.get_or_start_server_for_file(&resolved.to_string_lossy()).await
                    .ok_or_else(|| anyhow::anyhow!("Cannot start LSP for '{}'", file))?;
                let client = client_lock.read().await;

                let mut output = format!("# Code Structure: {}\n\n", file);

                // Document symbols
                if let Ok(symbols) = client.document_symbol(&resolved.to_string_lossy()).await {
                    output.push_str("## Symbols\n");
                    if symbols.is_empty() {
                        output.push_str("  (no symbols)\n");
                    }
                    for sym in &symbols {
                        let kind = match sym.kind {
                            lsp_types::SymbolKind::FUNCTION => "ƒ",
                            lsp_types::SymbolKind::METHOD => "🔧",
                            lsp_types::SymbolKind::CLASS => "🔵",
                            lsp_types::SymbolKind::STRUCT => "🔶",
                            lsp_types::SymbolKind::ENUM => "🔷",
                            lsp_types::SymbolKind::INTERFACE => "🔌",
                            lsp_types::SymbolKind::MODULE => "📦",
                            lsp_types::SymbolKind::VARIABLE => "📌",
                            lsp_types::SymbolKind::CONSTANT => "🔒",
                            _ => "❓",
                        };
                        output.push_str(&format!("- {} `{}` — {}:{}\n",
                            kind, sym.name,
                            sym.range.start.line + 1, sym.range.start.character + 1));
                        // Show children with indentation
                        for child in sym.children.iter().flatten() {
                            output.push_str(&format!("  - {} `{}` — {}:{}\n",
                                kind, child.name,
                                child.range.start.line + 1, child.range.start.character + 1));
                        }
                    }
                }

                Ok(ToolOutput::new(output).with_title("code_intel: structure"))
            }

            // AST query (requires tree-sitter)
            "ast_query" => {
                let file = params.file.as_deref()
                    .ok_or_else(|| anyhow::anyhow!("'file' required for ast_query mode"))?;
                let symbol = params.symbol.as_deref().unwrap_or("");
                let resolved = ctx.resolve_path(Path::new(file));

                let mgr = ensure_lsp().await?;

                // First try LSP workspace symbol for the symbol
                let client_lock = mgr.get_or_start_server_for_file(&resolved.to_string_lossy()).await;
                let mut output = format!("# AST Analysis: {} (symbol: {})\n\n", file, symbol);

                if let Some(cl) = client_lock {
                    let client = cl.read().await;
                    if !symbol.is_empty() {
                        if let Ok(symbols) = client.workspace_symbol(symbol).await {
                            if !symbols.is_empty() {
                                output.push_str(&format!("Found {} reference(s) for '{}':\n", symbols.len(), symbol));
                                for sym in symbols.iter().take(20) {
                                    output.push_str(&format!("- `{}` — {}:{}:{}\n",
                                        sym.name,
                                        sym.location.uri.as_str(),
                                        sym.location.range.start.line + 1,
                                        sym.location.range.start.character + 1));
                                }
                            }
                        }
                    }
                    // Get document symbols as structure overview
                    if let Ok(syms) = client.document_symbol(&resolved.to_string_lossy()).await {
                        output.push_str(&format!("\nDocument structure ({} top-level symbols):\n", syms.len()));
                        for sym in syms.iter().take(30) {
                            output.push_str(&format!("- {} — {}:{}\n", sym.name,
                                sym.range.start.line + 1, sym.range.start.character + 1));
                        }
                    }
                }

                Ok(ToolOutput::new(output).with_title("code_intel: ast"))
            }

            other => Ok(ToolOutput::new(format!(
                "Unknown mode: {}. Supported: navigate, search, diagnose, structure, ast_query",
                other
            )).with_title("code_intel: error")),
        }
    }
}
