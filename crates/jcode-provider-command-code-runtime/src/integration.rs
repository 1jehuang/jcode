use crate::{
    CommandCodeProvider, auth::CommandCodeAccount, project_context::project_context_cache,
};
use anyhow::{Result, anyhow};
use jcode_provider_core::Provider;

/// Compose a provider only from a verified daemon account.
pub fn compose_provider(account: CommandCodeAccount, model: &str) -> Result<CommandCodeProvider> {
    if account.api_key.trim().is_empty()
        || account.user_id.trim().is_empty()
        || account.user_name.trim().is_empty()
    {
        return Err(anyhow!("Command Code account is not whoami-verified"));
    }
    let provider = CommandCodeProvider::new(
        account.api_key,
        format!("command-code-{}", account.user_id),
        crate::models::default_command_code_model().to_string(),
    );
    let catalog = provider.catalog.clone();
    let key = provider.api_key.clone();
    let _ = std::thread::spawn(move || {
        if let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            let _ = runtime.block_on(catalog.refresh_live(&reqwest::Client::new(), &key));
        }
    })
    .join();
    let canonical = crate::models::canonicalize_command_code_model(model, &provider.catalog)
        .unwrap_or_else(|| crate::models::default_command_code_model().to_string());
    provider.set_model(&canonical)?;
    Ok(provider)
}

/// Compose from the daemon-owned persisted account set so pre-stream quota
/// failover can rotate credentials without touching caller-owned state.
pub fn compose_provider_from_store(
    store: &crate::auth::CommandCodeStore,
    model: &str,
) -> Result<CommandCodeProvider> {
    let active = store.active.as_deref();
    let accounts = store
        .accounts
        .iter()
        .filter(|a| {
            !a.api_key.trim().is_empty()
                && !a.user_id.trim().is_empty()
                && !a.user_name.trim().is_empty()
        })
        .collect::<Vec<_>>();
    let selected = accounts
        .iter()
        .find(|a| active.is_none() || a.label.as_deref() == active)
        .or_else(|| accounts.first())
        .ok_or_else(|| anyhow::anyhow!("no verified Command Code account"))?;
    let provider = compose_provider((*selected).clone(), model)?;
    Ok(provider.with_pool(
        accounts
            .into_iter()
            .map(|a| (a.label.clone().unwrap_or_default(), a.api_key.clone()))
            .collect(),
    ))
}

pub fn bounded_context() -> crate::project_context::ProjectContext {
    project_context_cache(std::env::current_dir().unwrap_or_default())
}
