//! LSP 语言服务处理器
//!
//! 通过 jcode-lsp crate 提供代码智能功能：
//! - 代码补全 (textDocument/completion)
//! - 定义跳转 (textDocument/definition)
//! - 引用查找 (textDocument/references)
//! - 诊断信息 (textDocument/publishDiagnostics)

use crate::ws::protocol::{WsRequest, WsResponse, CompletionItem, DiagnosticInfo, CursorPosition, MessageType};
use crate::ws::session::SessionManager;
use anyhow::Result;
use jcode_lsp::{LspOperations, LspServerManager};
use std::sync::Arc;
use tracing::{info, warn};

/// 处理代码补全请求
pub async fn handle_completion(
    request: &WsRequest,
    session_id: &str,
    session_manager: &SessionManager,
) -> Result<WsResponse> {
    let file_path = request.params.get("file_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'file_path' parameter"))?;

    let line: u32 = request.params.get("line")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32)
        .ok_or_else(|| anyhow::anyhow!("Missing 'line' parameter"))?;

    let character: u32 = request.params.get("character")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32)
        .ok_or_else(|| anyhow::anyhow!("Missing 'character' parameter"))?;

    let prefix = request.params.get("prefix")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    info!(
        session_id = %session_id,
        file = %file_path,
        line = line,
        character = character,
        prefix = %prefix,
        "Completion requested"
    );

    let completions: Vec<CompletionItem> = match get_lsp_manager(session_manager).await {
        Some(manager) => {
            match manager.get_completion(file_path, line, character).await {
                Ok(items) => items.into_iter().map(|item| CompletionItem {
                    label: item.label,
                    detail: item.detail,
                    documentation: item.documentation.map(|doc| match doc {
                        lsp_types::Documentation::String(s) => s,
                        lsp_types::Documentation::MarkupContent(mc) => mc.value,
                    }),
                    kind: item.kind.map(|k| format!("{:?}", k)),
                    insert_text: item.insert_text.unwrap_or_default(),
                    sort_priority: item.sort_text
                        .and_then(|s: String| s.parse::<i32>().ok())
                        .unwrap_or(0),
                }).collect(),
                Err(e) => {
                    warn!(error = %e, "LSP completion failed, falling back to mock");
                    generate_mock_completions(prefix)
                }
            }
        }
        None => generate_mock_completions(prefix),
    };

    Ok(WsResponse::new(&request.id, MessageType::Response, serde_json::json!({
        "completions": completions,
        "is_incomplete": false,
        "request": {
            "file_path": file_path,
            "position": { "line": line, "character": character }
        }
    })))
}

/// 处理定义跳转请求
pub async fn handle_definition(
    request: &WsRequest,
    session_id: &str,
    session_manager: &SessionManager,
) -> Result<WsResponse> {
    let file_path = request.params.get("file_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'file_path' parameter"))?;

    let position: CursorPosition = serde_json::from_value(
        request.params.get("position").cloned().unwrap_or_default()
    ).unwrap_or(CursorPosition { line: 0, character: 0 });

    info!(
        session_id = %session_id,
        file = %file_path,
        line = position.line,
        character = position.character,
        "Definition requested"
    );

    let definitions: Vec<serde_json::Value> = match get_lsp_manager(session_manager).await {
        Some(manager) => {
            match manager.goto_definition(file_path, position.line, position.character).await {
                Ok(locations) => locations.into_iter().map(|loc| {
                    let (uri, range) = match loc {
                        lsp_types::Location { uri, range } => (uri, range),
                    };
                    serde_json::json!({
                        "file_path": uri.to_string(),
                        "range": {
                            "start": { "line": range.start.line, "character": range.start.character },
                            "end": { "line": range.end.line, "character": range.end.character }
                        }
                    })
                }).collect(),
                Err(e) => {
                    warn!(error = %e, "LSP goto_definition failed, returning empty");
                    vec![]
                }
            }
        }
        None => vec![],
    };

    Ok(WsResponse::new(&request.id, MessageType::Response, serde_json::json!({
        "definitions": definitions
    })))
}

/// 处理引用查找请求
pub async fn handle_references(
    request: &WsRequest,
    session_id: &str,
    session_manager: &SessionManager,
) -> Result<WsResponse> {
    let file_path = request.params.get("file_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'file_path' parameter"))?;

    let position: CursorPosition = serde_json::from_value(
        request.params.get("position").cloned().unwrap_or_default()
    ).unwrap_or(CursorPosition { line: 0, character: 0 });

    info!(
        session_id = %session_id,
        file = %file_path,
        line = position.line,
        character = position.character,
        "References requested"
    );

    let references: Vec<serde_json::Value> = match get_lsp_manager(session_manager).await {
        Some(manager) => {
            match manager.find_references(file_path, position.line, position.character).await {
                Ok(locations) => locations.into_iter().map(|loc| {
                    let (uri, range) = match loc {
                        lsp_types::Location { uri, range } => (uri, range),
                    };
                    serde_json::json!({
                        "file_path": uri.to_string(),
                        "range": {
                            "start": { "line": range.start.line, "character": range.start.character },
                            "end": { "line": range.end.line, "character": range.end.character }
                        }
                    })
                }).collect(),
                Err(e) => {
                    warn!(error = %e, "LSP find_references failed, returning empty");
                    vec![]
                }
            }
        }
        None => vec![],
    };

    Ok(WsResponse::new(&request.id, MessageType::Response, serde_json::json!({
        "references": references
    })))
}

