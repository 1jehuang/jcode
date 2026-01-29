//! MCP management tool - connect, disconnect, list, reload MCP servers

#![allow(dead_code)]

use crate::mcp::{McpConfig, McpManager, McpServerConfig};
use crate::tool::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Deserialize)]
struct McpToolInput {
    action: String,
    #[serde(default)]
    server: Option<String>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    args: Option<Vec<String>>,
    #[serde(default)]
    env: Option<HashMap<String, String>>,
}

pub struct McpManagementTool {
    manager: Arc<RwLock<McpManager>>,
}

impl McpManagementTool {
    pub fn new(manager: Arc<RwLock<McpManager>>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl Tool for McpManagementTool {
    fn name(&self) -> &str {
        "mcp"
    }

    fn description(&self) -> &str {
        "Manage MCP (Model Context Protocol) servers. Connect to new servers, disconnect, list available servers and tools, or reload configuration."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "connect", "disconnect", "reload"],
                    "description": "Action to perform: 'list' shows connected servers and their tools, 'connect' adds a new server, 'disconnect' removes a server, 'reload' reloads from config file"
                },
                "server": {
                    "type": "string",
                    "description": "Server name (required for connect/disconnect)"
                },
                "command": {
                    "type": "string",
                    "description": "Command to run the MCP server (required for connect)"
                },
                "args": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Arguments to pass to the command (optional, for connect)"
                },
                "env": {
                    "type": "object",
                    "additionalProperties": {"type": "string"},
                    "description": "Environment variables for the server (optional, for connect)"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        let params: McpToolInput = serde_json::from_value(input)?;

        match params.action.as_str() {
            "list" => self.list_servers().await,
            "connect" => self.connect_server(params).await,
            "disconnect" => self.disconnect_server(params).await,
            "reload" => self.reload_config().await,
            _ => Ok(ToolOutput::new(format!(
                "Unknown action: {}. Use 'list', 'connect', 'disconnect', or 'reload'.",
                params.action
            ))),
        }
    }
}

// Helper for tests to update cached server names
impl McpManagementTool {
    pub fn manager(&self) -> &Arc<RwLock<McpManager>> {
        &self.manager
    }
}

impl McpManagementTool {
    async fn list_servers(&self) -> Result<ToolOutput> {
        let manager = self.manager.read().await;
        let servers = manager.connected_servers().await;
        let all_tools = manager.all_tools().await;

        if servers.is_empty() {
            return Ok(ToolOutput::new(
                "No MCP servers connected.\n\n\
                To connect a server, use:\n\
                {\"action\": \"connect\", \"server\": \"name\", \"command\": \"/path/to/server\", \"args\": []}\n\n\
                Or add servers to .claude/mcp.json and use {\"action\": \"reload\"}"
            ).with_title("MCP: No servers"));
        }

        let mut output = String::new();
        output.push_str(&format!("Connected MCP servers: {}\n\n", servers.len()));

        for server in &servers {
            output.push_str(&format!("## {}\n", server));
            let server_tools: Vec<_> = all_tools.iter().filter(|(s, _)| s == server).collect();

            if server_tools.is_empty() {
                output.push_str("  (no tools)\n");
            } else {
                for (_, tool) in server_tools {
                    output.push_str(&format!(
                        "  - mcp__{}_{}: {}\n",
                        server,
                        tool.name,
                        tool.description.as_deref().unwrap_or("(no description)")
                    ));
                }
            }
            output.push('\n');
        }

        Ok(ToolOutput::new(output).with_title("MCP: Server list"))
    }

