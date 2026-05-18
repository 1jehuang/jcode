//! AI 助手交互处理器
//!
//! 提供 AI 集成功能：
//! - 对话式聊天（非流式 / 流式）
//! - 代码补全建议
//! - 代码解释
//!
//! 使用 Sidecar（轻量级 AI 客户端）调用底层 LLM 提供服务。

use crate::sidecar::Sidecar;
use crate::ws::protocol::{WsRequest, WsResponse, MessageType};
use crate::ws::session::SessionManager;
use anyhow::Result;
use tracing::{info, warn};

const CHAT_SYSTEM_PROMPT: &str = "You are a helpful AI coding assistant integrated into a Web IDE. \
    You help users with code writing, debugging, explanation, and general programming questions. \
    Be concise and practical. When showing code, include the code block with language annotation.";

const COMPLETION_SYSTEM_PROMPT: &str = "You are a code completion engine. \
    Given the code context and cursor position, provide relevant completion suggestions. \
    Return a JSON array of completions, each with 'text' (the completion code), \
    'confidence' (0.0-1.0), and 'description' (brief explanation). \
    Output ONLY the JSON array, no other text.";

const EXPLAIN_SYSTEM_PROMPT: &str = "You are a code explanation engine. \
    Given a code snippet and language, provide a clear technical explanation. \
    Include: purpose, key patterns, complexity analysis, and improvement suggestions.";

/// 处理 AI 聊天请求
pub async fn handle_chat(
    request: &WsRequest,
    session_id: &str,
    _session_manager: &SessionManager,
) -> Result<WsResponse> {
    let prompt = request.params.get("prompt")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'prompt' parameter"))?;

    let context = request.params.get("context")
        .and_then(|v| v.as_str());

    let stream: bool = request.params.get("stream")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    info!(
        session_id = %session_id,
        prompt_len = prompt.len(),
        has_context = context.is_some(),
        stream = stream,
        "AI chat requested"
    );

    // Build system prompt with optional context
    let system_prompt = match context {
        Some(ctx) => format!(
            "{}\n\n## Current Context\n{}",
            CHAT_SYSTEM_PROMPT, ctx
        ),
        None => CHAT_SYSTEM_PROMPT.to_string(),
    };

    let sidecar = Sidecar::new();

    if stream {
        // Stream mode: return streaming marker (actual streaming requires a persistent WS event loop)
        Ok(WsResponse::new(&request.id, MessageType::Response, serde_json::json!({
            "streaming": true,
            "stream_id": format!("stream_{}", uuid::Uuid::new_v4()),
            "message": "Stream started. Listen for stream_chunk messages."
        })))
    } else {
        // Non-streaming: call Sidecar for real response
        match sidecar.complete(&system_prompt, prompt).await {
            Ok(response) => {
                let prompt_tokens = estimate_tokens(prompt);
                let completion_tokens = estimate_tokens(&response);

                info!(
                    session_id = %session_id,
                    response_len = response.len(),
                    prompt_tokens = prompt_tokens,
                    completion_tokens = completion_tokens,
                    "AI chat response generated"
                );

                Ok(WsResponse::new(&request.id, MessageType::Response, serde_json::json!({
                    "response": response,
                    "model": sidecar.model_name(),
                    "tokens_used": {
                        "prompt_tokens": prompt_tokens,
                        "completion_tokens": completion_tokens,
                        "total_tokens": prompt_tokens + completion_tokens,
                    }
                })))
            }
            Err(e) => {
                warn!(
                    session_id = %session_id,
                    error = %e,
                    "AI chat failed, falling back to mock"
                );
                // Fallback: only on complete failure, provide a graceful message
                Ok(WsResponse::new(&request.id, MessageType::Response, serde_json::json!({
                    "response": format!(
                        "I'm sorry, I encountered an error connecting to the AI service: {}\n\n\
                         Please check that your API credentials are configured correctly \
                         (OPENAI_API_KEY for OpenAI, or Claude authentication).",
                        e
                    ),
                    "model": "fallback",
                    "tokens_used": {
                        "prompt_tokens": 0,
                        "completion_tokens": 0,
                        "total_tokens": 0,
                    }
                })))
            }
        }
    }
}

