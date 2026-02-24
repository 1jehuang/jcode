use super::cli_common::{build_cli_prompt, run_cli_text_command};
use super::{EventStream, Provider};
use crate::message::{Message, ToolDefinition};
use anyhow::Result;
use async_trait::async_trait;
use std::sync::{Arc, RwLock};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

const DEFAULT_MODEL: &str = "default";
const AVAILABLE_MODELS: &[&str] = &["default"];

pub struct AntigravityCliProvider {
    cli_path: String,
    model: Arc<RwLock<String>>,
    prompt_flag: Option<String>,
    model_flag: Option<String>,
}

impl AntigravityCliProvider {
    pub fn new() -> Self {
        let cli_path = std::env::var("JCODE_ANTIGRAVITY_CLI_PATH")
            .unwrap_or_else(|_| "antigravity".to_string());
        let model =
            std::env::var("JCODE_ANTIGRAVITY_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into());
        let prompt_flag = std::env::var("JCODE_ANTIGRAVITY_PROMPT_FLAG")
            .ok()
            .or_else(|| Some("-p".to_string()));
        let model_flag = std::env::var("JCODE_ANTIGRAVITY_MODEL_FLAG")
            .ok()
            .or_else(|| Some("--model".to_string()));

        Self {
            cli_path,
            model: Arc::new(RwLock::new(model)),
            prompt_flag,
            model_flag,
        }
    }
}

impl Default for AntigravityCliProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for AntigravityCliProvider {
    async fn complete(
        &self,
        messages: &[Message],
        _tools: &[ToolDefinition],
        system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let prompt = build_cli_prompt(system, messages);
        let model = self.model.read().unwrap().clone();
        let cli_path = self.cli_path.clone();
        let prompt_flag = self.prompt_flag.clone();
        let model_flag = self.model_flag.clone();
        let cwd = std::env::current_dir().ok();
        let (tx, rx) = mpsc::channel::<Result<crate::message::StreamEvent>>(100);

        tokio::spawn(async move {
            if tx
                .send(Ok(crate::message::StreamEvent::ConnectionType {
                    connection: "cli subprocess".to_string(),
                }))
                .await
                .is_err()
            {
                return;
            }
            let mut cmd = Command::new(&cli_path);
            if let Some(flag) = model_flag.as_ref().filter(|f| !f.trim().is_empty()) {
                cmd.arg(flag).arg(&model);
            }
            if let Some(flag) = prompt_flag.as_ref().filter(|f| !f.trim().is_empty()) {
                cmd.arg(flag).arg(prompt);
            } else {
                cmd.arg(prompt);
            }
            if let Some(dir) = cwd {
                cmd.current_dir(dir);
            }

            if let Err(e) = run_cli_text_command(cmd, tx.clone(), "Antigravity").await {
                let _ = tx.send(Err(e)).await;
            }
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    fn name(&self) -> &'static str {
        "antigravity"
    }

    fn model(&self) -> String {
        self.model.read().unwrap().clone()
    }

    fn set_model(&self, model: &str) -> Result<()> {
        let trimmed = model.trim();
        if trimmed.is_empty() {
            anyhow::bail!("Antigravity model cannot be empty");
        }
        *self.model.write().unwrap() = trimmed.to_string();
        Ok(())
    }

    fn available_models(&self) -> Vec<&'static str> {
        AVAILABLE_MODELS.to_vec()
    }

    fn supports_compaction(&self) -> bool {
        false
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self {
            cli_path: self.cli_path.clone(),
            model: Arc::new(RwLock::new(self.model())),
            prompt_flag: self.prompt_flag.clone(),
            model_flag: self.model_flag.clone(),
        })
    }
}
