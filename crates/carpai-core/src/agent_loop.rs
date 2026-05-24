use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use anyhow::Result;
use carpai_internal::*;
use crate::{
    LocalFileSessionStore, LocalToolExecutor, SidecarInferenceBackend,
    LocalFileSystem, InProcessEventBus, LocalMemoryBackend,
};
use tracing::{info, warn};

/// Information about a tool call made during agent execution
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolCallInfo {
    pub name: String,
    pub arguments: serde_json::Value,
    pub result: Option<String>,
    pub duration_ms: u64,
    pub status: String,
}

/// Execute one complete agent turn (pure business logic)
///
/// # Flow
/// 1. Get or create session via SessionStore
/// 2. Append user message to session
/// 3. Build context from session history
/// 4. Call InferenceBackend.generate() for response
/// 5. If tool_calls present, execute via ToolExecutor
/// 6. Collect tool results and send back to inference (loop)
/// 7. Return final AgentTurnOutput
///
/// # Arguments
/// * `ctx` - AgentContext containing all trait objects
/// * `user_message` - Raw user input string
///
/// # Returns
/// * `AgentTurnOutput` with text, tool_calls, usage, etc.
pub async fn execute_agent_turn(
    ctx: &AgentContext,
    user_message: &str,
) -> Result<AgentTurnOutput> {
    let start = Instant::now();

    info!(working_dir = ?ctx.config.working_dir, "Starting agent turn");

    // Step 1: Ensure session exists
    let sessions = &ctx.sessions;
    let session_id = SessionId("default-session".to_string());

    match sessions.load_session(&session_id).await {
        Ok(Some(existing)) => existing.meta.id,
        Ok(None) => {
            let meta = SessionMeta {
                id: session_id.clone(),
                title: Some("Agent Session".into()),
                owner_id: None,
                state: SessionState::Active,
                model: Some(ctx.config.default_model.clone()),
                working_dir: Some(ctx.config.working_dir.to_string_lossy().to_string()),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
                last_active_at: Some(chrono::Utc::now()),
                tags: HashMap::new(),
                message_count: 0,
                parent_id: None,
            };

            sessions.create_session(meta).await.map_err(|e| {
                anyhow::anyhow!("Failed to create session: {}", e)
            })?
        }
        Err(e) => return Err(anyhow::anyhow!("Session error: {}", e)),
    };

    // Step 2: Append user message
    let user_msg = StoredMessage {
        id: uuid::Uuid::new_v4().to_string(),
        role: MessageRole::User,
        content: vec![ContentBlock::Text { text: user_message.to_string() }],
        timestamp: chrono::Utc::now(),
        token_usage: None,
        model: None,
    };
    sessions.append_messages(&session_id, vec![user_msg]).await
        .map_err(|e| anyhow::anyhow!("Failed to append message: {}", e))?;

    // Step 3: Load session history for context
    let history = sessions.load_session(&session_id).await
        .map_err(|e| anyhow::anyhow!("Failed to load session: {}", e))?
        .ok_or_else(|| anyhow::anyhow!("Session not found after creation"))?;

    // Convert StoredMessages to ChatMessages for inference
    let chat_messages: Vec<ChatMessage> = history.messages.iter().map(|msg| ChatMessage {
        role: match msg.role {
            MessageRole::User => ChatRole::User,
            MessageRole::Assistant => ChatRole::Assistant,
            MessageRole::System => ChatRole::System,
            MessageRole::Tool => ChatRole::Tool,
        },
        content: if msg.content.is_empty() {
            ChatContent::Text(String::new())
        } else {
            // Use first content block for simplicity
            match &msg.content[0] {
                ContentBlock::Text { text } => ChatContent::Text(text.clone()),
                ContentBlock::ToolUse { name, input, .. } => {
                    ChatContent::Text(format!("[Tool Call] {}({})", name, input))
                }
                ContentBlock::ToolResult { content, .. } => ChatContent::Text(content.clone()),
                ContentBlock::Thinking { text, .. } => ChatContent::Text(text.clone()),
            }
        },
        name: None,
    }).collect();

    // Step 4: Generate response
    let inference = &ctx.inference;
    let request = ChatCompletionRequest {
        messages: chat_messages,
        model: ctx.config.default_model.clone(),
        max_tokens: Some(4096),
        temperature: Some(0.7),
        top_p: None,
        stop: None,
        presence_penalty: None,
        frequency_penalty: None,
        tools: None,
        tool_choice: None,
        user_id: None,
        session_id: Some(session_id.to_string()),
        metadata: HashMap::new(),
    };

    let response = inference.complete_chat(request).await.map_err(|e| {
        anyhow::anyhow!("Inference failed: {}", e)
    })?;

    // Extract tool calls from the response choices
    let tool_calls_info = vec![];
    let mut response_text = String::new();

    if let Some(choice) = response.choices.first() {
        response_text = match &choice.message.content {
            ChatContent::Text(t) => t.clone(),
            ChatContent::Parts(parts) => {
                parts.iter()
                    .filter_map(|p| p.text.as_ref())
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        };

        // Check for tool calls in content blocks
        // Note: Current implementation doesn't have direct tool_calls field
        // Tool calls would be represented as ToolUse content blocks
    }

    // Step 5-6: Handle tool calls (if any)

    // For now, skip tool execution as the current architecture doesn't have
    // direct tool_calls in the response. This needs to be implemented
    // when the inference backend supports function calling properly.

    // Step 7: Append assistant response to session
    if !response_text.is_empty() {
        let assistant_msg = StoredMessage {
            id: uuid::Uuid::new_v4().to_string(),
            role: MessageRole::Assistant,
            content: vec![ContentBlock::Text { text: response_text.clone() }],
            timestamp: chrono::Utc::now(),
            token_usage: None,
            model: Some(response.model.clone()),
        };
        if let Err(e) = sessions.append_messages(&session_id, vec![assistant_msg]).await {
            warn!(error = %e, "Failed to append assistant message");
        }
    }

    let duration_ms = start.elapsed().as_millis() as u64;

    info!(
        session_id = %session_id,
        duration_ms,
        tokens = ?response.usage,
        tool_calls = tool_calls_info.len(),
        "Agent turn completed"
    );

    Ok(AgentTurnOutput {
        text: response_text,
        tool_calls: tool_calls_info,
        usage: TokenUsage {
            prompt_tokens: response.usage.prompt_tokens,
            completion_tokens: response.usage.completion_tokens,
            total_tokens: response.usage.total_tokens,
        },
        session_id,
        duration_ms,
    })
}

/// Output of a single agent interaction
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentTurnOutput {
    pub text: String,
    pub tool_calls: Vec<ToolCallInfo>,
    pub usage: TokenUsage,
    pub session_id: SessionId,
    pub duration_ms: u64,
}

/// Build a complete AgentContext with all Local* implementations
///
/// This is the primary entry point for CLI/local development mode.
/// All trait objects are wired to their local filesystem-backed implementations.
///
/// # Example
/// ```ignore
/// use carpai_core::{CoreConfig, build_local_agent_context};
///
/// let config = CoreConfig::load(&PathBuf::from("~/.carpai/config.toml"))?;
/// let ctx = build_local_agent_context(&config);
///
/// let output = execute_agent_turn(&ctx, "Hello, CarpAI!").await?;
/// println!("{}", output.text);
/// ```
pub fn build_local_agent_context(config: &crate::config::CoreConfig) -> AgentContext {
    let app_config = AppConfig {
        mode: AppMode::Cli,
        data_dir: config.data_dir.clone(),
        working_dir: config.base.working_dir.clone(),
        default_model: config.base.default_model.clone(),
        max_context_tokens: config.base.max_context_tokens,
        tools_enabled: true,
        default_tool_mode: ExecutionMode::Local,
        vfs_enabled: config.base.vfs_enabled,
        vfs_root: config.base.vfs_root.clone(),
        memory_enabled: config.base.memory_enabled,
        event_bus_enabled: config.base.event_bus_enabled,
    };

    AgentContextBuilder::new(app_config)
        .with_sessions(Arc::new(LocalFileSessionStore::new(
            config.session_store_path(),
        )))
        .with_tools(Arc::new(LocalToolExecutor::new(
            config.max_concurrent_tools,
        )))
        .with_inference(Arc::new(SidecarInferenceBackend::new(
            &config.completion_provider,
        )))
        .with_fs(Arc::new(LocalFileSystem::new(
            &config.base.working_dir,
            config.base.vfs_root.as_deref(),
        )))
        .with_events(Arc::new(InProcessEventBus::new(1024)))
        .with_memory(Arc::new(LocalMemoryBackend::new(
            config.memory_store_path(),
        )))
        .build()
        .expect("AgentContext assembly: all components must be valid")
}
