use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;

pub struct DoctorCommand;

impl Command for DoctorCommand {
    fn name(&self) -> &str { "doctor" }
    fn description(&self) -> &str { "System diagnosis" }
    async fn execute(&self, _args: &[String]) -> Result<CommandResult> {
        Ok(CommandResult::success("Doctor placeholder"))
    }
}