/// 处理 AI 代码补全请求
pub async fn handle_complete(
    request: &WsRequest,
    session_id: &str,
    _session_manager: &SessionManager,
) -> Result<WsResponse> {
    let code = request.params.get("code")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'code' parameter"))?;

    let language = request.params.get("language")
        .and_then(|v| v.as_str())
        .unwrap_or("rust");

    let cursor_line: usize = request.params.get("cursor_line")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize;

    info!(
        session_id = %session_id,
        language = %language,
        code_len = code.lines().count(),
        "AI completion requested"
    );

    let sidecar = Sidecar::new();

    // Build a structured prompt for completion
    let user_prompt = format!(
        "Language: {}\nCursor at line: {}\n\nCode before cursor:\n```{}\n{}\n```\n\n\
         Suggest 2-3 relevant completions for what the developer is likely typing next. \
         Return as JSON array of {{'text': string, 'confidence': float, 'description': string}}.",
        language, cursor_line, language, code
    );

    match sidecar.complete(COMPLETION_SYSTEM_PROMPT, &user_prompt).await {
        Ok(response) => {
            // Try to parse the response as JSON; fall back to wrapping it
            let completions: Vec<serde_json::Value> = serde_json::from_str(&response)
                .unwrap_or_else(|_| {
                    vec![serde_json::json!({
                        "text": response,
                        "confidence": 0.8,
                        "description": format!("AI-generated {} completion", language)
                    })]
                });

            Ok(WsResponse::new(&request.id, MessageType::Response, serde_json::json!({
                "completions": completions,
                "language": language,
                "model": sidecar.model_name(),
            })))
        }
        Err(e) => {
            warn!(error = %e, "AI completion failed, using fallback");
            Ok(WsResponse::new(&request.id, MessageType::Response, serde_json::json!({
                "completions": [{
                    "text": format!("\n// Completion unavailable: {}", e),
                    "confidence": 0.0,
                    "description": "Service unavailable"
                }],
                "language": language
            })))
        }
    }
}

/// 处理代码解释请求
pub async fn handle_explain(
    request: &WsRequest,
    session_id: &str,
    _session_manager: &SessionManager,
) -> Result<WsResponse> {
    let code = request.params.get("code")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'code' parameter"))?;

    let language = request.params.get("language")
        .and_then(|v| v.as_str())
        .unwrap_or("auto");

    let detail_level: String = request.params.get("detail_level")
        .and_then(|v| v.as_str())
        .unwrap_or("medium")
        .to_string();

    info!(
        session_id = %session_id,
        language = %language,
        detail_level = %detail_level,
        code_len = code.lines().count(),
        "Code explanation requested"
    );

    let sidecar = Sidecar::new();

    let detail_instruction = match detail_level.as_str() {
        "brief" => "Provide a brief summary (2-3 sentences).",
        "medium" => "Provide a moderate-detail analysis covering structure, patterns, and key observations.",
        _ => "Provide a detailed analysis including: overview, structure breakdown, key patterns, \
               best practices observed, potential improvements, and complexity metrics.",
    };

    let user_prompt = format!(
        "Language: {}\nDetail level: {}\n\nCode to explain:\n```{}\n{}\n```\n\n{}",
        language, detail_level, language, code, detail_instruction
    );

    match sidecar.complete(EXPLAIN_SYSTEM_PROMPT, &user_prompt).await {
        Ok(explanation) => {
            Ok(WsResponse::new(&request.id, MessageType::Response, serde_json::json!({
                "explanation": explanation,
                "language": language,
                "detail_level": detail_level,
                "lines_analyzed": code.lines().count(),
                "model": sidecar.model_name(),
            })))
        }
        Err(e) => {
            warn!(error = %e, "AI explanation failed, using fallback");
            Ok(WsResponse::new(&request.id, MessageType::Response, serde_json::json!({
                "explanation": format!(
                    "## Code Analysis ({}.{} lines {})\n\n{}\n\nThe explanation service is currently unavailable. \
                     Please check your API credentials configuration.",
                    language, code.lines().count(),
                    if detail_level == "brief" { "brief" } else { "detailed" },
                    code
                ),
                "language": language,
                "detail_level": detail_level,
                "lines_analyzed": code.lines().count()
            })))
        }
    }
}

/// Rough token estimation (4 chars ≈ 1 token for English text)
fn estimate_tokens(text: &str) -> u32 {
    (text.len() / 4).max(1) as u32
}
