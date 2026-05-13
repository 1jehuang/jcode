use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ModelProvider {
    OpenAI,
    Anthropic,
    Google,
    DeepSeek,
    Qwen,
    Llama,
    Zhipu,
    Doubao,
    Xunfei,
    Moonshot,
    GLM,
    Other(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub provider: ModelProvider,
    pub base_url: Option<String>,
    pub api_key_env: String,
    pub context_window: usize,
    pub max_tokens: usize,
    pub supports_streaming: bool,
    pub supports_vision: bool,
    pub supports_tools: bool,
    pub category: ModelCategory,
    pub pricing_input: f64,
    pub pricing_output: f64,
    pub recommended: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ModelCategory {
    Chat,
    Code,
    Embedding,
    Vision,
    Multimodal,
}

#[derive(Debug, Clone)]
pub struct ModelsRegistry {
    models: HashMap<String, ModelInfo>,
    provider_models: HashMap<ModelProvider, Vec<String>>,
    category_models: HashMap<ModelCategory, Vec<String>>,
}

impl Default for ModelsRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelsRegistry {
    pub fn new() -> Self {
        let mut registry = Self {
            models: HashMap::new(),
            provider_models: HashMap::new(),
            category_models: HashMap::new(),
        };
        registry.load_default_models();
        registry
    }

    fn load_default_models(&mut self) {
        self.add_model(ModelInfo {
            id: "gpt-4o".to_string(),
            name: "GPT-4o".to_string(),
            provider: ModelProvider::OpenAI,
            base_url: None,
            api_key_env: "OPENAI_API_KEY".to_string(),
            context_window: 128000,
            max_tokens: 4096,
            supports_streaming: true,
            supports_vision: true,
            supports_tools: true,
            category: ModelCategory::Multimodal,
            pricing_input: 0.005,
            pricing_output: 0.015,
            recommended: true,
        });

        self.add_model(ModelInfo {
            id: "gpt-4o-mini".to_string(),
            name: "GPT-4o mini".to_string(),
            provider: ModelProvider::OpenAI,
            base_url: None,
            api_key_env: "OPENAI_API_KEY".to_string(),
            context_window: 128000,
            max_tokens: 16384,
            supports_streaming: true,
            supports_vision: true,
            supports_tools: true,
            category: ModelCategory::Multimodal,
            pricing_input: 0.00015,
            pricing_output: 0.0006,
            recommended: true,
        });

        self.add_model(ModelInfo {
            id: "claude-3-5-sonnet".to_string(),
            name: "Claude 3.5 Sonnet".to_string(),
            provider: ModelProvider::Anthropic,
            base_url: None,
            api_key_env: "ANTHROPIC_API_KEY".to_string(),
            context_window: 200000,
            max_tokens: 4096,
            supports_streaming: true,
            supports_vision: true,
            supports_tools: true,
            category: ModelCategory::Multimodal,
            pricing_input: 0.003,
            pricing_output: 0.015,
            recommended: true,
        });

        self.add_model(ModelInfo {
            id: "claude-3-opus".to_string(),
            name: "Claude 3 Opus".to_string(),
            provider: ModelProvider::Anthropic,
            base_url: None,
            api_key_env: "ANTHROPIC_API_KEY".to_string(),
            context_window: 200000,
            max_tokens: 4096,
            supports_streaming: true,
            supports_vision: true,
            supports_tools: true,
            category: ModelCategory::Multimodal,
            pricing_input: 0.015,
            pricing_output: 0.075,
            recommended: false,
        });

        self.add_model(ModelInfo {
            id: "gemini-1.5-pro".to_string(),
            name: "Gemini 1.5 Pro".to_string(),
            provider: ModelProvider::Google,
            base_url: None,
            api_key_env: "GOOGLE_API_KEY".to_string(),
            context_window: 1048576,
            max_tokens: 8192,
            supports_streaming: true,
            supports_vision: true,
            supports_tools: true,
            category: ModelCategory::Multimodal,
            pricing_input: 0.00125,
            pricing_output: 0.00375,
            recommended: true,
        });

        self.add_model(ModelInfo {
            id: "deepseek-chat".to_string(),
            name: "DeepSeek Chat".to_string(),
            provider: ModelProvider::DeepSeek,
            base_url: Some("https://api.deepseek.com/v1".to_string()),
            api_key_env: "DEEPSEEK_API_KEY".to_string(),
            context_window: 128000,
            max_tokens: 4096,
            supports_streaming: true,
            supports_vision: false,
            supports_tools: true,
            category: ModelCategory::Chat,
            pricing_input: 0.00015,
            pricing_output: 0.0006,
            recommended: true,
        });

        self.add_model(ModelInfo {
            id: "deepseek-code".to_string(),
            name: "DeepSeek Code".to_string(),
            provider: ModelProvider::DeepSeek,
            base_url: Some("https://api.deepseek.com/v1".to_string()),
            api_key_env: "DEEPSEEK_API_KEY".to_string(),
            context_window: 128000,
            max_tokens: 8192,
            supports_streaming: true,
            supports_vision: false,
            supports_tools: true,
            category: ModelCategory::Code,
            pricing_input: 0.00015,
            pricing_output: 0.0006,
            recommended: true,
        });

        self.add_model(ModelInfo {
            id: "qwen-max".to_string(),
            name: "Qwen Max".to_string(),
            provider: ModelProvider::Qwen,
            base_url: Some("https://api.tongyi.aliyun.com/v1".to_string()),
            api_key_env: "QWEN_API_KEY".to_string(),
            context_window: 128000,
            max_tokens: 8192,
            supports_streaming: true,
            supports_vision: true,
            supports_tools: true,
            category: ModelCategory::Multimodal,
            pricing_input: 0.0008,
            pricing_output: 0.0024,
            recommended: true,
        });

        self.add_model(ModelInfo {
            id: "qwen-plus".to_string(),
            name: "Qwen Plus".to_string(),
            provider: ModelProvider::Qwen,
            base_url: Some("https://api.tongyi.aliyun.com/v1".to_string()),
            api_key_env: "QWEN_API_KEY".to_string(),
            context_window: 32000,
            max_tokens: 4096,
            supports_streaming: true,
            supports_vision: true,
            supports_tools: true,
            category: ModelCategory::Multimodal,
            pricing_input: 0.0005,
            pricing_output: 0.0015,
            recommended: true,
        });

        self.add_model(ModelInfo {
            id: "qwen-code".to_string(),
            name: "Qwen Code".to_string(),
            provider: ModelProvider::Qwen,
            base_url: Some("https://api.tongyi.aliyun.com/v1".to_string()),
            api_key_env: "QWEN_API_KEY".to_string(),
            context_window: 64000,
            max_tokens: 8192,
            supports_streaming: true,
            supports_vision: false,
            supports_tools: true,
            category: ModelCategory::Code,
            pricing_input: 0.0003,
            pricing_output: 0.0009,
            recommended: true,
        });

        self.add_model(ModelInfo {
            id: "glm-4".to_string(),
            name: "GLM-4".to_string(),
            provider: ModelProvider::GLM,
            base_url: Some("https://open.bigmodel.cn/api/paas/v4/".to_string()),
            api_key_env: "GLM_API_KEY".to_string(),
            context_window: 128000,
            max_tokens: 8192,
            supports_streaming: true,
            supports_vision: true,
            supports_tools: true,
            category: ModelCategory::Multimodal,
            pricing_input: 0.0012,
            pricing_output: 0.0036,
            recommended: true,
        });

        self.add_model(ModelInfo {
            id: "glm-4-flash".to_string(),
            name: "GLM-4 Flash".to_string(),
            provider: ModelProvider::GLM,
            base_url: Some("https://open.bigmodel.cn/api/paas/v4/".to_string()),
            api_key_env: "GLM_API_KEY".to_string(),
            context_window: 64000,
            max_tokens: 4096,
            supports_streaming: true,
            supports_vision: false,
            supports_tools: true,
            category: ModelCategory::Chat,
            pricing_input: 0.00015,
            pricing_output: 0.00045,
            recommended: true,
        });

        self.add_model(ModelInfo {
            id: "zhipu-4".to_string(),
            name: "智谱清言".to_string(),
            provider: ModelProvider::Zhipu,
            base_url: Some("https://open.bigmodel.cn/api/paas/v4/".to_string()),
            api_key_env: "ZHIPU_API_KEY".to_string(),
            context_window: 128000,
            max_tokens: 8192,
            supports_streaming: true,
            supports_vision: true,
            supports_tools: true,
            category: ModelCategory::Multimodal,
            pricing_input: 0.0015,
            pricing_output: 0.0045,
            recommended: true,
        });

        self.add_model(ModelInfo {
            id: "doubao-pro".to_string(),
            name: "豆包 Pro".to_string(),
            provider: ModelProvider::Doubao,
            base_url: Some("https://api.doubao.com/v1".to_string()),
            api_key_env: "DOUBAO_API_KEY".to_string(),
            context_window: 128000,
            max_tokens: 8192,
            supports_streaming: true,
            supports_vision: true,
            supports_tools: true,
            category: ModelCategory::Multimodal,
            pricing_input: 0.0008,
            pricing_output: 0.0024,
            recommended: true,
        });

        self.add_model(ModelInfo {
            id: "doubao-lite".to_string(),
            name: "豆包 Lite".to_string(),
            provider: ModelProvider::Doubao,
            base_url: Some("https://api.doubao.com/v1".to_string()),
            api_key_env: "DOUBAO_API_KEY".to_string(),
            context_window: 64000,
            max_tokens: 4096,
            supports_streaming: true,
            supports_vision: false,
            supports_tools: true,
            category: ModelCategory::Chat,
            pricing_input: 0.0001,
            pricing_output: 0.0003,
            recommended: true,
        });

        self.add_model(ModelInfo {
            id: "xunfei-spark-4".to_string(),
            name: "讯飞星火 4.0".to_string(),
            provider: ModelProvider::Xunfei,
            base_url: Some("https://spark-api.xf-yun.com/v1".to_string()),
            api_key_env: "XUNFEI_API_KEY".to_string(),
            context_window: 128000,
            max_tokens: 8192,
            supports_streaming: true,
            supports_vision: true,
            supports_tools: true,
            category: ModelCategory::Multimodal,
            pricing_input: 0.0012,
            pricing_output: 0.0036,
            recommended: true,
        });

        self.add_model(ModelInfo {
            id: "moonshot-v1-8k".to_string(),
            name: "Moonshot 8K".to_string(),
            provider: ModelProvider::Moonshot,
            base_url: Some("https://api.moonshot.cn/v1".to_string()),
            api_key_env: "MOONSHOT_API_KEY".to_string(),
            context_window: 8192,
            max_tokens: 2048,
            supports_streaming: true,
            supports_vision: false,
            supports_tools: true,
            category: ModelCategory::Chat,
            pricing_input: 0.0001,
            pricing_output: 0.0003,
            recommended: true,
        });

        self.add_model(ModelInfo {
            id: "moonshot-v1-32k".to_string(),
            name: "Moonshot 32K".to_string(),
            provider: ModelProvider::Moonshot,
            base_url: Some("https://api.moonshot.cn/v1".to_string()),
            api_key_env: "MOONSHOT_API_KEY".to_string(),
            context_window: 32768,
            max_tokens: 8192,
            supports_streaming: true,
            supports_vision: false,
            supports_tools: true,
            category: ModelCategory::Chat,
            pricing_input: 0.0002,
            pricing_output: 0.0006,
            recommended: true,
        });

        self.add_model(ModelInfo {
            id: "moonshot-v1-128k".to_string(),
            name: "Moonshot 128K".to_string(),
            provider: ModelProvider::Moonshot,
            base_url: Some("https://api.moonshot.cn/v1".to_string()),
            api_key_env: "MOONSHOT_API_KEY".to_string(),
            context_window: 131072,
            max_tokens: 16384,
            supports_streaming: true,
            supports_vision: false,
            supports_tools: true,
            category: ModelCategory::Chat,
            pricing_input: 0.0004,
            pricing_output: 0.0012,
            recommended: true,
        });

        self.add_model(ModelInfo {
            id: "llama-3-70b".to_string(),
            name: "Llama 3 70B".to_string(),
            provider: ModelProvider::Llama,
            base_url: None,
            api_key_env: "LLAMA_API_KEY".to_string(),
            context_window: 128000,
            max_tokens: 8192,
            supports_streaming: true,
            supports_vision: false,
            supports_tools: true,
            category: ModelCategory::Chat,
            pricing_input: 0.0003,
            pricing_output: 0.0009,
            recommended: false,
        });

        self.add_model(ModelInfo {
            id: "text-embedding-3-small".to_string(),
            name: "Text Embedding 3 Small".to_string(),
            provider: ModelProvider::OpenAI,
            base_url: None,
            api_key_env: "OPENAI_API_KEY".to_string(),
            context_window: 8191,
            max_tokens: 1536,
            supports_streaming: false,
            supports_vision: false,
            supports_tools: false,
            category: ModelCategory::Embedding,
            pricing_input: 0.0001,
            pricing_output: 0.0,
            recommended: true,
        });

        self.add_model(ModelInfo {
            id: "text-embedding-3-large".to_string(),
            name: "Text Embedding 3 Large".to_string(),
            provider: ModelProvider::OpenAI,
            base_url: None,
            api_key_env: "OPENAI_API_KEY".to_string(),
            context_window: 8191,
            max_tokens: 3072,
            supports_streaming: false,
            supports_vision: false,
            supports_tools: false,
            category: ModelCategory::Embedding,
            pricing_input: 0.0002,
            pricing_output: 0.0,
            recommended: false,
        });
    }

    fn add_model(&mut self, model: ModelInfo) {
        let id = model.id.clone();
        self.models.insert(id.clone(), model.clone());
        
        self.provider_models
            .entry(model.provider.clone())
            .or_default()
            .push(id.clone());
        
        self.category_models
            .entry(model.category.clone())
            .or_default()
            .push(id);
    }

    pub fn get_model(&self, model_id: &str) -> Option<&ModelInfo> {
        self.models.get(model_id)
    }

    pub fn list_models(&self) -> Vec<&ModelInfo> {
        self.models.values().collect()
    }

    pub fn list_models_by_provider(&self, provider: ModelProvider) -> Vec<&ModelInfo> {
        self.provider_models
            .get(&provider)
            .map(|ids| ids.iter().filter_map(|id| self.models.get(id)).collect())
            .unwrap_or_default()
    }

    pub fn list_models_by_category(&self, category: ModelCategory) -> Vec<&ModelInfo> {
        self.category_models
            .get(&category)
            .map(|ids| ids.iter().filter_map(|id| self.models.get(id)).collect())
            .unwrap_or_default()
    }

    pub fn list_recommended_models(&self) -> Vec<&ModelInfo> {
        self.models.values().filter(|m| m.recommended).collect()
    }

    pub fn list_code_models(&self) -> Vec<&ModelInfo> {
        self.list_models_by_category(ModelCategory::Code)
    }

    pub fn list_chat_models(&self) -> Vec<&ModelInfo> {
        self.list_models_by_category(ModelCategory::Chat)
    }

    pub fn list_multimodal_models(&self) -> Vec<&ModelInfo> {
        self.list_models_by_category(ModelCategory::Multimodal)
    }

    pub fn list_embedding_models(&self) -> Vec<&ModelInfo> {
        self.list_models_by_category(ModelCategory::Embedding)
    }

    pub fn list_chinese_models(&self) -> Vec<&ModelInfo> {
        let chinese_providers = [
            ModelProvider::Qwen,
            ModelProvider::Zhipu,
            ModelProvider::Doubao,
            ModelProvider::Xunfei,
            ModelProvider::Moonshot,
            ModelProvider::GLM,
            ModelProvider::DeepSeek,
        ];
        
        let mut result = Vec::new();
        for provider in chinese_providers.iter() {
            result.extend(self.list_models_by_provider(provider.clone()));
        }
        result
    }

    pub fn list_streaming_models(&self) -> Vec<&ModelInfo> {
        self.models.values().filter(|m| m.supports_streaming).collect()
    }

    pub fn list_tool_models(&self) -> Vec<&ModelInfo> {
        self.models.values().filter(|m| m.supports_tools).collect()
    }

    pub fn get_providers(&self) -> Vec<ModelProvider> {
        self.provider_models.keys().cloned().collect()
    }

    pub fn add_custom_model(&mut self, model: ModelInfo) -> Result<()> {
        if self.models.contains_key(&model.id) {
            return Err(anyhow!("Model with id '{}' already exists", model.id));
        }
        self.add_model(model);
        Ok(())
    }

    pub fn remove_model(&mut self, model_id: &str) -> Option<ModelInfo> {
        if let Some(model) = self.models.remove(model_id) {
            if let Some(ids) = self.provider_models.get_mut(&model.provider) {
                ids.retain(|id| id != model_id);
            }
            if let Some(ids) = self.category_models.get_mut(&model.category) {
                ids.retain(|id| id != model_id);
            }
            Some(model)
        } else {
            None
        }
    }

    pub fn estimate_cost(&self, model_id: &str, input_tokens: usize, output_tokens: usize) -> Result<f64> {
        let model = self.get_model(model_id)
            .ok_or_else(|| anyhow!("Model not found: {}", model_id))?;
        
        let input_cost = (input_tokens as f64 / 1000.0) * model.pricing_input;
        let output_cost = (output_tokens as f64 / 1000.0) * model.pricing_output;
        
        Ok(input_cost + output_cost)
    }

    pub fn get_supported_features(&self, model_id: &str) -> Option<ModelFeatures> {
        self.get_model(model_id).map(|model| ModelFeatures {
            streaming: model.supports_streaming,
            vision: model.supports_vision,
            tools: model.supports_tools,
            context_window: model.context_window,
            max_tokens: model.max_tokens,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ModelFeatures {
    pub streaming: bool,
    pub vision: bool,
    pub tools: bool,
    pub context_window: usize,
    pub max_tokens: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_models() {
        let registry = ModelsRegistry::new();
        let models = registry.list_models();
        assert!(models.len() > 10);
    }

    #[test]
    fn test_get_model() {
        let registry = ModelsRegistry::new();
        let model = registry.get_model("gpt-4o");
        assert!(model.is_some());
        assert_eq!(model.unwrap().name, "GPT-4o");
    }

    #[test]
    fn test_list_by_provider() {
        let registry = ModelsRegistry::new();
        let openai_models = registry.list_models_by_provider(ModelProvider::OpenAI);
        assert!(!openai_models.is_empty());
    }

    #[test]
    fn test_list_chinese_models() {
        let registry = ModelsRegistry::new();
        let chinese_models = registry.list_chinese_models();
        assert!(!chinese_models.is_empty());
    }

    #[test]
    fn test_recommended_models() {
        let registry = ModelsRegistry::new();
        let recommended = registry.list_recommended_models();
        assert!(!recommended.is_empty());
    }

    #[test]
    fn test_add_custom_model() {
        let mut registry = ModelsRegistry::new();
        let new_model = ModelInfo {
            id: "custom-model".to_string(),
            name: "Custom Model".to_string(),
            provider: ModelProvider::Other("Custom".to_string()),
            base_url: Some("https://api.example.com/v1".to_string()),
            api_key_env: "CUSTOM_API_KEY".to_string(),
            context_window: 8192,
            max_tokens: 1024,
            supports_streaming: true,
            supports_vision: false,
            supports_tools: true,
            category: ModelCategory::Chat,
            pricing_input: 0.001,
            pricing_output: 0.003,
            recommended: false,
        };
        
        registry.add_custom_model(new_model).unwrap();
        assert!(registry.get_model("custom-model").is_some());
    }

    #[test]
    fn test_estimate_cost() {
        let registry = ModelsRegistry::new();
        let cost = registry.estimate_cost("gpt-4o-mini", 1000, 1000);
        assert!(cost.is_ok());
        assert!(cost.unwrap() > 0.0);
    }
}