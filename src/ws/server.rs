use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio_tungstenite::{accept_async, tungstenite::protocol::Message};

#[derive(Debug, Clone)]
pub struct WebSocketServer {
    port: u16,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WsRequest {
    pub id: String,
    pub method: String,
    pub params: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WsResponse {
    pub id: String,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
}

impl WebSocketServer {
    pub fn new(port: u16) -> Self {
        Self { port }
    }

    pub async fn serve(&self) -> Result<()> {
        let addr = format!("0.0.0.0:{}", self.port);
        let listener = TcpListener::bind(&addr).await?;
        
        println!("WebSocket server listening on ws://{}", addr);

        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                if let Err(e) = Self::handle_connection(stream).await {
                    eprintln!("WebSocket connection error: {}", e);
                }
            });
        }

        Ok(())
    }

    async fn handle_connection(stream: tokio::net::TcpStream) -> Result<()> {
        let ws_stream = accept_async(stream).await?;
        let (mut write, mut read) = ws_stream.split();

        while let Some(msg) = read.next().await {
            let msg = msg?;

            match msg {
                Message::Text(text) => {
                    let response = Self::process_request(&text).await;
                    let response_json = serde_json::to_string(&response)?;
                    write.send(Message::Text(response_json)).await?;
                }
                Message::Binary(_) => {
                    let response = WsResponse {
                        id: "".to_string(),
                        result: None,
                        error: Some("Binary messages not supported".to_string()),
                    };
                    let response_json = serde_json::to_string(&response)?;
                    write.send(Message::Text(response_json)).await?;
                }
                Message::Ping(_) => {
                    write.send(Message::Pong(vec![])).await?;
                }
                Message::Pong(_) => {}
                Message::Close(_) => break,
                Message::Frame(_) => {}
            }
        }

        Ok(())
    }

    async fn process_request(request_json: &str) -> WsResponse {
        let request: Result<WsRequest, _> = serde_json::from_str(request_json);

        match request {
            Ok(req) => Self::handle_request(&req).await,
            Err(e) => WsResponse {
                id: "".to_string(),
                result: None,
                error: Some(format!("Invalid request: {}", e)),
            },
        }
    }

    async fn handle_request(request: &WsRequest) -> WsResponse {
        match request.method.as_str() {
            "complete" => Self::handle_complete(request).await,
            "generate" => Self::handle_generate(request).await,
            "analyze" => Self::handle_analyze(request).await,
            "ping" => WsResponse {
                id: request.id.clone(),
                result: Some(serde_json::json!({"message": "pong"})),
                error: None,
            },
            _ => WsResponse {
                id: request.id.clone(),
                result: None,
                error: Some(format!("Unknown method: {}", request.method)),
            },
        }
    }

    async fn handle_complete(request: &WsRequest) -> WsResponse {
        let code = request.params.get("code").and_then(|v| v.as_str()).unwrap_or("");
        let language = request.params.get("language").and_then(|v| v.as_str()).unwrap_or("rust");
        
        let result = format!("// Autocompleted {} code\n{}", language, code);

        WsResponse {
            id: request.id.clone(),
            result: Some(serde_json::json!({"completion": result})),
            error: None,
        }
    }

    async fn handle_generate(request: &WsRequest) -> WsResponse {
        let prompt = request.params.get("prompt").and_then(|v| v.as_str()).unwrap_or("");
        
        let result = format!("// Generated code based on: {}\n\nfn generated_function() {{\n    // Implementation goes here\n}}", prompt);

        WsResponse {
            id: request.id.clone(),
            result: Some(serde_json::json!({"code": result})),
            error: None,
        }
    }

    async fn handle_analyze(request: &WsRequest) -> WsResponse {
        let code = request.params.get("code").and_then(|v| v.as_str()).unwrap_or("");
        let line_count = code.lines().count();
        let char_count = code.chars().count();

        WsResponse {
            id: request.id.clone(),
            result: Some(serde_json::json!({
                "line_count": line_count,
                "char_count": char_count,
                "analysis": "Code analysis completed"
            })),
            error: None,
        }
    }
}