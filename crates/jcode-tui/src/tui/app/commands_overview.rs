//! `/overview`: toggle the consolidated single Overview panel against the
//! classic separate widgets.
//!
//! Every form persists to config.toml, and the confirmation reports the
//! effective live layout: when `JCODE_SINGLE_OVERVIEW` pins the session to
//! the opposite of the saved choice, the message says so instead of
//! claiming the choice applied immediately (a saved choice with no visible
//! effect is worse than a clear notice).

use super::commands::parse_on_off_value;
use super::{App, DisplayMessage};

pub(super) fn handle_overview_command(app: &mut App, trimmed: &str) -> bool {
    if trimmed != "/overview" && !trimmed.starts_with("/overview ") {
        return false;
    }

    // The guard above ensures the prefix is present; let-else keeps the
    // strip total without a swallowed-error default.
    let Some(rest) = trimmed.strip_prefix("/overview") else {
        return false;
    };
    let rest = rest.trim();

    let enabled = match rest {
        "" => !crate::config::config().display.single_overview,
        "show" | "status" => {
            let current = crate::config::config().display.single_overview;
            app.push_display_message(DisplayMessage::system(format!(
                "Info panel is currently {}.\n\nOn: one consolidated Overview panel in the transcript margins.\nOff: separate widgets (Todos, Swarm, Background, Compaction, Runtime, Usage, KV cache, Git, Commits) each dock on their own.\n\nUse /overview, /overview on, or /overview off to change it.",
                if current { "single overview (on)" } else { "separate widgets (off)" }
            )));
            return true;
        }
        _ => match parse_on_off_value(rest) {
            Some(value) => value,
            None => {
                app.push_display_message(DisplayMessage::error(
                    "Usage: /overview (toggle), /overview status, /overview on, or /overview off",
                ));
                return true;
            }
        },
    };

    app.set_status_notice(if enabled {
        "Info panel: single overview"
    } else {
        "Info panel: separate widgets"
    });
    match crate::config::Config::set_single_overview(enabled) {
        Ok(()) => {
            // The env override (JCODE_SINGLE_OVERVIEW) can pin the live
            // layout to the opposite of the saved choice, so report the
            // effective setting instead of claiming the choice applied.
            let effective = crate::config::config().display.single_overview;
            app.push_display_message(DisplayMessage::system(format!(
                "Saved {}: {}. {}",
                if enabled {
                    "single overview"
                } else {
                    "separate widgets"
                },
                if enabled { "on" } else { "off" },
                if effective == enabled {
                    "Applied to this session immediately.".to_string()
                } else {
                    format!(
                        "This session keeps the layout from JCODE_SINGLE_OVERVIEW ({}); the saved default applies to fresh sessions.",
                        if effective { "overview on" } else { "overview off" }
                    )
                },
            )))
        }
        Err(error) => app.push_display_message(DisplayMessage::error(format!(
            "Applied {} for this session, but failed to save it as the default: {}",
            if enabled {
                "single overview"
            } else {
                "separate widgets"
            },
            error
        ))),
    }

    true
}
