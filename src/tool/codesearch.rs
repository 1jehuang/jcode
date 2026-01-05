use super::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

const BASE_URL: &str = "https://mcp.exa.ai/mcp";
const DEFAULT_TOKENS: u32 = 5000;
const MIN_TOKENS: u32 = 1000;
const MAX_TOKENS: u32 = 50000;
const MAX_OUTPUT_LEN: usize = 30000;

pub struct CodeSearchTool {
    client: reqwest::Client,
}

impl CodeSearchTool {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
        }
    }
}

#[derive(Deserialize)]
struct CodeSearchInput {
    query: String,
    #[serde(rename = "tokensNum", default)]
    tokens_num: Option<u32>,
}

#[derive(Deserialize)]
struct McpResponse {
    result: Option<McpResult>,
}

#[derive(Deserialize)]
struct McpResult {
    content: Vec<McpContent>,
}

#[derive(Deserialize)]
struct McpContent {
    text: String,
}

#[async_trait]
impl Tool for CodeSearchTool {
    fn name(&self) -> &str {
        "codesearch"
    }

    fn description(&self) -> &str {
        "Search for code snippets and documentation using Exa code search. \
         Use for API usage examples or library references."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["query"],
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query for APIs, libraries, or SDK usage"
                },
                "tokensNum": {
                    "type": "integer",
                    "minimum": MIN_TOKENS,
                    "maximum": MAX_TOKENS,
                    "description": "Number of tokens to return (1000-50000, default 5000)"
                }
            }
        })
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        let params: CodeSearchInput = serde_json::from_value(input)?;
        let tokens_num = params
            .tokens_num
            .unwrap_or(DEFAULT_TOKENS)
            .clamp(MIN_TOKENS, MAX_TOKENS);

        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "get_code_context_exa",
                "arguments": {
                    "query": params.query,
                    "tokensNum": tokens_num
                }
            }
        });

        let response = self
            .client
            .post(BASE_URL)
            .header("accept", "application/json, text/event-stream")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!(
                "Code search error ({}): {}",
                status,
                text
            ));
        }

        let response_text = response.text().await?;
        for line in response_text.lines() {
            if let Some(data) = line.strip_prefix("data: ") {
                if let Ok(parsed) = serde_json::from_str::<McpResponse>(data) {
                    if let Some(result) = parsed.result {
                        if let Some(first) = result.content.first() {
                            let mut output = first.text.clone();
                            if output.len() > MAX_OUTPUT_LEN {
                                output.truncate(MAX_OUTPUT_LEN);
                                output.push_str("\n... (truncated)");
                            }
                            return Ok(ToolOutput::new(output).with_title(format!("codesearch: {}", params.query)));
                        }
                    }
                }
            }
        }

        Ok(ToolOutput::new("No code snippets found. Try a different query."))
    }
}