    async fn connect_server(&self, params: McpToolInput) -> Result<ToolOutput> {
        let server_name = params
            .server
            .ok_or_else(|| anyhow::anyhow!("'server' is required for connect action"))?;
        let command = params
            .command
            .ok_or_else(|| anyhow::anyhow!("'command' is required for connect action"))?;

        let config = McpServerConfig {
            command,
            args: params.args.unwrap_or_default(),
            env: params.env.unwrap_or_default(),
        };

        let manager = self.manager.read().await;

        // Check if already connected
        let connected = manager.connected_servers().await;
        if connected.contains(&server_name) {
            return Ok(ToolOutput::new(format!(
                "Server '{}' is already connected. Use 'disconnect' first to reconnect.",
                server_name
            ))
            .with_title("MCP: Already connected"));
        }
        drop(manager);

        // Connect
        let manager = self.manager.read().await;
        match manager.connect(&server_name, &config).await {
            Ok(()) => {
                let tools = manager.all_tools().await;
                let server_tools: Vec<_> =
                    tools.iter().filter(|(s, _)| s == &server_name).collect();

                let mut output = format!(
                    "Connected to MCP server '{}'\n\nAvailable tools ({}):\n",
                    server_name,
                    server_tools.len()
                );
                for (_, tool) in server_tools {
                    output.push_str(&format!(
                        "  - mcp__{}_{}: {}\n",
                        server_name,
                        tool.name,
                        tool.description.as_deref().unwrap_or("(no description)")
                    ));
                }

                Ok(ToolOutput::new(output).with_title(format!("MCP: Connected {}", server_name)))
            }
            Err(e) => Ok(
                ToolOutput::new(format!("Failed to connect to '{}': {}", server_name, e))
                    .with_title("MCP: Connection failed"),
            ),
        }
    }

    async fn disconnect_server(&self, params: McpToolInput) -> Result<ToolOutput> {
        let server_name = params
            .server
            .ok_or_else(|| anyhow::anyhow!("'server' is required for disconnect action"))?;

        let manager = self.manager.read().await;
        let connected = manager.connected_servers().await;

        if !connected.contains(&server_name) {
            return Ok(ToolOutput::new(format!(
                "Server '{}' is not connected.\n\nConnected servers: {}",
                server_name,
                if connected.is_empty() {
                    "(none)".to_string()
                } else {
                    connected.join(", ")
                }
            ))
            .with_title("MCP: Not connected"));
        }
        drop(manager);

        let manager = self.manager.read().await;
        manager.disconnect(&server_name).await?;

        Ok(
            ToolOutput::new(format!("Disconnected from MCP server '{}'", server_name))
                .with_title(format!("MCP: Disconnected {}", server_name)),
        )
    }

