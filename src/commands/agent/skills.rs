use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;

pub struct SkillsCommand;

impl Command for SkillsCommand {
    fn name(&self) -> &str { "skills" }
    fn description(&self) -> &str { "Manage skills" }
    async fn execute(&self, _args: &[String]) -> Result<CommandResult> {
        Ok(CommandResult::success("Skills placeholder"))
    }
}
