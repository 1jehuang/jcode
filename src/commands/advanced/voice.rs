use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;

pub struct VoiceCommand;

impl Command for VoiceCommand {
    fn name(&self) -> &str { "voice" }
    fn description(&self) -> &str { "Voice mode" }
    async fn execute(&self, _args: &[String]) -> Result<CommandResult> {
        Ok(CommandResult::success("Voice placeholder"))
    }
}
