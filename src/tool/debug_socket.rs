//! Debug socket tool - send commands to the jcode debug socket
//!
//! This tool provides direct access to the debug socket API, allowing the agent
//! to control visual debugging, spawn test instances, and inspect agent state.

use crate::protocol::{Request, ServerEvent};
use crate::server;
use crate::tool::{Tool, ToolContext, ToolOutput};
use crate::transport::Stream;
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[derive(Debug, Deserialize)]
struct DebugSocketInput {
    command: String,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    timeout_secs: Option<u64>,
}

pub struct DebugSocketTool;

impl DebugSocketTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for DebugSocketTool {
    fn name(&self) -> &str {
        "debug_socket"
    }

    fn description(&self) -> &str {
        "Send a debug socket command."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Debug command."
                },
                "session_id": {
                    "type": "string",
                    "description": "Target session ID."
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "Timeout in seconds."
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        let params: DebugSocketInput = serde_json::from_value(input)?;
        let timeout_secs = params.timeout_secs.unwrap_or(30);

        // Build title based on command namespace
        let title = if params.command.starts_with("client:") {
            format!("debug_socket {}", params.command)
        } else if params.command.starts_with("tester:") {
            format!("debug_socket {}", params.command)
        } else {
            format!("debug_socket server:{}", params.command)
        };

        let result = execute_debug_command(&params.command, params.session_id, timeout_secs).await;

        match result {
            Ok(output) => Ok(ToolOutput::new(output).with_title(title)),
            Err(e) => Ok(ToolOutput::new(format!("Error: {}", e)).with_title(title)),
        }
    }
}

/// Execute a debug command via the debug socket
async fn execute_debug_command(
    command: &str,
    session_id: Option<String>,
    timeout_secs: u64,
) -> Result<String> {
    let socket_path = server::debug_socket_path();

    // Connect to debug socket
    let stream = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        Stream::connect(&socket_path),
    )
    .await
    .map_err(|_| anyhow::anyhow!("Timeout connecting to debug socket"))?
    .map_err(|e| {
        anyhow::anyhow!(
            "Failed to connect to debug socket at {}: {}",
            socket_path.display(),
            e
        )
    })?;

    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    // Build request
    let request = Request::DebugCommand {
        id: 1,
        command: command.to_string(),
        session_id,
    };

    let json = serde_json::to_string(&request)? + "\n";
    writer.write_all(json.as_bytes()).await?;

    // Read response with timeout
    let mut line = String::new();
    let read_result = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        reader.read_line(&mut line),
    )
    .await
    .map_err(|_| anyhow::anyhow!("Timeout waiting for response ({}s)", timeout_secs))?;

    read_result?;

    // Parse response
    let event: ServerEvent = serde_json::from_str(&line)
        .map_err(|e| anyhow::anyhow!("Failed to parse response: {}", e))?;

    match event {
        ServerEvent::DebugResponse { ok, output, .. } => {
            if ok {
                Ok(output)
            } else {
                Err(anyhow::anyhow!("{}", output))
            }
        }
        ServerEvent::Error { message, .. } => Err(anyhow::anyhow!("{}", message)),
        _ => Err(anyhow::anyhow!("Unexpected response: {:?}", line.trim())),
    }
}
