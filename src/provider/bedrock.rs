use super::{EventStream, Provider};
use crate::message::{
    ContentBlock as JContentBlock, Message as JMessage, Role as JRole, StreamEvent, ToolDefinition,
};
use anyhow::Result;
use async_trait::async_trait;
use aws_config::BehaviorVersion;
use aws_sdk_bedrockruntime::Client;
use aws_sdk_bedrockruntime::types::{
    ContentBlock, ContentBlockDelta, ContentBlockStart, ConversationRole, ConverseStreamOutput,
    Message, ReasoningContentBlockDelta, SystemContentBlock, Tool, ToolConfiguration,
    ToolInputSchema, ToolSpecification,
};
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

const DEFAULT_MODEL: &str = "anthropic.claude-3-5-sonnet-20241022-v2:0";

pub struct BedrockProvider {
    model: Arc<RwLock<String>>,
}

impl BedrockProvider {
    pub fn new() -> Self {
        let model =
            std::env::var("JCODE_BEDROCK_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());
        Self {
            model: Arc::new(RwLock::new(model)),
        }
    }

    pub fn has_credentials() -> bool {
        std::env::var_os("AWS_ACCESS_KEY_ID").is_some()
            || std::env::var_os("AWS_PROFILE").is_some()
            || std::env::var_os("AWS_WEB_IDENTITY_TOKEN_FILE").is_some()
            || std::env::var_os("AWS_CONTAINER_CREDENTIALS_RELATIVE_URI").is_some()
            || std::env::var_os("AWS_CONTAINER_CREDENTIALS_FULL_URI").is_some()
            || std::env::var_os("AWS_REGION").is_some()
            || std::env::var_os("JCODE_BEDROCK_REGION").is_some()
    }

    async fn client() -> Client {
        let mut loader = aws_config::defaults(BehaviorVersion::latest());
        if let Ok(region) =
            std::env::var("JCODE_BEDROCK_REGION").or_else(|_| std::env::var("AWS_REGION"))
        {
            loader = loader.region(aws_types::region::Region::new(region));
        }
        if let Ok(profile) =
            std::env::var("JCODE_BEDROCK_PROFILE").or_else(|_| std::env::var("AWS_PROFILE"))
        {
            loader = loader.profile_name(profile);
        }
        let config = loader.load().await;
        Client::new(&config)
    }

    fn json_to_document(value: &serde_json::Value) -> aws_smithy_types::Document {
        match value {
            serde_json::Value::Null => aws_smithy_types::Document::Null,
            serde_json::Value::Bool(v) => aws_smithy_types::Document::Bool(*v),
            serde_json::Value::Number(n) => {
                if let Some(v) = n.as_u64() {
                    aws_smithy_types::Document::from(v)
                } else if let Some(v) = n.as_i64() {
                    aws_smithy_types::Document::from(v)
                } else if let Some(v) = n.as_f64() {
                    aws_smithy_types::Document::from(v)
                } else {
                    aws_smithy_types::Document::Null
                }
            }
            serde_json::Value::String(v) => aws_smithy_types::Document::String(v.clone()),
            serde_json::Value::Array(values) => aws_smithy_types::Document::Array(
                values.iter().map(Self::json_to_document).collect(),
            ),
            serde_json::Value::Object(map) => aws_smithy_types::Document::Object(
                map.iter()
                    .map(|(key, value)| (key.clone(), Self::json_to_document(value)))
                    .collect::<HashMap<_, _>>(),
            ),
        }
    }

    fn to_bedrock_messages(messages: &[JMessage]) -> Vec<Message> {
        messages
            .iter()
            .filter_map(|msg| {
                let role = match msg.role {
                    JRole::User => ConversationRole::User,
                    JRole::Assistant => ConversationRole::Assistant,
                };
                let mut content = Vec::new();
                for block in &msg.content {
                    match block {
                        JContentBlock::Text { text, .. } => {
                            content.push(ContentBlock::Text(text.clone()))
                        }
                        JContentBlock::ToolResult {
                            tool_use_id,
                            content: text,
                            is_error,
                        } => {
                            let status = if is_error.unwrap_or(false) {
                                aws_sdk_bedrockruntime::types::ToolResultStatus::Error
                            } else {
                                aws_sdk_bedrockruntime::types::ToolResultStatus::Success
                            };
                            let result = aws_sdk_bedrockruntime::types::ToolResultBlock::builder()
                                .tool_use_id(tool_use_id)
                                .status(status)
                                .content(
                                    aws_sdk_bedrockruntime::types::ToolResultContentBlock::Text(
                                        text.clone(),
                                    ),
                                )
                                .build()
                                .ok()?;
                            content.push(ContentBlock::ToolResult(result));
                        }
                        JContentBlock::ToolUse { id, name, input } => {
                            let tool_use = aws_sdk_bedrockruntime::types::ToolUseBlock::builder()
                                .tool_use_id(id)
                                .name(name)
                                .input(Self::json_to_document(input))
                                .build()
                                .ok()?;
                            content.push(ContentBlock::ToolUse(tool_use));
                        }
                        _ => {}
                    }
                }
                if content.is_empty() {
                    return None;
                }
                Message::builder()
                    .role(role)
                    .set_content(Some(content))
                    .build()
                    .ok()
            })
            .collect()
    }

    fn tool_config(tools: &[ToolDefinition]) -> Option<ToolConfiguration> {
        if tools.is_empty() {
            return None;
        }
        let bedrock_tools = tools
            .iter()
            .filter_map(|tool| {
                let schema = ToolInputSchema::Json(Self::json_to_document(&tool.input_schema));
                ToolSpecification::builder()
                    .name(&tool.name)
                    .description(tool.description.clone())
                    .input_schema(schema)
                    .build()
                    .ok()
                    .map(Tool::ToolSpec)
            })
            .collect::<Vec<_>>();
        if bedrock_tools.is_empty() {
            None
        } else {
            ToolConfiguration::builder()
                .set_tools(Some(bedrock_tools))
                .build()
                .ok()
        }
    }

    fn known_models() -> Vec<&'static str> {
        vec![
            "anthropic.claude-3-5-sonnet-20241022-v2:0",
            "anthropic.claude-3-5-haiku-20241022-v1:0",
            "anthropic.claude-3-7-sonnet-20250219-v1:0",
            "anthropic.claude-sonnet-4-20250514-v1:0",
            "anthropic.claude-opus-4-20250514-v1:0",
            "amazon.nova-pro-v1:0",
            "amazon.nova-lite-v1:0",
            "amazon.nova-micro-v1:0",
            "meta.llama3-1-405b-instruct-v1:0",
            "mistral.mistral-large-2407-v1:0",
        ]
    }
}