/// 处理诊断信息请求
pub async fn handle_diagnostics(
    request: &WsRequest,
    session_id: &str,
    session_manager: &SessionManager,
) -> Result<WsResponse> {
    let file_path = request.params.get("file_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'file_path' parameter"))?;

    info!(
        session_id = %session_id,
        file = %file_path,
        "Diagnostics requested"
    );

    let diagnostics: Vec<DiagnosticInfo> = match get_lsp_manager(session_manager).await {
        Some(manager) => {
            match manager.get_diagnostics(file_path).await {
                Ok(diags) => diags.into_iter().map(|d| DiagnosticInfo {
                    severity: match d.severity {
                        Some(lsp_types::DiagnosticSeverity::ERROR) => crate::ws::protocol::DiagnosticSeverity::Error,
                        Some(lsp_types::DiagnosticSeverity::WARNING) => crate::ws::protocol::DiagnosticSeverity::Warning,
                        Some(lsp_types::DiagnosticSeverity::INFORMATION) => crate::ws::protocol::DiagnosticSeverity::Information,
                        Some(lsp_types::DiagnosticSeverity::HINT) => crate::ws::protocol::DiagnosticSeverity::Hint,
                        _ => crate::ws::protocol::DiagnosticSeverity::Information,
                    },
                    message: d.message,
                    start: CursorPosition {
                        line: d.range.start.line,
                        character: d.range.start.character,
                    },
                    end: CursorPosition {
                        line: d.range.end.line,
                        character: d.range.end.character,
                    },
                    source: d.source,
                    code: d.code.map(|c| match c {
                        lsp_types::NumberOrString::Number(n) => n.to_string(),
                        lsp_types::NumberOrString::String(s) => s,
                    }),
                }).collect(),
                Err(e) => {
                    warn!(error = %e, "LSP get_diagnostics failed, returning empty");
                    vec![]
                }
            }
        }
        None => vec![],
    };

    Ok(WsResponse::new(&request.id, MessageType::Response, serde_json::json!({
        "diagnostics": diagnostics,
        "file_path": file_path
    })))
}

/// 从 SessionManager 获取 LspServerManager 实例
async fn get_lsp_manager(session_manager: &SessionManager) -> Option<Arc<LspServerManager>> {
    session_manager.lsp_manager()
        .await
}

/// 生成模拟的补全项（用于 LSP 不可用时的降级）
fn generate_mock_completions(prefix: &str) -> Vec<CompletionItem> {
    let mut items = Vec::new();

    if prefix.is_empty() || prefix.starts_with("fn") {
        items.push(CompletionItem {
            label: "function".to_string(),
            detail: Some("Define a function".to_string()),
            documentation: Some("Creates a new function definition".to_string()),
            kind: Some("keyword".to_string()),
            insert_text: "fn name() {\n    \n}".to_string(),
            sort_priority: 100,
        });
    }

    if prefix.is_empty() || prefix.starts_with("let") {
        items.push(CompletionItem {
            label: "let".to_string(),
            detail: Some("Declare variable".to_string()),
            documentation: Some("Declares a new immutable variable binding".to_string()),
            kind: Some("keyword".to_string()),
            insert_text: "let  = ;".to_string(),
            sort_priority: 99,
        });
    }

    if prefix.is_empty() || prefix.starts_with("match") {
        items.push(CompletionItem {
            label: "match".to_string(),
            detail: Some("Pattern matching".to_string()),
            documentation: Some("Control flow based on pattern matching".to_string()),
            kind: Some("keyword".to_string()),
            insert_text: "match  {\n    => {\n        \n    }\n}".to_string(),
            sort_priority: 98,
        });
    }

    if prefix.is_empty() || prefix.starts_with("if") {
        items.push(CompletionItem {
            label: "if".to_string(),
            detail: Some("Conditional".to_string()),
            documentation: Some("Conditional execution based on a condition".to_string()),
            kind: Some("keyword".to_string()),
            insert_text: "if  {\n    \n}".to_string(),
            sort_priority: 97,
        });
    }

    if prefix.is_empty() || prefix.starts_with("for") {
        items.push(CompletionItem {
            label: "for".to_string(),
            detail: Some("Loop".to_string()),
            documentation: Some("Iterates over a range or iterator".to_string()),
            kind: Some("keyword".to_string()),
            insert_text: "for  in  {\n    \n}".to_string(),
            sort_priority: 96,
        });
    }

    let common_items = [
        ("println!", "Print to stdout", "Macro for printing with newline", "macro", "println!(\"{}\", );"),
        ("vec![]", "Create vector", "Macro to create a vector literal", "macro", "vec![];"),
        ("Vec::new()", "New empty vector", "Creates a new empty Vec<T>", "function", "Vec::new();"),
        ("String::new()", "New empty string", "Creates a new empty String", "function", "String::new();"),
        ("Some()", "Create Option::Some", "Wraps value in Some", "function", "Some();"),
        ("None", "Option::None value", "Represents no value", "constant", "None"),
        ("Ok()", "Result::Ok", "Success variant of Result", "function", "Ok();"),
        ("Err()", "Result::Err", "Error variant of Result", "function", "Err();"),
        ("self", "Current instance", "Reference to the current object", "keyword", "self"),
    ];

    for (label, detail, doc, kind, insert_text) in common_items.iter() {
        if prefix.is_empty() || label.starts_with(prefix) {
            items.push(CompletionItem {
                label: label.to_string(),
                detail: Some(detail.to_string()),
                documentation: Some(doc.to_string()),
                kind: Some(kind.to_string()),
                insert_text: insert_text.to_string(),
                sort_priority: 50 + (items.len() as i32),
            });
        }
    }

    items
}
