//! Provider settings saved from `/accounts`.

use super::*;

/// `/accounts claude cli <label> <on|off>`.
pub(super) fn parse_claude_code_cli(value: &str) -> Result<AccountCommand, String> {
    let usage = || "Usage: /accounts claude cli <label> <on|off>".to_string();
    let (label, mode) = value.rsplit_once(char::is_whitespace).ok_or_else(usage)?;
    let enabled = match mode.to_ascii_lowercase().as_str() {
        "on" => true,
        "off" => false,
        _ => return Err(usage()),
    };
    Ok(AccountCommand::SetClaudeCodeCli {
        label: label.trim().to_string(),
        enabled,
    })
}

pub(super) fn save_claude_code_cli_setting(app: &mut App, label: &str, enabled: bool) {
    match crate::auth::claude_code::set_account_transport(label, enabled) {
        Ok(message) => {
            app.set_status_notice(format!(
                "Claude `{label}`: {}",
                if enabled {
                    "Claude Code CLI"
                } else {
                    "native API"
                }
            ));
            app.push_display_message(DisplayMessage::system(message));
        }
        Err(err) => app.push_display_message(DisplayMessage::error(format!("{err:#}"))),
    }
}

pub(super) fn save_copilot_premium_setting(app: &mut App, mode: Option<&str>) {
    use crate::provider::copilot::PremiumMode;

    let premium_mode = match mode.unwrap_or("normal") {
        "normal" => PremiumMode::Normal,
        "one" => PremiumMode::OnePerSession,
        "zero" => PremiumMode::Zero,
        other => {
            app.push_display_message(DisplayMessage::error(format!(
                "Copilot premium mode must be normal, one, or zero (got {}).",
                other
            )));
            return;
        }
    };
    app.provider.set_premium_mode(premium_mode);
    let result = match mode {
        None | Some("normal") => crate::config::Config::set_copilot_premium(None),
        Some(value) => crate::config::Config::set_copilot_premium(Some(value)),
    };
    match result {
        Ok(()) => {
            let label = match premium_mode {
                PremiumMode::Normal => "normal",
                PremiumMode::OnePerSession => "one premium per session",
                PremiumMode::Zero => "zero premium requests",
            };
            app.set_status_notice(format!("Premium: {}", label));
            app.push_display_message(DisplayMessage::system(format!(
                "Saved Copilot premium mode: {}.",
                label
            )));
        }
        Err(err) => app.push_display_message(DisplayMessage::error(format!(
            "Failed to save Copilot premium mode: {}",
            err
        ))),
    }
}
