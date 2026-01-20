//! Mock provider for e2e tests
//!
//! Returns pre-scripted StreamEvent sequences for deterministic testing.

use anyhow::Result;
use async_stream::stream;
use jcode::message::{Message, StreamEvent, ToolDefinition};
use jcode::provider::{EventStream, Provider};
use std::collections::VecDeque;
use std::sync::Mutex;

pub struct MockProvider {
    responses: Mutex<VecDeque<Vec<StreamEvent>>>,
    models: Vec<&'static str>,
    current_model: Mutex<String>,
    /// Captured system prompts from complete() calls (for testing)
    pub captured_system_prompts: Mutex<Vec<String>>,
    /// Captured resume session IDs from complete() calls (for testing)
    pub captured_resume_session_ids: Mutex<Vec<Option<String>>>,
}

impl MockProvider {
    pub fn new() -> Self {
        Self {
            responses: Mutex::new(VecDeque::new()),
            models: Vec::new(),
            current_model: Mutex::new("mock".to_string()),
            captured_system_prompts: Mutex::new(Vec::new()),
            captured_resume_session_ids: Mutex::new(Vec::new()),
        }
    }

    pub fn with_models(models: Vec<&'static str>) -> Self {
        let current = models
            .first()
            .map(|m| (*m).to_string())
            .unwrap_or_else(|| "mock".to_string());
        Self {
            responses: Mutex::new(VecDeque::new()),
            models,
            current_model: Mutex::new(current),
            captured_system_prompts: Mutex::new(Vec::new()),
            captured_resume_session_ids: Mutex::new(Vec::new()),
        }
    }

    /// Queue a response (sequence of StreamEvents) to be returned on next complete() call
    pub fn queue_response(&self, events: Vec<StreamEvent>) {
        self.responses.lock().unwrap().push_back(events);
    }
}

#[async_trait::async_trait]
impl Provider for MockProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        system: &str,
        resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        // Capture the system prompt for testing
        self.captured_system_prompts
            .lock()
            .unwrap()
            .push(system.to_string());
        self.captured_resume_session_ids
            .lock()
            .unwrap()
            .push(resume_session_id.map(|s| s.to_string()));

        let events = self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_default();

        let stream = stream! {
            for event in events {
                yield Ok(event);
            }
        };

        Ok(Box::pin(stream))
    }

    fn name(&self) -> &str {
        "mock"
    }

    fn model(&self) -> String {
        self.current_model.lock().unwrap().clone()
    }

    fn set_model(&self, model: &str) -> Result<()> {
        if !self.models.is_empty() && !self.models.contains(&model) {
            anyhow::bail!("Unknown model: {}", model);
        }
        *self.current_model.lock().unwrap() = model.to_string();
        Ok(())
    }

    fn available_models(&self) -> Vec<&'static str> {
        self.models.clone()
    }
}
