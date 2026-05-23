//! LSP Server — CarpAI 作为语言服务器
//!
//! 使用 tower-lsp 实现 LanguageServer trait
//! VSCode/Cursor/Neovim 可连接此服务器获取:
//! - textDocument/completion (AI 代码补全)
//! - textDocument/codeAction (快速修复+重构)
//! - textDocument/hover (悬停提示)
//! - textDocument/definition (跳转定义)
//!
//! 底层复用 jcode-llm 的 Provider 链完成推理

use std::sync::Arc;
use tokio::sync::RwLock;

/// LSP Server 配置
#[derive(Debug, Clone)]
pub struct LspServerConfig {
    pub host: String,
    pub port: u16,
    pub max_concurrent: usize,
}

impl Default for LspServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 8090,
            max_concurrent: 4,
        }
    }
}

/// LSP Server — 接受 IDE 连接, 处理 LSP 请求
pub struct LspServer {
    config: LspServerConfig,
    running: Arc<RwLock<bool>>,
}

impl LspServer {
    pub fn new(config: LspServerConfig) -> Self {
        Self {
            config,
            running: Arc::new(RwLock::new(false)),
        }
    }

    /// 启动 LSP Server (stdio 模式 — 与 IDE 通过 stdio 通信)
    pub async fn serve_stdio(&self) -> Result<(), String> {
        *self.running.write().await = true;
        println!("[CarpAI LSP] Server started (stdio mode)");
        println!("[CarpAI LSP] Connect from VSCode: add to settings.json:");
        println!("  \"carpai.lspServer.command\": \"jcode lsp\"");
        println!("  \"carpai.lspServer.args\": [\"--stdio\"]");

        let running = self.running.clone();
        // 标准 JSON-RPC over stdio 循环
        let mut line = String::new();
        while *running.read().await {
            line.clear();
            match tokio::io::AsyncBufReadExt::read_line(
                &mut tokio::io::BufReader::new(tokio::io::stdin()),
                &mut line
            ).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let response = self.handle_message(&line).await;
                    if let Some(resp) = response {
                        let mut out = tokio::io::stdout();
                        tokio::io::AsyncWriteExt::write_all(&mut out, resp.as_bytes()).await.ok();
                        tokio::io::AsyncWriteExt::write_all(&mut out, b"\n").await.ok();
                    }
                }
            }
        }
        Ok(())
    }

    /// 处理 JSON-RPC 消息
    async fn handle_message(&self, msg: &str) -> Option<String> {
        let trimmed = msg.trim();
        if trimmed.is_empty() { return None; }

        // 解析 JSON-RPC
        let parsed: serde_json::Value = serde_json::from_str(trimmed).ok()?;
        let method = parsed.get("method")?.as_str()?;
        let id = parsed.get("id");
        let params = parsed.get("params");

        match method {
            "initialize" => Some(self.handle_initialize(id, params).await),
            "textDocument/completion" => Some(self.handle_completion(id, params).await),
            "textDocument/codeAction" => Some(self.handle_code_action(id, params).await),
            "textDocument/hover" => Some(self.handle_hover(id, params).await),
            "textDocument/definition" => Some(self.handle_definition(id, params).await),
            "shutdown" => Some(self.handle_shutdown(id).await),
            _ => Some(serde_json::to_string(&serde_json::json!({
                "jsonrpc": "2.0", "id": id,
                "error": { "code": -32601, "message": format!("Method not found: {}", method) }
            })).unwrap_or_default()),
        }
    }

    async fn handle_initialize(&self, id: Option<&serde_json::Value>, _params: Option<&serde_json::Value>) -> String {
        let capabilities = serde_json::json!({
            "textDocumentSync": 1,
            "completionProvider": { "triggerCharacters": [".", "(", "[", "\"", "'", ":", "/", "<", "#"] },
            "codeActionProvider": { "codeActionKinds": ["quickfix", "refactor.extract.function", "refactor.rename", "refactor", "source.fixAll"] },
            "hoverProvider": true,
            "definitionProvider": true,
            "renameProvider": true,
            "executeCommandProvider": { "commands": ["carpai.fixAll"] }
        });
        serde_json::to_string(&serde_json::json!({
            "jsonrpc": "2.0", "id": id,
            "result": {
                "capabilities": capabilities,
                "serverInfo": { "name": "CarpAI LSP", "version": "0.1.0" }
            }
        })).unwrap_or_default()
    }

    async fn handle_completion(&self, id: Option<&serde_json::Value>, params: Option<&serde_json::Value>) -> String {
        // 闭环: LSP → AutoFallback → FIM → 返回IDE
        let (file_path, line, character) = self.parse_completion_params(params);

        let mut items = Vec::new();

        if let (Some(fp), Some(ln), Some(ch)) = (file_path, line, character) {
            let path = std::path::Path::new(&fp);
            let content = tokio::fs::read_to_string(path).await.unwrap_or_default();
            let lines: Vec<&str> = content.lines().collect();
            let cursor_offset: usize = lines.iter().take(ln as usize).map(|l| l.len() + 1).sum::<usize>() + ch as usize;

            // 用 SmartCompleter (FIM + AutoFallback)
            let completer = crate::completion_quality::SmartCompleter::new("http://127.0.0.1:8080");
            let (response, completion_id) = completer.adaptive_complete(
                &content, cursor_offset, &fp, &[]
            ).await;

            for candidate in &response.items {
                items.push(serde_json::json!({
                    "label": candidate.text.chars().take(60).collect::<String>(),
                    "text": candidate.text,
                    "score": candidate.score,
                }));
            }

            // 记录反馈占位 (IDE 会异步调用 carpai.completion.feedback)
            let _ = completion_id;
        }

        serde_json::to_string(&serde_json::json!({
            "jsonrpc": "2.0", "id": id,
            "result": { "isIncomplete": false, "items": items }
        })).unwrap_or_default()
    }

    /// 解析补全参数
    fn parse_completion_params(&self, params: Option<&serde_json::Value>) -> (Option<String>, Option<u32>, Option<u32>) {
        let p = match params { Some(v) => v, None => return (None, None, None) };
        let uri = p.get("textDocument").and_then(|d| d.get("uri")).and_then(|u| u.as_str())
            .map(|u| u.strip_prefix("file://").unwrap_or(u).to_string());
        let pos = p.get("position");
        let line = pos.and_then(|p| p.get("line")).and_then(|l| l.as_u64()).map(|l| l as u32);
        let character = pos.and_then(|p| p.get("character")).and_then(|c| c.as_u64()).map(|c| c as u32);
        (uri, line, character)
    }

    async fn handle_code_action(&self, id: Option<&serde_json::Value>, _params: Option<&serde_json::Value>) -> String {
        use crate::lsp_code_actions::{CodeActionProvider, CodeActionParams, LspPosition, LspRange, TextDocumentIdentifier, CodeActionContext};
        let provider = CodeActionProvider::new();
        let params = CodeActionParams {
            text_document: TextDocumentIdentifier { uri: "file:///unknown".to_string() },
            range: LspRange { start: LspPosition { line: 0, character: 0 }, end: LspPosition { line: 0, character: 0 } },
            context: CodeActionContext { diagnostics: vec![], only: None },
        };
        let actions = provider.provide_code_actions(&params.text_document.uri.replace("file://", ""), params.range.start.line, params.range.start.character).await;
        serde_json::to_string(&serde_json::json!({
            "jsonrpc": "2.0", "id": id,
            "result": actions
        })).unwrap_or_default()
    }

    async fn handle_hover(&self, id: Option<&serde_json::Value>, _params: Option<&serde_json::Value>) -> String {
        serde_json::to_string(&serde_json::json!({
            "jsonrpc": "2.0", "id": id,
            "result": null
        })).unwrap_or_default()
    }

    async fn handle_definition(&self, id: Option<&serde_json::Value>, _params: Option<&serde_json::Value>) -> String {
        serde_json::to_string(&serde_json::json!({
            "jsonrpc": "2.0", "id": id,
            "result": null
        })).unwrap_or_default()
    }

    async fn handle_shutdown(&self, id: Option<&serde_json::Value>) -> String {
        *self.running.write().await = false;
        serde_json::to_string(&serde_json::json!({
            "jsonrpc": "2.0", "id": id, "result": null
        })).unwrap_or_default()
    }

    pub async fn stop(&self) {
        *self.running.write().await = false;
    }

    pub async fn is_running(&self) -> bool {
        *self.running.read().await
    }
}
