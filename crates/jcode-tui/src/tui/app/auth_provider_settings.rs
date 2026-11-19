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
    if !crate::auth::claude::list_accounts()
        .unwrap_or_default()
        .iter()
        .any(|account| account.label == label)
    {
        app.push_display_message(DisplayMessage::error(format!(
            "No saved Claude account `{label}`."
        )));
        return;
    }
    if enabled && !crate::auth::claude_code::binary_available() {
        app.push_display_message(DisplayMessage::error(format!(
            "Claude Code CLI `{}` not found. Install it, or set [provider.claude_code].binary.",
            crate::auth::claude_code::binary()
        )));
        return;
    }
    match crate::config::Config::set_claude_code_jcode_account(label, enabled, None) {
        Ok(()) => {
            let instance = crate::auth::claude_code::instances()
                .into_iter()
                .find(|i| i.jcode_account.as_deref() == Some(label))
                .map(|i| i.id)
                .unwrap_or_default();
            app.set_status_notice(format!(
                "Claude `{label}`: {}",
                if enabled {
                    "Claude Code CLI"
                } else {
                    "native API"
                }
            ));
            app.push_display_message(DisplayMessage::system(if enabled {
                format!(
                    "Claude account `{label}` now also runs through the Claude Code CLI with its own login. Pick a `claude-code` model for it in /model, or run /accounts claude-code switch {instance}."
                )
            } else {
                format!("Claude account `{label}` no longer runs through the Claude Code CLI.")
            }));
        }
        Err(err) => app.push_display_message(DisplayMessage::error(format!(
            "Failed to save Claude Code CLI setting: {err}"
        ))),
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
