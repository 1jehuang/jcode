use anyhow::{anyhow, Result};
use reqwest::{Client, Response};
use serde::{Deserialize, Serialize};
use std::env;

#[derive(Debug, Clone)]
pub struct DeepSeekClient {
    client: Client,
    api_key: String,
    base_url: String,
}

#[derive(Debug, Serialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub temperature: f32,
    pub max_tokens: Option<u32>,
    pub stream: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<Choice>,
    pub usage: Usage,
}

#[derive(Debug, Deserialize)]
pub struct Choice {
    pub index: u32,
    pub message: Message,
    pub finish_reason: String,
}

#[derive(Debug, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

impl Default for DeepSeekClient {
    fn default() -> Self {
        Self::new()
    }
}

impl DeepSeekClient {
    pub fn new() -> Self {
        let api_key = env::var("DEEPSEEK_API_KEY").unwrap_or_else(|_| "".to_string());
        let base_url = env::var("DEEPSEEK_BASE_URL")
            .unwrap_or_else(|_| "https://api.deepseek.com/v1".to_string());

        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .unwrap();

        Self {
            client,
            api_key,
            base_url,
        }
    }

    pub fn with_api_key(api_key: &str) -> Self {
        let mut client = Self::new();
        client.api_key = api_key.to_string();
        client
    }

    pub async fn chat_completion(&self, request: ChatCompletionRequest) -> Result<ChatCompletionResponse> {
        if self.api_key.is_empty() {
            return Err(anyhow!("DEEPSEEK_API_KEY not set"));
        }

        let url = format!("{}/chat/completions", self.base_url);

        let response = self.client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await?;

        self.handle_response(response).await
    }

    pub async fn stream_chat_completion(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<reqwest::Response> {
        if self.api_key.is_empty() {
            return Err(anyhow!("DEEPSEEK_API_KEY not set"));
        }

        let url = format!("{}/chat/completions", self.base_url);

        let response = self.client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let error_body = response.text().await?;
            return Err(anyhow!("DeepSeek API error: {}", error_body));
        }

        Ok(response)
    }

    pub async fn list_models(&self) -> Result<Vec<String>> {
        if self.api_key.is_empty() {
            return Err(anyhow!("DEEPSEEK_API_KEY not set"));
        }

        let url = format!("{}/models", self.base_url);

        let response = self.client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await?;

        if !response.status().is_success() {
            let error_body = response.text().await?;
            return Err(anyhow!("DeepSeek API error: {}", error_body));
        }

        #[derive(Debug, Deserialize)]
        struct ModelListResponse {
            data: Vec<ModelInfo>,
        }

        #[derive(Debug, Deserialize)]
        struct ModelInfo {
            id: String,
        }

        let model_list: ModelListResponse = response.json().await?;
        Ok(model_list.data.into_iter().map(|m| m.id).collect())
    }

    async fn handle_response(&self, response: Response) -> Result<ChatCompletionResponse> {
        if !response.status().is_success() {
            let error_body = response.text().await?;
            return Err(anyhow!("DeepSeek API error: {}", error_body));
        }

        response.json().await.map_err(|e| anyhow!("Failed to parse response: {}", e))
    }
}

#[derive(Debug, Deserialize)]
pub struct StreamChunk {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<StreamChoice>,
}

#[derive(Debug, Deserialize)]
pub struct StreamChoice {
    pub index: u32,
    pub delta: Delta,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Delta {
    pub role: Option<String>,
    pub content: Option<String>,
}