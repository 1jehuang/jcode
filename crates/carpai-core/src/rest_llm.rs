use serde::{Deserialize, Serialize};

pub struct InferenceRouter {
    models: Vec<String>,
    fallback: String,
}

impl InferenceRouter {
    pub fn new(models: Vec<String>, fallback: &str) -> Self {
        Self { models, fallback: fallback.into() }
    }

    pub async fn chat_completion(&self, _prompt: &str, _system: &str) -> Result<String, String> {
        Err("InferenceRouter not yet implemented".into())
    }
}
