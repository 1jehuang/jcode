use super::maybe_open_browser;
use anyhow::Result;
use jcode_provider_command_code_runtime::auth::CommandCodeLoginStep;

pub(super) async fn login_command_code_flow(
    account_label: Option<&str>,
    no_browser: bool,
) -> Result<()> {
    match jcode_provider_command_code_runtime::auth::prepare_command_code_login().await? {
        CommandCodeLoginStep::Imported { label } => {
            eprintln!("Imported Command Code account `{label}` from ~/.commandcode/auth.json.");
        }
        CommandCodeLoginStep::NeedsBrowser { login } => {
            eprintln!("Open this URL to authorize Command Code:\n  {}", login.url);
            maybe_open_browser(&login.url, no_browser);
            eprintln!("Waiting for the Command Code browser callback...");
            let label = login
                .finish(
                    &jcode_provider_command_code_runtime::auth::auth_store_path()?,
                    account_label.map(str::to_string),
                )
                .await?;
            eprintln!("Logged in to Command Code (`{label}`).");
        }
    }
    crate::telemetry::record_auth_success("command-code", "oauth");
    Ok(())
}
