//! `/overview`: toggle the consolidated single Overview panel against the
//! classic separate widgets, and pin/unpin its position.
//!
//! Every form persists to config.toml, and the confirmation reports the
//! effective live layout: when `JCODE_SINGLE_OVERVIEW` or
//! `JCODE_OVERVIEW_PINNED` pins the session to the opposite of the saved
//! choice, the message says so instead of claiming the choice applied
//! immediately (a saved choice with no visible effect is worse than a clear
//! notice).

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
            report_overview_status(app);
            return true;
        }
        "pin" | "pinned" => {
            set_overview_pinned(app, true);
            return true;
        }
        "unpin" | "dock" | "float" => {
            set_overview_pinned(app, false);
            return true;
        }
        _ => match parse_on_off_value(rest) {
            Some(value) => value,
            None => {
                app.push_display_message(DisplayMessage::error(
                    "Usage: /overview (toggle), /overview status, /overview on|off, /overview pinned, /overview dock",
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
            "Info panel was not changed: saving the choice to config failed ({}). \
             The live layout still follows the previous default ({}).",
            error,
            if crate::config::config().display.single_overview {
                "single overview"
            } else {
                "separate widgets"
            }
        ))),
    }

    true
}

fn report_overview_status(app: &mut App) {
    let single = crate::config::config().display.single_overview;
    let pinned = crate::config::config().display.overview_pinned;
    app.push_display_message(DisplayMessage::system(format!(
        "Info panel is currently {}.\n\nOverview placement: {}.\n\nOn: one consolidated Overview panel in the transcript margins.\nOff: separate widgets (Swarm, Background, Compaction, Runtime, Usage, KV cache, Git, Commits) each dock on their own.\nPinned: the Overview stays in the top-right corner and the transcript wraps before reaching it.\nDocked: the Overview floats in the margins like the other widgets.\n\nUse /overview, /overview on|off, /overview pinned, or /overview dock to change it.",
        if single { "single overview (on)" } else { "separate widgets (off)" },
        if pinned { "pinned top-right (HUD)" } else { "docked in margins" },
    )));
}

fn set_overview_pinned(app: &mut App, pinned: bool) {
    app.set_status_notice(if pinned {
        "Info panel: pinned top-right"
    } else {
        "Info panel: docked in margins"
    });
    match crate::config::Config::set_overview_pinned(pinned) {
        Ok(()) => {
            // JCODE_OVERVIEW_PINNED can pin the live placement to the
            // opposite of the saved choice, so report the effective state
            // instead of claiming the choice applied.
            let effective = crate::config::config().display.overview_pinned;
            app.push_display_message(DisplayMessage::system(format!(
                "Saved overview placement: {}. {}",
                if pinned { "pinned top-right" } else { "docked in margins" },
                if effective == pinned {
                    "Applied to this session immediately.".to_string()
                } else {
                    format!(
                        "This session keeps the placement from JCODE_OVERVIEW_PINNED ({}); the saved default applies to fresh sessions.",
                        if effective {
                            "pinned top-right"
                        } else {
                            "docked in margins"
                        }
                    )
                },
            )))
        }
        Err(error) => app.push_display_message(DisplayMessage::error(format!(
            "Overview placement was not changed: saving the choice to config failed ({}). \
             The live placement still follows the previous default ({}).",
            error,
            if crate::config::config().display.overview_pinned {
                "pinned top-right"
            } else {
                "docked in margins"
            }
        ))),
    }
}
