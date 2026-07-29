//! The single dispatch table for slash commands that run entirely inside this
//! process.
//!
//! There used to be two hand-maintained copies of this chain: one in the local
//! TUI submit path and one for a remote client whose connection had dropped.
//! They drifted, so commands like `/cancel`, `/ssh`, `/productivity`, and
//! `/model-status` silently did nothing in the remote-disconnected case while
//! working locally. Both call sites now share this list, so adding a handler
//! here makes it reachable from every in-process entry point at once.

use super::App;

/// Run `trimmed` against every locally handled slash command.
///
/// Returns `true` when a handler claimed the input. Callers own presentation
/// concerns (clearing the input line, telemetry) because those differ between
/// the local and remote entry points.
pub(super) fn dispatch_local_command(app: &mut App, trimmed: &str) -> bool {
    super::commands::handle_cancel_command(app, trimmed)
        || super::commands::handle_help_command(app, trimmed)
        || super::commands::handle_keys_command(app, trimmed)
        || super::commands::handle_ssh_command(app, trimmed)
        // `/test`, `/mission`, `/goal`, and `/goals` are dispatched inside
        // `handle_session_command`, so they need no separate entries here.
        || super::commands::handle_session_command(app, trimmed)
        || super::commands::handle_dictation_command(app, trimmed)
        || super::commands::handle_config_command(app, trimmed)
        || super::commands_colors::handle_colors_command(app, trimmed)
        || super::commands::handle_log_command(app, trimmed)
        || super::commands::handle_diff_command(app, trimmed)
        || super::commands::handle_model_status_command(app, trimmed)
        || super::debug::handle_debug_command(app, trimmed)
        || super::model_context::handle_model_command(app, trimmed)
        || super::commands::handle_usage_command(app, trimmed)
        || super::productivity::handle_productivity_command(app, trimmed)
        || super::commands::handle_feedback_command(app, trimmed)
        || super::commands::handle_telemetry_command(app, trimmed)
        || super::support::handle_support_command(app, trimmed)
        || super::state_ui::handle_info_command(app, trimmed)
        || super::auth::handle_auth_command(app, trimmed)
        || super::tui_lifecycle_runtime::handle_dev_command(app, trimmed)
}

#[cfg(test)]
mod tests {
    /// The bug this module exists to prevent: two hand-copied dispatch chains
    /// that drift apart. Both entry points must call this shared table and
    /// must not rebuild a chain of their own.
    #[test]
    fn both_entry_points_use_the_shared_dispatch_table() {
        for (path, source) in [
            ("input.rs", include_str!("input.rs")),
            ("remote.rs", include_str!("remote.rs")),
        ] {
            assert!(
                source.contains("commands_dispatch::dispatch_local_command"),
                "{path} should dispatch slash commands through the shared table"
            );
            // A local `||`-chain of `handle_*_command` calls is how the two
            // copies drifted before; catching it here keeps the table single.
            let chained = source
                .lines()
                .filter(|line| line.trim_start().starts_with("|| ") && line.contains("_command("))
                .count();
            assert_eq!(
                chained, 0,
                "{path} rebuilds its own slash-command chain; extend \
                 dispatch_local_command instead"
            );
        }
    }
}
