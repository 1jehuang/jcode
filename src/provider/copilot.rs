use super::cli_common::{build_cli_prompt, run_cli_text_command};
use super::{EventStream, Provider};
use crate::message::{Message, ToolDefinition};
use anyhow::Result;
use async_trait::async_trait;
use std::sync::{Arc, RwLock};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

const DEFAULT_MODEL: &str = "claude-sonnet-4";
const AVAILABLE_MODELS: &[&str] = &["claude-sonnet-4", "gpt-5", "gpt-4.1"];

pub struct CopilotCliProvider {
    cli_path: String,
    model: Arc<RwLock<String>>,
}

impl CopilotCliProvider {
    pub fn new() -> Self {
        let cli_path =
            std::env::var("JCODE_COPILOT_CLI_PATH").unwrap_or_else(|_| "copilot".to_string());
        let model = std::env::var("JCODE_COPILOT_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into());
        Self {
            cli_path,
            model: Arc::new(RwLock::new(model)),
        }
    }
}

impl Default for CopilotCliProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for CopilotCliProvider {
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
        let cwd = std::env::current_dir().ok();
        let (tx, rx) = mpsc::channel::<Result<crate::message::StreamEvent>>(100);

        tokio::spawn(async move {
            let mut cmd = Command::new(&cli_path);
            cmd.arg("chat").arg("-p").arg(prompt);
            if !model.trim().is_empty() {
                cmd.arg("--model").arg(model);
            }
            if let Some(dir) = cwd {
                cmd.current_dir(dir);
            }

            if let Err(e) = run_cli_text_command(cmd, tx.clone(), "Copilot").await {
                let _ = tx.send(Err(e)).await;
            }
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    fn name(&self) -> &'static str {
        "copilot"
    }

    fn model(&self) -> String {
        self.model.read().unwrap().clone()
    }

    fn set_model(&self, model: &str) -> Result<()> {
        let trimmed = model.trim();
        if trimmed.is_empty() {
            anyhow::bail!("Copilot model cannot be empty");
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
        })
    }
}
