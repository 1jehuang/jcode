use crate::server_manager::LspServerManager;
use crate::LspOperations;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;
use futures::SinkExt;
use futures::StreamExt;
use tracing::{info, warn};

pub struct RemoteLspProxy {
    lsp_manager: Arc<LspServerManager>,
}

impl RemoteLspProxy {
    pub fn new(lsp_manager: Arc<LspServerManager>) -> Self {
        Self { lsp_manager }
    }

    pub async fn serve(self, addr: SocketAddr) -> anyhow::Result<()> {
        let listener = TcpListener::bind(&addr).await?;
        info!("Remote LSP proxy on ws://{}", listener.local_addr()?);
        loop {
            let (stream, peer) = listener.accept().await?;
            let mgr = self.lsp_manager.clone();
            tokio::spawn(async move {
                if let Err(e) = proxy_conn(stream, mgr).await {
                    warn!("LSP proxy {}: {}", peer, e);
                }
            });
        }
    }
}

async fn proxy_conn(stream: TcpStream, mgr: Arc<LspServerManager>) -> anyhow::Result<()> {
    let ws = accept_async(stream).await?;
    let (mut tx, mut rx) = ws.split();
    while let Some(Ok(msg)) = rx.next().await {
        let text = match msg.to_text() { Ok(t) => t.to_string(), Err(_) => continue };
        let value = dispatch(&text, &mgr).await;
        let out = serde_json::to_string(&value)?;
        tx.send(Message::Text(out.into())).await?;
    }
    Ok(())
}

async fn dispatch(text: &str, mgr: &LspServerManager) -> Value {
    let req: Value = match serde_json::from_str(text) {
        Ok(r) => r, Err(e) => return err(None, -32700, &e.to_string()),
    };
    let id = req.get("id").cloned();
    let method = match req.get("method").and_then(Value::as_str) {
        Some(m) => m, None => return err(id, -32600, "no method"),
    };
    let p = req.get("params").cloned().unwrap_or(json!({}));
    match run_lsp(method, &p, mgr).await {
        Ok(v) => ok(id, v),
        Err(e) => err(id, -32603, &e),
    }
}

async fn run_lsp(method: &str, p: &Value, m: &LspServerManager) -> Result<Value, String> {
    let uri = p["textDocument"]["uri"].as_str().unwrap_or("");
    let line = p["position"]["line"].as_u64().unwrap_or(0) as u32;
    let col = p["position"]["character"].as_u64().unwrap_or(0) as u32;
    match method {
        "textDocument/completion" => exec(m.get_completion(uri, line, col).await),
        "textDocument/definition" => exec(m.goto_definition(uri, line, col).await),
        "textDocument/references" => exec(m.find_references(uri, line, col).await),
        "textDocument/hover" => exec_opt(m.hover(uri, line, col).await),
        "textDocument/documentSymbol" => exec(m.document_symbol(uri).await),
        "textDocument/rename" => exec(m.rename_symbol_lsp(uri, line, col, p["newName"].as_str().unwrap_or("")).await),
        "textDocument/diagnostic" => exec(m.get_diagnostics(uri).await),
        "workspace/symbol" => exec(m.workspace_symbol(p["query"].as_str().unwrap_or("")).await),
        "initialize" => Ok(json!({"capabilities":{}})),
        _ => Err(format!("unknown method: {}", method)),
    }
}

fn exec<T: serde::Serialize>(r: Result<T, crate::LspError>) -> Result<Value, String> {
    r.map(|v| serde_json::to_value(v).unwrap_or_default()).map_err(|e| e.to_string())
}

fn exec_opt<T: serde::Serialize>(r: Result<Option<T>, crate::LspError>) -> Result<Value, String> {
    r.map(|v| serde_json::to_value(v).unwrap_or(json!(null))).map_err(|e| e.to_string())
}

fn ok(id: Option<Value>, v: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":v})
}
fn err(id: Option<Value>, code: i32, msg: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":msg}})
}
