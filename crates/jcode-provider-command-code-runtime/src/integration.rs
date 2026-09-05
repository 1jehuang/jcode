use crate::{auth::CommandCodeAccount, project_context::project_context_cache, CommandCodeProvider};
use anyhow::{anyhow, Result};

/// Compose a provider only from a verified daemon account.
pub fn compose_provider(account: CommandCodeAccount, model: &str) -> Result<CommandCodeProvider> {
    if account.api_key.trim().is_empty() || account.user_id.trim().is_empty() || account.user_name.trim().is_empty() {
        return Err(anyhow!("Command Code account is not whoami-verified"));
    }
    let catalog = crate::models::CommandCodeCatalog::new();
    let canonical = crate::models::canonicalize_command_code_model(model, &catalog).unwrap_or_else(|| crate::models::default_command_code_model().to_string());
    Ok(CommandCodeProvider::new(account.api_key, account.label.unwrap_or_else(|| "command-code".into()), canonical))
}

pub fn bounded_context() -> crate::project_context::ProjectContext { project_context_cache(std::env::current_dir().unwrap_or_default()) }