#[async_trait]
impl Provider for BedrockProvider {
    async fn complete(
        &self,
        messages: &[JMessage],
        tools: &[ToolDefinition],
        system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let model = self.model();
        let request_messages = Self::to_bedrock_messages(messages);
        let tool_config = Self::tool_config(tools);
        let system_blocks = if system.trim().is_empty() {
            None
        } else {
            Some(vec![SystemContentBlock::Text(system.to_string())])
        };
        let (tx, rx) = mpsc::channel::<Result<StreamEvent>>(64);
        tokio::spawn(async move {
            let client = Self::client().await;
            let mut req = client
                .converse_stream()
                .model_id(model.clone())
                .set_messages(Some(request_messages));
            if let Some(system_blocks) = system_blocks {
                req = req.set_system(Some(system_blocks));
            }
            if let Some(tool_config) = tool_config {
                req = req.tool_config(tool_config);
            }
            let resp = match req.send().await {
                Ok(resp) => resp,
                Err(err) => {
                    let _ = tx
                        .send(Err(
                            anyhow::anyhow!(err).context("Bedrock ConverseStream request failed")
                        ))
                        .await;
                    return;
                }
            };
            let mut stream = resp.stream;
            let mut current_tool: Option<(String, String, String)> = None;
            loop {
                match stream.recv().await {
                    Ok(Some(event)) => match event {
                        ConverseStreamOutput::ContentBlockStart(start) => {
                            if let Some(ContentBlockStart::ToolUse(tool)) = start.start {
                                let id = tool.tool_use_id().to_string();
                                let name = tool.name().to_string();
                                current_tool = Some((id.clone(), name.clone(), String::new()));
                                let _ = tx.send(Ok(StreamEvent::ToolUseStart { id, name })).await;
                            }
                        }
                        ConverseStreamOutput::ContentBlockDelta(delta) => {
                            if let Some(d) = delta.delta {
                                match d {
                                    ContentBlockDelta::Text(text) => {
                                        let _ = tx.send(Ok(StreamEvent::TextDelta(text))).await;
                                    }
                                    ContentBlockDelta::ToolUse(tool_delta) => {
                                        let input = tool_delta.input();
                                        if !input.is_empty() {
                                            if let Some((_, _, buf)) = current_tool.as_mut() {
                                                buf.push_str(input);
                                            }
                                            let _ = tx
                                                .send(Ok(StreamEvent::ToolInputDelta(
                                                    input.to_string(),
                                                )))
                                                .await;
                                        }
                                    }
                                    ContentBlockDelta::ReasoningContent(reasoning) => {
                                        if let ReasoningContentBlockDelta::Text(text) = reasoning {
                                            let _ =
                                                tx.send(Ok(StreamEvent::ThinkingDelta(text))).await;
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                        ConverseStreamOutput::ContentBlockStop(_) => {
                            if current_tool.take().is_some() {
                                let _ = tx.send(Ok(StreamEvent::ToolUseEnd)).await;
                            }
                        }
                        ConverseStreamOutput::MessageStop(stop) => {
                            let reason = Some(format!("{:?}", stop.stop_reason()));
                            let _ = tx
                                .send(Ok(StreamEvent::MessageEnd {
                                    stop_reason: reason,
                                }))
                                .await;
                        }
                        ConverseStreamOutput::Metadata(meta) => {
                            if let Some(usage) = meta.usage() {
                                let _ = tx
                                    .send(Ok(StreamEvent::TokenUsage {
                                        input_tokens: Some(usage.input_tokens() as u64),
                                        output_tokens: Some(usage.output_tokens() as u64),
                                        cache_read_input_tokens: None,
                                        cache_creation_input_tokens: None,
                                    }))
                                    .await;
                            }
                        }
                        _ => {}
                    },
                    Ok(None) => break,
                    Err(err) => {
                        let _ = tx
                            .send(Err(anyhow::anyhow!(err).context("Bedrock stream failed")))
                            .await;
                        break;
                    }
                }
            }
        });
        Ok(Box::pin(ReceiverStream::new(rx))
            as Pin<
                Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>,
            >)
    }

    fn name(&self) -> &str {
        "bedrock"
    }
    fn model(&self) -> String {
        self.model.read().unwrap_or_else(|p| p.into_inner()).clone()
    }
    fn supports_image_input(&self) -> bool {
        false
    }
    fn set_model(&self, model: &str) -> Result<()> {
        *self.model.write().unwrap_or_else(|p| p.into_inner()) = model.to_string();
        Ok(())
    }
    fn available_models(&self) -> Vec<&'static str> {
        Self::known_models()
    }
    fn available_models_display(&self) -> Vec<String> {
        Self::known_models()
            .into_iter()
            .map(str::to_string)
            .collect()
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self {
            model: Arc::new(RwLock::new(self.model())),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detects_env_credentials() {
        crate::env::set_var("JCODE_BEDROCK_REGION", "us-east-1");
        assert!(BedrockProvider::has_credentials());
        crate::env::remove_var("JCODE_BEDROCK_REGION");
    }

    #[test]
    fn switches_arbitrary_model_ids() {
        let p = BedrockProvider::new();
        p.set_model("us.anthropic.claude-3-5-sonnet-20241022-v2:0")
            .unwrap();
        assert_eq!(p.model(), "us.anthropic.claude-3-5-sonnet-20241022-v2:0");
    }
}
