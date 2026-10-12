use super::*;
use jcode_provider_command_code_runtime::auth::CommandCodeLoginStep;

pub(super) fn start_login(app: &mut App) {
    app.set_status_notice("Login: command code...");
    app.push_display_message(DisplayMessage::system(
        "Starting Command Code browser login...".to_string(),
    ));
    tokio::spawn(async move {
        let prepared =
            match jcode_provider_command_code_runtime::auth::prepare_command_code_login().await {
                Ok(step) => step,
                Err(error) => {
                    publish(false, format!("Command Code login failed: {error}"));
                    return;
                }
            };
        match prepared {
            CommandCodeLoginStep::Imported { label } => {
                publish(true, format!("Imported Command Code account `{label}`."));
            }
            CommandCodeLoginStep::NeedsBrowser { login } => {
                publish(
                    true,
                    format!(
                        "Command Code login\n\nOpen this URL if the browser does not:\n{}\n\nWaiting for the browser callback...",
                        login.url
                    ),
                );
                let _ = App::open_auth_browser(&login.url);
                let store = match jcode_provider_command_code_runtime::auth::auth_store_path() {
                    Ok(path) => path,
                    Err(error) => {
                        publish(false, format!("Command Code login failed: {error}"));
                        return;
                    }
                };
                match login.finish(&store, None).await {
                    Ok(label) => publish(true, format!("Logged in to Command Code (`{label}`).")),
                    Err(error) => publish(false, format!("Command Code login failed: {error}")),
                }
            }
        }
    });
}

pub(super) fn logout_fallback(
    provider: crate::provider_catalog::LoginProviderDescriptor,
) -> anyhow::Result<String> {
    if matches!(
        provider.target,
        crate::provider_catalog::LoginProviderTarget::CommandCode
    ) {
        let path = crate::storage::app_config_dir()?.join("command_code_accounts.json");
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        return Ok("Logged out of Command Code.".to_string());
    }
    Ok(format!(
        "Logout for {} is not automated yet. Remove its saved API key or external CLI session from /account {} settings.",
        provider.display_name, provider.id
    ))
}

fn publish(success: bool, message: String) {
    Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
        provider: "command-code".to_string(),
        success,
        message,
    }));
}
