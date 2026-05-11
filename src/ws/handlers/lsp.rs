//! LSP 语言服务处理器
//!
//! 通过 LSP 协议提供代码智能功能：
//! - 代码补全
//! - 定义跳转
//! - 引用查找
//! - 诊断信息

use crate::ws::protocol::{WsRequest, WsResponse, CompletionItem, DiagnosticInfo, CursorPosition, MessageType};
use crate::ws::session::SessionManager;
use anyhow::Result;
use tracing::{info};

/// 处理代码补全请求
pub async fn handle_completion(
    request: &WsRequest,
    session_id: &str,
    _session_manager: &SessionManager,
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

    // TODO: 集成真正的 LSP 服务（通过 jcode-lsp crate）
    // 目前返回模拟数据用于测试
    
    let completions = generate_mock_completions(prefix);

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
    _session_manager: &SessionManager,
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

    // TODO: 调用真正的 LSP gotoDefinition
    // 返回模拟数据
    Ok(WsResponse::new(&request.id, MessageType::Response, serde_json::json!({
        "definitions": [{
            "file_path": file_path,
            "range": {
                "start": { "line": position.line.saturating_sub(5), "character": 0 },
                "end": { "line": position.line.saturating_sub(5), "character": 20 }
            },
            "symbol_name": "example_function"
        }]
    })))
}

/// 处理引用查找请求
pub async fn handle_references(
    request: &WsRequest,
    session_id: &str,
    _session_manager: &SessionManager,
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

    // TODO: 调用真正的 LSP findReferences
    Ok(WsResponse::new(&request.id, MessageType::Response, serde_json::json!({
        "references": [
            {
                "file_path": file_path,
                "range": {
                    "start": { "line": position.line, "character": position.character },
                    "end": { "line": position.line, "character": position.character + 10 }
                }
            }
        ]
    })))
}

/// 处理诊断信息请求
pub async fn handle_diagnostics(
    request: &WsRequest,
    session_id: &str,
    _session_manager: &SessionManager,
) -> Result<WsResponse> {
    let file_path = request.params.get("file_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'file_path' parameter"))?;

    info!(
        session_id = %session_id,
        file = %file_path,
        "Diagnostics requested"
    );

    // TODO: 从 LSP 服务器获取诊断信息或运行编译器检查
    // 返回模拟诊断数据
    let diagnostics = vec![
        DiagnosticInfo {
            severity: crate::ws::protocol::DiagnosticSeverity::Warning,
            message: "Unused variable".to_string(),
            start: CursorPosition { line: 10, character: 5 },
            end: CursorPosition { line: 10, character: 15 },
            source: Some("rust-analyzer".to_string()),
            code: Some("unused_variables".to_string()),
        },
    ];

    Ok(WsResponse::new(&request.id, MessageType::Response, serde_json::json!({
        "diagnostics": diagnostics,
        "file_path": file_path
    })))
}

/// 生成模拟的补全项（用于开发/测试）
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

    // 添加一些常见的 Rust 函数和宏
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
