use super::{App, DisplayMessage};
use crate::gateway::control::{
    RemoteCommand, RemoteStatus, ToggleOutcome, create_pairing_invite, parse_remote_command,
    revoke_device, set_gateway_enabled,
};

const REMOTE_HELP: &str = "\
**`/remote`** - reach this session from another machine

The gateway lets another computer or phone drive this jcode server over the
network, using the same protocol the local UI speaks.

- `/remote` or `/remote status` - gateway state, dial address, paired devices
- `/remote on` / `/remote off` - enable or disable the gateway
- `/remote pair` - show a pairing code and QR for a new device
- `/remote revoke <device>` - remove a paired device

Setup is: `/remote on`, restart the server, then `/remote pair`.
";

pub(super) fn handle_remote_command(app: &mut App, trimmed: &str) -> bool {
    let Some(parsed) = parse_remote_command(trimmed) else {
        return false;
    };

    match parsed {
        Err(message) => app.push_display_message(DisplayMessage::error(message)),
        Ok(RemoteCommand::Help) => {
            app.push_display_message(DisplayMessage::system(REMOTE_HELP.to_string()));
            app.set_status_notice("Remote help");
        }
        Ok(RemoteCommand::Status) => show_status(app),
        Ok(RemoteCommand::On) => toggle(app, true),
        Ok(RemoteCommand::Off) => toggle(app, false),
        Ok(RemoteCommand::Pair) => show_pairing_invite(app),
        Ok(RemoteCommand::Revoke(target)) => revoke(app, &target),
    }

    true
}

fn show_status(app: &mut App) {
    let status = RemoteStatus::load();
    app.push_display_message(DisplayMessage::system(status.to_markdown()));
    app.set_status_notice(if status.enabled {
        "Remote access on"
    } else {
        "Remote access off"
    });
}

fn toggle(app: &mut App, enabled: bool) {
    match set_gateway_enabled(enabled) {
        Ok(ToggleOutcome::Unchanged { .. }) => {
            app.push_display_message(DisplayMessage::system(format!(
                "Remote access is already **{}**.",
                if enabled { "on" } else { "off" }
            )));
        }
        Ok(ToggleOutcome::Changed { .. }) => {
            // The listener binds during server startup, so a config change
            // alone does not open or close the port.
            let body = if enabled {
                let status = RemoteStatus::load();
                format!(
                    "Remote access **enabled** on port `{}`.\n\n\
                     Restart the server to open the port:\n\n\
                     ```\njcode server reload\n```\n\n\
                     Then run `/remote pair` to authorize a device.\n\n\
                     Other machines will dial `{}`.\n",
                    status.port,
                    status.dial_address()
                )
            } else {
                "Remote access **disabled**.\n\n\
                 Restart the server to close the port:\n\n\
                 ```\njcode server reload\n```\n\n\
                 Paired devices are kept; use `/remote revoke <device>` to remove them.\n"
                    .to_string()
            };
            app.push_display_message(DisplayMessage::system(body));
            app.set_status_notice(if enabled {
                "Remote enabled (restart required)"
            } else {
                "Remote disabled (restart required)"
            });
        }
        Err(error) => app.push_display_message(DisplayMessage::error(format!(
            "Failed to update remote access: {}",
            crate::util::format_error_chain(&error)
        ))),
    }
}

fn show_pairing_invite(app: &mut App) {
    match create_pairing_invite() {
        Ok(invite) => {
            let mut body = String::from("**Pair a device**\n\n");

            if !invite.gateway_enabled {
                body.push_str(
                    "Remote access is currently **off**, so this code cannot be used yet. \
                     Run `/remote on` and restart the server first.\n\n",
                );
            }

            body.push_str(&format!(
                "- Pairing code: **{}** (expires in 5 minutes)\n- Connect to: `{}`\n",
                invite.spaced_code(),
                invite.dial_address
            ));

            if let Ok(qr) = crate::login_qr::render_unicode_qr(&invite.uri) {
                body.push_str("\n```\n");
                body.push_str(&qr);
                body.push_str("\n```\n");
            }

            app.push_display_message(DisplayMessage::system(body));
            app.set_status_notice("Pairing code ready");
        }
        Err(error) => app.push_display_message(DisplayMessage::error(format!(
            "Failed to create pairing code: {}",
            crate::util::format_error_chain(&error)
        ))),
    }
}

fn revoke(app: &mut App, target: &str) {
    match revoke_device(target) {
        Ok(removed) if removed.is_empty() => {
            app.push_display_message(DisplayMessage::error(format!(
                "No paired device matching `{target}`. Run `/remote status` to list devices."
            )));
        }
        Ok(removed) => {
            app.push_display_message(DisplayMessage::system(format!(
                "Revoked {}. That device must pair again to reconnect.",
                removed
                    .iter()
                    .map(|name| format!("`{name}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
            app.set_status_notice("Device revoked");
        }
        Err(error) => app.push_display_message(DisplayMessage::error(format!(
            "Failed to revoke device: {}",
            crate::util::format_error_chain(&error)
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `/remote` and `/remote-release` share a prefix, and only dispatch order
    /// plus the parser's whitespace rule keep them apart. Assert the boundary
    /// so a later edit cannot silently steal the other command's input.
    #[test]
    fn remote_parser_does_not_claim_remote_release() {
        assert_eq!(parse_remote_command("/remote-release"), None);
        assert_eq!(
            parse_remote_command("/remote"),
            Some(Ok(RemoteCommand::Status))
        );
    }

    /// The help text is the discoverability surface; keep it in sync with the
    /// subcommands the parser actually accepts.
    #[test]
    fn help_documents_every_supported_subcommand() {
        for sub in ["status", "on", "off", "pair", "revoke"] {
            assert!(
                REMOTE_HELP.contains(sub),
                "help should document /remote {sub}"
            );
            assert!(
                parse_remote_command(&format!("/remote {sub} x")).is_some(),
                "parser should recognize /remote {sub}"
            );
        }
    }
}
