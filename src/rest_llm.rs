//! REST API 真实 LLM 集成 — 替换 Mock complete/generate handler
//!
//! 将 src/rest/server.rs 的 Mock 端点接入真实 LLM Provider:
//!   1. POST /api/v1/complete → AutoFallbackRouter → CpuEngine/DeepseekProvider
//!   2. POST /api/v1/generate → 同上 + 代码模板
//!   3. POST /v1/completions (FIM) → Fill-in-the-Middle 补全

use std::sync::Arc;

use crate::auto_fallback::{AutoFallbackRouter, InferenceTarget};

/// 补全请求
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AiCompleteRequest {
    pub code: String,
    pub language: String,
    pub cursor_line: usize,
    pub cursor_character: usize,
}

/// 补全响应
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AiCompleteResponse {
    pub items: Vec<AiCompletionItem>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AiCompletionItem {
    pub label: String,
    pub text: String,
    pub kind: String,
}

/// 生成请求
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AiGenerateRequest {
    pub prompt: String,
    pub language: String,
    pub framework: Option<String>,
    pub include_tests: bool,
}

/// 生成响应
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AiGenerateResponse {
    pub code: String,
    pub explanation: String,
    pub language: String,
}

/// FIM (Fill-in-Middle) 请求 — 用于内联补全
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FimRequest {
    pub model: String,
    pub prompt: String,    // 光标前内容
    pub suffix: String,    // 光标后内容
    pub max_tokens: Option<u32>,
    pub temperature: Option<f64>,
}

/// FIM 响应
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FimResponse {
    pub id: String,
    pub choices: Vec<FimChoice>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FimChoice {
    pub text: String,
    pub index: u32,
    pub finish_reason: Option<String>,
}

/// 推理路由器 — 封装了 local→cloud fallback
pub struct InferenceRouter {
    fallback: Arc<AutoFallbackRouter>,
}

impl InferenceRouter {
    pub fn new(local_models: Vec<String>, cloud_model: &str) -> Self {
        Self {
            fallback: Arc::new(AutoFallbackRouter::new(local_models, cloud_model)),
        }
    }

    /// 执行自动路由推理
    pub async fn chat_completion(&self, prompt: &str, system: &str) -> Result<String, String> {
        let target = self.fallback.resolve_target().await;
        let result = match &target {
            InferenceTarget::Local { model } => {
                self.call_local_model(model, prompt, system).await
            }
            InferenceTarget::Cloud { provider, model } => {
                self.call_cloud_api(provider, model, prompt, system).await
            }
        };

        match &result {
            Ok(_) => self.fallback.report_success().await,
            Err(e) => self.fallback.report_failure(e).await,
        }

        result
    }

    /// 调用本地 llama.cpp 模型
    async fn call_local_model(&self, model: &str, prompt: &str, system: &str) -> Result<String, String> {
        // 查找本地 llama 进程的端口
        for port in 18000..18100u16 {
            let url = format!("http://127.0.0.1:{}/v1/chat/completions", port);
            let body = serde_json::json!({
                "model": model,
                "messages": [
                    {"role": "system", "content": system},
                    {"role": "user", "content": prompt}
                ],
                "max_tokens": 2048,
                "temperature": 0.7,
            });

            match reqwest::Client::new()
                .post(&url)
                .json(&body)
                .timeout(std::time::Duration::from_secs(30))
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    let data: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
                    let text = data["choices"][0]["message"]["content"]
                        .as_str().unwrap_or("")
                        .to_string();
                    return Ok(text);
                }
                Ok(_) => continue,
                Err(_) => continue,
            }
        }
        Err("No local model available on ports 18000-18100".to_string())
    }

    /// 调用 Deepseek 云端 API
    async fn call_cloud_api(&self, _provider: &str, model: &str, prompt: &str, system: &str) -> Result<String, String> {
        let api_key = std::env::var("DEEPSEEK_API_KEY")
            .map_err(|_| "DEEPSEEK_API_KEY not set".to_string())?;

        let body = serde_json::json!({
            "model": model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": prompt}
            ],
            "max_tokens": 4096,
            "temperature": 0.7,
        });

        let client = reqwest::Client::new();
        let resp = client
            .post("https://api.deepseek.com/v1/chat/completions")
            .header("Authorization", format!("Bearer {}", api_key))
            .json(&body)
            .timeout(std::time::Duration::from_secs(60))
            .send()
            .await
            .map_err(|e| format!("Deepseek API error: {}", e))?;

        if !resp.status().is_success() {
            let status = resp.status();
            return Err(format!("Deepseek API returned {}", status));
        }

        let data: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
        let text = data["choices"][0]["message"]["content"]
            .as_str().unwrap_or("")
            .to_string();

        Ok(text)
    }

    /// 生成代码补全 (针对光标位置)
    pub async fn complete(&self, req: &AiCompleteRequest) -> AiCompleteResponse {
        let prompt = format!(
            "Complete the following {} code at cursor position (line {}, column {}):\n\n\
             Code:\n```{}\n{}\n```\n\n\
             Return ONLY the completion text (single line). No explanation.",
            req.language, req.cursor_line, req.cursor_character,
            req.language, req.code
        );

        match self.chat_completion(&prompt, "You are a code completion engine. Return ONLY the code, no explanation.").await {
            Ok(text) => AiCompleteResponse {
                items: vec![AiCompletionItem {
                    label: text.chars().take(50).collect(),
                    text,
                    kind: "snippet".to_string(),
                }],
            },
            Err(_) => AiCompleteResponse { items: vec![] },
        }
    }

    /// 生成代码 (完整文件)
    pub async fn generate(&self, req: &AiGenerateRequest) -> AiGenerateResponse {
        let prompt = format!(
            "Generate {} code.{} Additional requirements: {}\n\n\
             Return ONLY the code, wrapped in ```{} ... ```.\
             {}",
            req.language,
            req.framework.as_ref().map(|f| format!(" Framework: {}.", f)).unwrap_or_default(),
            req.prompt,
            req.language,
            if req.include_tests { " Include tests." } else { "" },
        );

        match self.chat_completion(&prompt, "You are a code generation engine.").await {
            Ok(text) => {
                let code = extract_code_block(&text, &req.language);
                AiGenerateResponse {
                    code,
                    explanation: "Generated by CarpAI inference router".to_string(),
                    language: req.language.clone(),
                }
            }
            Err(e) => AiGenerateResponse {
                code: format!("// Error: {}", e),
                explanation: e,
                language: req.language.clone(),
            },
        }
    }

    /// FIM 补全 (Fill-in-the-Middle — 用于内联补全)
    pub async fn fill_in_middle(&self, req: &FimRequest) -> FimResponse {
        let fim_prompt = format!(
            "<|fim_prefix|>{}<|fim_suffix|>{}<|fim_middle|>",
            req.prompt, req.suffix
        );

        match self.chat_completion(&fim_prompt, "You are a code infill engine. Complete the code between prefix and suffix.").await {
            Ok(text) => FimResponse {
                id: format!("fim-{}", std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos()),
                choices: vec![FimChoice {
                    text,
                    index: 0,
                    finish_reason: Some("stop".to_string()),
                }],
            },
            Err(e) => FimResponse {
                id: "error".to_string(),
                choices: vec![FimChoice {
                    text: format!("// Error: {}", e),
                    index: 0,
                    finish_reason: Some("error".to_string()),
                }],
            },
        }
    }

    /// 获取 Fallback 状态
    pub async fn fallback_status(&self) -> String {
        self.fallback.status_summary().await
    }
}

