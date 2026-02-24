use super::cli_common::{build_cli_prompt, run_cli_text_command};
use super::{EventStream, Provider};
use crate::message::{Message, ToolDefinition};
use anyhow::Result;
use async_trait::async_trait;
use std::path::Path;
use std::sync::{Arc, RwLock};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

const DEFAULT_MODEL: &str = "claude-sonnet-4";
const AVAILABLE_MODELS: &[&str] = &["claude-sonnet-4-6", "claude-sonnet-4", "gpt-5", "gpt-4.1"];

#[derive(Clone)]
struct CopilotInvocation {
    program: String,
    prefix_args: Vec<String>,
}

impl CopilotInvocation {
    fn command(&self) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.prefix_args);
        cmd
    }

    fn render_with(&self, args: &[&str]) -> String {
        let mut parts = vec![self.program.clone()];
        parts.extend(self.prefix_args.clone());
        parts.extend(args.iter().map(|s| s.to_string()));
        parts.join(" ")
    }
}

fn command_exists(command: &str) -> bool {
    let path = Path::new(command);
    if path.is_absolute() || command.contains('/') {
        return path.exists();
    }

    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(command).exists()))
        .unwrap_or(false)
}

fn resolve_copilot_invocation() -> CopilotInvocation {
    if let Ok(cli_path) = std::env::var("JCODE_COPILOT_CLI_PATH") {
        let trimmed = cli_path.trim();
        if !trimmed.is_empty() {
            return CopilotInvocation {
                program: trimmed.to_string(),
                prefix_args: Vec::new(),
            };
        }
    }

    if command_exists("copilot") {
        return CopilotInvocation {
            program: "copilot".to_string(),
            prefix_args: Vec::new(),
        };
    }

    if command_exists("gh") {
        return CopilotInvocation {
            program: "gh".to_string(),
            prefix_args: vec!["copilot".to_string(), "--".to_string()],
        };
    }

    CopilotInvocation {
        program: "copilot".to_string(),
        prefix_args: Vec::new(),
    }
}

pub fn copilot_login_command() -> (String, Vec<String>, String) {
    let invocation = resolve_copilot_invocation();
    let login_args = vec!["-i".to_string(), "/login".to_string()];
    let rendered = invocation.render_with(&["-i", "/login"]);
    (
        invocation.program,
        [invocation.prefix_args, login_args].concat(),
        rendered,
    )
}

pub struct CopilotCliProvider {
    invocation: CopilotInvocation,
    model: Arc<RwLock<String>>,
}

impl CopilotCliProvider {
    pub fn new() -> Self {
        let invocation = resolve_copilot_invocation();
        let model = std::env::var("JCODE_COPILOT_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into());
        Self {
            invocation,
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
        let invocation = self.invocation.clone();
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
            let mut cmd = invocation.command();
            cmd.arg("-p").arg(prompt).arg("--allow-all-tools");
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
            invocation: self.invocation.clone(),
            model: Arc::new(RwLock::new(self.model())),
        })
    }
}