    async fn reload_config(&self) -> Result<ToolOutput> {
        // Load fresh config
        let config = McpConfig::load();

        if config.servers.is_empty() {
            return Ok(ToolOutput::new(
                "No servers found in config.\n\n\
                Add servers to .claude/mcp.json:\n\
                {\n  \"servers\": {\n    \"server-name\": {\n      \"command\": \"/path/to/server\",\n      \"args\": [],\n      \"env\": {}\n    }\n  }\n}"
            ).with_title("MCP: Empty config"));
        }

        let mut manager = self.manager.write().await;
        let (successes, failures) = manager.reload().await?;

        let servers = manager.connected_servers().await;
        let all_tools = manager.all_tools().await;

        let mut output = format!(
            "Reloaded MCP config. Connected: {}/{}\n\n",
            successes,
            config.servers.len()
        );

        // Show failures first
        if !failures.is_empty() {
            output.push_str("## Connection Failures\n");
            for (name, error) in &failures {
                output.push_str(&format!("  - {}: {}\n", name, error));
            }
            output.push('\n');
        }

        for server in &servers {
            output.push_str(&format!("## {}\n", server));
            let server_tools: Vec<_> = all_tools.iter().filter(|(s, _)| s == server).collect();

            for (_, tool) in server_tools {
                output.push_str(&format!("  - {}\n", tool.name));
            }
            output.push('\n');
        }

        Ok(ToolOutput::new(output).with_title("MCP: Reloaded"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::Tool;
    use std::fs;
    use std::path::PathBuf;

    fn create_test_tool() -> McpManagementTool {
        let manager = Arc::new(RwLock::new(McpManager::new()));
        McpManagementTool::new(manager)
    }

    fn create_test_context() -> ToolContext {
        ToolContext {
            session_id: "test-session".to_string(),
            message_id: "test-message".to_string(),
            tool_call_id: "test-tool-call".to_string(),
        }
    }

    struct LocalMcpConfigGuard {
        path: PathBuf,
        backup: Option<String>,
        created_dir: bool,
    }

    impl LocalMcpConfigGuard {
        fn new(content: &str) -> std::io::Result<Self> {
            let path = PathBuf::from(".claude/mcp.json");
            let dir = path
                .parent()
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::Other, "missing parent"))?;
            let created_dir = if !dir.exists() {
                fs::create_dir_all(dir)?;
                true
            } else {
                false
            };
            let backup = if path.exists() {
                Some(fs::read_to_string(&path)?)
            } else {
                None
            };
            fs::write(&path, content)?;
            Ok(Self {
                path,
                backup,
                created_dir,
            })
        }
    }

    impl Drop for LocalMcpConfigGuard {
        fn drop(&mut self) {
            match &self.backup {
                Some(content) => {
                    let _ = fs::write(&self.path, content);
                }
                None => {
                    let _ = fs::remove_file(&self.path);
                    if self.created_dir {
                        if let Some(dir) = self.path.parent() {
                            let _ = fs::remove_dir(dir);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn test_tool_name() {
        let tool = create_test_tool();
        assert_eq!(tool.name(), "mcp");
    }

    #[test]
    fn test_tool_description() {
        let tool = create_test_tool();
        assert!(tool.description().contains("MCP"));
        assert!(tool.description().contains("Model Context Protocol"));
    }

    #[test]
    fn test_parameters_schema() {
        let tool = create_test_tool();
        let schema = tool.parameters_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["action"].is_object());
        assert!(schema["properties"]["server"].is_object());
        assert!(schema["properties"]["command"].is_object());
    }

    #[tokio::test]
    async fn test_list_empty() {
        let tool = create_test_tool();
        let ctx = create_test_context();
        let input = json!({"action": "list"});

        let result = tool.execute(input, ctx).await.unwrap();
        assert!(result.output.contains("No MCP servers connected"));
    }

    #[tokio::test]
    async fn test_connect_missing_server() {
        let tool = create_test_tool();
        let ctx = create_test_context();
        let input = json!({"action": "connect", "command": "/bin/test"});

        let result = tool.execute(input, ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("server"));
    }

    #[tokio::test]
    async fn test_connect_missing_command() {
        let tool = create_test_tool();
        let ctx = create_test_context();
        let input = json!({"action": "connect", "server": "test"});

        let result = tool.execute(input, ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("command"));
    }

    #[tokio::test]
    async fn test_disconnect_not_connected() {
        let tool = create_test_tool();
        let ctx = create_test_context();
        let input = json!({"action": "disconnect", "server": "nonexistent"});

        let result = tool.execute(input, ctx).await.unwrap();
        assert!(result.output.contains("not connected"));
    }

    #[tokio::test]
    async fn test_unknown_action() {
        let tool = create_test_tool();
        let ctx = create_test_context();
        let input = json!({"action": "invalid_action"});

        let result = tool.execute(input, ctx).await.unwrap();
        assert!(result.output.contains("Unknown action"));
    }

    #[tokio::test]
    async fn test_reload_empty_config() {
        let _guard = LocalMcpConfigGuard::new("{\"servers\":{}}")
            .expect("create temporary .claude/mcp.json");
        let tool = create_test_tool();
        let ctx = create_test_context();
        let input = json!({"action": "reload"});

        let result = tool.execute(input, ctx).await.unwrap();
        // Should mention empty config or show no servers
        assert!(
            result.output.contains("No servers")
                || result.output.contains("Empty config")
                || result.output.contains("Connected servers: 0")
        );
    }
}