/// 从 LLM 响应中提取代码块
fn extract_code_block(text: &str, language: &str) -> String {
    // 尝试匹配 ```language ... ```
    let pattern = format!("```{}", language);
    if let Some(start) = text.find(&pattern) {
        let after_start = &text[start + pattern.len()..];
        if let Some(end) = after_start.find("```") {
            return after_start[..end].trim().to_string();
        }
    }
    // 回退: 匹配任何 ``` ... ```
    if let Some(start) = text.find("```") {
        let after_start = &text[start + 3..];
        // 跳过语言标识
        let after_lang = if let Some(nl) = after_start.find('\n') {
            &after_start[nl + 1..]
        } else { after_start };
        if let Some(end) = after_lang.find("```") {
            return after_lang[..end].trim().to_string();
        }
    }
    text.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_code_block() {
        let text = "Here is the code:\n```rust\nfn main() {\n    println!(\"hello\");\n}\n```\nEnd.";
        let code = extract_code_block(text, "rust");
        assert!(code.contains("fn main()"));
        assert!(code.contains("println"));
    }

    #[tokio::test]
    async fn test_fim_fallback_on_no_local() {
        let router = InferenceRouter::new(vec![], "deepseek-chat");
        let req = FimRequest {
            model: "deepseek-chat".to_string(),
            prompt: "fn hello()".to_string(),
            suffix: "}".to_string(),
            max_tokens: Some(50),
            temperature: Some(0.5),
        };
        // 没有本地模型也没有 API_KEY → 返回 "Error"
        let resp = router.fill_in_middle(&req).await;
        assert!(resp.choices[0].text.contains("Error") || resp.choices[0].text.contains("DEEPSEEK_API_KEY"));
    }
}
