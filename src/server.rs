use crate::agent::Agent;
use crate::provider::Provider;
use crate::tool::Registry;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;

/// Default socket path
pub fn socket_path() -> PathBuf {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    runtime_dir.join("jcode.sock")
}

/// JSON-RPC request
#[derive(Debug, Deserialize)]
struct Request {
    id: u64,
    method: String,
    params: serde_json::Value,
}

/// JSON-RPC response
#[derive(Debug, Serialize, Deserialize)]
struct Response {
    id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// Server state
pub struct Server {
    agent: Arc<Mutex<Agent>>,
    socket_path: PathBuf,
}

impl Server {
    pub fn new(provider: Box<dyn Provider>, registry: Registry) -> Self {
        let agent = Agent::new(provider, registry);
        Self {
            agent: Arc::new(Mutex::new(agent)),
            socket_path: socket_path(),
        }
    }

    /// Start the server
    pub async fn run(&self) -> Result<()> {
        // Remove existing socket
        let _ = std::fs::remove_file(&self.socket_path);

        let listener = UnixListener::bind(&self.socket_path)?;
        eprintln!("Server listening on {:?}", self.socket_path);

        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let agent = Arc::clone(&self.agent);
                    tokio::spawn(async move {
                        if let Err(e) = handle_client(stream, agent).await {
                            eprintln!("Client error: {}", e);
                        }
                    });
                }
                Err(e) => {
                    eprintln!("Accept error: {}", e);
                }
            }
        }
    }
}

async fn handle_client(stream: UnixStream, agent: Arc<Mutex<Agent>>) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();

    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            break; // Client disconnected
        }

        let request: Request = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                let response = Response {
                    id: 0,
                    result: None,
                    error: Some(format!("Invalid request: {}", e)),
                };
                let mut resp_json = serde_json::to_string(&response)?;
                resp_json.push('\n');
                writer.write_all(resp_json.as_bytes()).await?;
                continue;
            }
        };

        let response = handle_request(&agent, request).await;
        let mut resp_json = serde_json::to_string(&response)?;
        resp_json.push('\n');
        writer.write_all(resp_json.as_bytes()).await?;
    }

    Ok(())
}

async fn handle_request(agent: &Arc<Mutex<Agent>>, request: Request) -> Response {
    match request.method.as_str() {
        "message" => {
            let message = request.params.get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            let mut agent = agent.lock().await;
            match agent.run_once(message).await {
                Ok(()) => Response {
                    id: request.id,
                    result: Some(serde_json::json!({"status": "ok"})),
                    error: None,
                },
                Err(e) => Response {
                    id: request.id,
                    result: None,
                    error: Some(e.to_string()),
                },
            }
        }
        "ping" => Response {
            id: request.id,
            result: Some(serde_json::json!({"pong": true})),
            error: None,
        },
        "clear" => {
            let mut agent = agent.lock().await;
            agent.clear();
            Response {
                id: request.id,
                result: Some(serde_json::json!({"cleared": true})),
                error: None,
            }
        }
        _ => Response {
            id: request.id,
            result: None,
            error: Some(format!("Unknown method: {}", request.method)),
        },
    }
}

/// Client for connecting to a running server
pub struct Client {
    stream: UnixStream,
    next_id: u64,
}

impl Client {
    pub async fn connect() -> Result<Self> {
        let path = socket_path();
        let stream = UnixStream::connect(&path).await?;
        Ok(Self { stream, next_id: 1 })
    }

    pub async fn send_message(&mut self, content: &str) -> Result<serde_json::Value> {
        let request = serde_json::json!({
            "id": self.next_id,
            "method": "message",
            "params": {"content": content}
        });
        self.next_id += 1;

        let mut req_json = serde_json::to_string(&request)?;
        req_json.push('\n');

        self.stream.write_all(req_json.as_bytes()).await?;

        let mut reader = BufReader::new(&mut self.stream);
        let mut line = String::new();
        reader.read_line(&mut line).await?;

        let response: Response = serde_json::from_str(&line)?;

        if let Some(error) = response.error {
            anyhow::bail!(error);
        }

        Ok(response.result.unwrap_or(serde_json::Value::Null))
    }

    pub async fn ping(&mut self) -> Result<bool> {
        let request = serde_json::json!({
            "id": self.next_id,
            "method": "ping",
            "params": {}
        });
        self.next_id += 1;

        let mut req_json = serde_json::to_string(&request)?;
        req_json.push('\n');

        self.stream.write_all(req_json.as_bytes()).await?;

        let mut reader = BufReader::new(&mut self.stream);
        let mut line = String::new();
        reader.read_line(&mut line).await?;

        let response: Response = serde_json::from_str(&line)?;
        Ok(response.error.is_none())
    }
}
