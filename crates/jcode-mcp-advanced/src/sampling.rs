// ════════════════════════════════════════════════════════════════
// MCP Sampling 处理器 — LLM 回调支持
//
// 当 MCP Server 发送 sampling/createMessage 请求时,
// Client 需要代表 Server 调用本地 LLM 并返回结果。
//
// 这是 Claude Code 的核心能力之一: 让 MCP Server 可以利用
// Client 的 LLM 能力来增强其功能 (如智能搜索、内容生成等)。
// ════════════════════════════════════════════════════════════════

use crate::types::{CreateMessageRequest, CreateMessageResult, ContentBlock};

/// Sampling 回调函数签名
pub type SamplingCallback = Box<dyn Fn(CreateMessageRequest) -> std::pin::Pin<Box<dyn std::future::Future<Output = CreateMessageResult> + Send>> + Send + Sync>;

/// Sampling 处理器 — 管理 MCP Server 的 LLM 采样请求
pub struct SamplingHandler {
    /// 实际执行采样的回调
    callback: Option<SamplingCallback>,
    /// 是否启用
    enabled: bool,
}

impl Default for SamplingHandler {
    fn default() -> Self {
        Self { callback: None, enabled: false }
    }
}

impl SamplingHandler {
    pub fn new() -> Self {
        Default::default()
    }

    /// 设置采样回调
    pub fn set_callback<F>(&mut self, callback: F)
    where F: Fn(CreateMessageRequest) -> std::pin::Pin<Box<dyn std::future::Future<Output = CreateMessageResult> + Send>> + Send + Sync + 'static {
        self.callback = Some(Box::new(callback));
        self.enabled = true;
    }

    /// 启用/禁用
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// 是否可用
    pub fn is_available(&self) -> bool {
        self.enabled && self.callback.is_some()
    }

    /// 执行采样请求
    pub async fn sample(&self, _request: CreateMessageRequest) -> Result<CreateResult, String> {
        if !self.is_available() {
            return Err("Sampling handler not configured".into());
        }

        let _cb = self.callback.as_ref().unwrap();
        
        // TODO: Fix the async trait issue with the callback signature.
        // For now return a placeholder result.
        
        Ok(CreateResult {
            role: "assistant".to_string(),
            content: ContentBlock {
                content_type: "text".into(),
                text: Some("Sampling response placeholder".into()),
                data: None,
            },
            model: None,
            stop_reason: None,
        })
    }

    // --- 内置默认采样实现 -------------------------

    /// 创建一个简单的回显采样处理器 (用于测试)
    pub fn echo_handler() -> Self {
        // Placeholder - real implementation would use actual LLM call
        Self::new()
    }
}

/// 采样结果 (内部使用)
struct CreateResult {
    role: String,
    content: ContentBlock,
    model: Option<String>,
    stop_reason: Option<String>,
}
