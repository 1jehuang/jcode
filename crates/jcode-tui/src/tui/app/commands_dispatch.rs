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

/// Local-only operations must not interpret the remote session's paths or IDs
/// against the laptop. Wire-backed commands are handled before local dispatch.
///
/// A command belongs on this list only when it is genuinely meaningless or
/// misleading over SSH: it reads/writes laptop files while the real state lives
/// on the server (auth, session metadata, todos, telemetry DBs), opens laptop
/// terminals/paths, or configures the laptop client in ways the remote session
/// cannot observe. Commands with a wire handler in `remote::key_handling` or a
/// client-local presentation handler in `dispatch_ssh_local_command` must NOT
/// be listed here — the list runs first, so entries shadow their real handlers.
pub(super) fn ssh_unsupported_command(input: &str) -> bool {
    let mut words = input.split_whitespace();
    let command = words.next().unwrap_or_default();
    // `/fast default` writes the *client's* startup default to the laptop
    // config; the session-scoped `/fast on|off` is wire-backed and allowed.
    if command == "/fast" && words.next() == Some("default") {
        return true;
    }
    matches!(
        command,
        // Accounts, credentials, and hosted billing live on the SSH server.
        // `/login` has a dedicated SSH flow; the rest have no wire request yet.
        "/logout"
            | "/auth"
            | "/account"
            | "/accounts"
            | "/hosted"
            | "/subscribe"
            | "/subscription"
            | "/usage"
            // Server-owned config/state files the laptop copy cannot
            // stand in for.
            | "/config"
            | "/permissions"
            | "/permission"
            | "/agents"
            | "/swarm-prompt"
            | "/initiatives"
            | "/goals"
            | "/overnight"
            // Session metadata and per-session state owned by the server's
            // session store; no wire request exists yet. (`/save`, `/unsave`,
            // `/todo`, and `/todos` were removed when `set_session_saved` and
            // `get_todos` landed — keep this list limited to gaps.)
            | "/fix"
            // Actions that would open paths or terminals on the laptop while
            // the real files live on the remote host.
            | "/transcript"
            | "/open"
            | "/file"
            | "/new-terminal"
            | "/selfdev"
            | "/ssh"
            | "/remote"
            | "/log"
            | "/support"
            // Laptop-local stats/debug databases and provider knobs that do
            // not describe the remote session.
            | "/stats"
            | "/productivity"
            | "/wrapped"
            | "/model-status"
            | "/provider-test-coverage"
            | "/cache"
            // Simulators exercise local onboarding/update code paths.
            | "/update-sim"
            | "/onboarding-sim"
            | "/onboarding-preview"
    )
}

/// `/resume`-family picker opens are safe over SSH: the session list is
/// fetched from the remote daemon (`list_sessions`), previews are fetched
/// lazily (`session_preview`), and resuming goes over the wire
/// (`resume_session`). Other session-shaped commands stay local-only and are
/// still blocked by the table above.
fn handle_ssh_session_picker_command(app: &mut App, trimmed: &str) -> bool {
    if matches!(trimmed, "/resume" | "/sessions" | "/session") {
        app.open_session_picker();
        return true;
    }
    false
}

/// Slash commands that are safe to run inside an SSH client: they only render
/// or configure what *this* process owns (the transcript view, display prefs,
/// keybindings, telemetry consent, dictation microphone, debug UI tooling).
/// Anything that would interpret remote session state against laptop files is
/// not reachable from here.
///
/// Unlike [`dispatch_local_command`]'s SSH branch, this returns `false` for
/// unmatched slash input so the connected remote path can still route unknown
/// commands as remote skill invocations or plain prompts.
pub(super) fn dispatch_ssh_local_command(app: &mut App, trimmed: &str) -> bool {
    if super::commands::handle_cancel_command(app, trimmed)
        || super::commands::handle_help_command(app, trimmed)
        || super::commands::handle_diff_command(app, trimmed)
        || handle_ssh_session_picker_command(app, trimmed)
    {
        return true;
    }
    // `/cls` clears only the rendered view in this client.
    if trimmed == "/cls" || trimmed == "/clear-view" {
        app.clear_view_keep_context();
        return true;
    }
    // Client-side presentation and consent commands. `handle_config_command`
    // is an umbrella handler; gating the command names first guarantees only
    // the display-preference sub-handlers can fire (server-owned `/config`,
    // `/agents`, etc. stay blocked upstream).
    let command = trimmed.split_whitespace().next().unwrap_or_default();
    let is_display_pref = matches!(
        command,
        "/alignment"
            | "/reasoning"
            | "/thinking"
            | "/thinking-display"
            | "/compact-notifications"
            | "/show-agentgrep-output"
            | "/tool-call-details"
    );
    if is_display_pref && super::commands::handle_config_command(app, trimmed) {
        return true;
    }
    super::commands::handle_keys_command(app, trimmed)
        || super::commands::handle_dictation_command(app, trimmed)
        || super::commands_colors::handle_colors_command(app, trimmed)
        || super::commands::handle_feedback_command(app, trimmed)
        || super::commands::handle_telemetry_command(app, trimmed)
        || super::debug::handle_debug_command(app, trimmed)
        || super::state_ui::handle_info_command(app, trimmed)
}

/// Return true after explaining why a laptop-local action is unavailable.
pub(super) fn ssh_local_action_blocked(app: &mut App, action: &str) -> bool {
    if !crate::tui::is_ssh_remote() {
        return false;
    }
    let host = crate::tui::ssh_remote_host().unwrap_or_else(|| "the remote host".to_string());
    let notice = format!(
        "{action} is unavailable in SSH mode. Nothing was changed on this computer. \
         Connect to {host} with SSH and run jcode there for remote setup or file/session operations. \
         Prompts and server-side tools remain available here."
    );
    app.set_status_notice(notice.clone());
    app.push_display_message(super::DisplayMessage::system(notice));
    true
}

/// Use before connected-remote dispatch as well as the disconnected fallback.
pub(super) fn handle_ssh_unsupported_command(app: &mut App, input: &str) -> bool {
    if app.handle_ssh_login_command(input) {
        return true;
    }
    ssh_unsupported_command(input)
        && ssh_local_action_blocked(
            app,
            input.split_whitespace().next().unwrap_or("This action"),
        )
}

/// Run `trimmed` against every locally handled slash command.
///
/// Returns `true` when a handler claimed the input. Callers own presentation
/// concerns (clearing the input line, telemetry) because those differ between
/// the local and remote entry points.
pub(super) fn dispatch_local_command(app: &mut App, trimmed: &str) -> bool {
    if handle_ssh_unsupported_command(app, trimmed) {
        return true;
    }
    // Anything not explicitly audited as presentation-only must stay on the
    // remote dispatcher. In particular, a disconnected client must not fall
    // back to local session/model/storage handlers for a remote command.
    if crate::tui::is_ssh_remote() {
        if dispatch_ssh_local_command(app, trimmed) {
            return true;
        }
        if trimmed.starts_with('/') {
            return ssh_local_action_blocked(app, "This command's local fallback");
        }
        return false;
    }
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

/// Environment-mode tests run in their own process, so parallel local-mode
/// tests never observe an SSH flag or another test's temporary home.
#[cfg(test)]
pub(super) fn ssh_test_runs_in_child(test_name: &str) -> bool {
    if std::env::var("JCODE_SSH_UI_TEST").as_deref() == Ok(test_name) {
        return false;
    }
    let home = tempfile::tempdir().expect("isolated SSH test home");
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .arg(test_name)
        .arg("--test-threads=1")
        .arg("--nocapture")
        .env("JCODE_SSH_UI_TEST", test_name)
        .env("JCODE_SSH_REMOTE", "test-remote")
        .env("JCODE_HOME", home.path())
        .output()
        .expect("run isolated SSH test");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    true
}

#[cfg(test)]
mod tests {
    #[test]
    fn ssh_ui_blocks_local_actions_and_preserves_text_and_cancel() {
        if super::ssh_test_runs_in_child(
            "ssh_ui_blocks_local_actions_and_preserves_text_and_cancel",
        ) {
            return;
        }
        use crate::tui::app::{input, tests::create_test_app};
        let mut app = create_test_app();
        app.is_remote = true;
        for command in ["/config init", "/logout", "/save test", "/reload", "/git"] {
            assert!(super::dispatch_local_command(&mut app, command));
            assert!(
                app.display_messages
                    .last()
                    .unwrap()
                    .content
                    .contains("SSH mode")
            );
        }
        for command in ["/login", "/account", "/subagent-model"] {
            app.input = command.to_string();
            app.sync_model_picker_preview_from_input();
            assert!(app.inline_interactive_state.is_none(), "{command}");
        }
        app.open_session_picker();
        // The remote picker opens immediately as a loading overlay; the
        // daemon `list_sessions` request is queued for the remote poll loop.
        assert!(app.session_picker_overlay.is_some());
        assert!(app.pending_remote_session_list);
        app.session_picker_overlay = None;
        app.pending_remote_session_list = false;
        // `/active` and `/catchup` use the same remote-list picker flow.
        app.open_active_sessions_picker();
        assert!(app.session_picker_overlay.is_some());
        assert!(app.pending_remote_session_list);
        app.session_picker_overlay = None;
        app.pending_remote_session_list = false;
        app.open_catchup_picker();
        assert!(app.session_picker_overlay.is_some());
        assert!(app.pending_remote_session_list);
        app.session_picker_overlay = None;
        app.pending_remote_session_list = false;
        app.open_login_picker_inline();
        app.open_agents_picker();
        app.handle_new_terminal_hotkey();
        app.toggle_next_prompt_new_session_routing();
        assert!(app.session_picker_overlay.is_none());
        assert!(app.pending_session_picker_load.is_none());
        assert!(app.inline_interactive_state.is_none());
        assert!(!app.route_next_prompt_to_new_session);

        let image = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        std::fs::write(image.path(), b"local image bytes").unwrap();
        let path = image.path().to_string_lossy().to_string();
        app.input.clear();
        app.cursor_pos = 0;
        input::handle_paste(&mut app, path.clone());
        assert_eq!(app.input, path);
        assert!(app.pending_images.is_empty());
        assert!(input::parse_dropped_paths(&path).is_none());
        assert!(!input::promote_dropped_images(&mut app));

        app.input.clear();
        app.cursor_pos = 0;
        input::insert_input_text(&mut app, "ordinary remote prompt");
        assert_eq!(app.input, "ordinary remote prompt");
        app.is_processing = true;
        app.input = "/cancel".to_string();
        app.submit_input();
        assert!(app.cancel_requested);
        assert!(app.input.is_empty());
    }

    #[test]
    fn ssh_blocks_local_side_effects_but_preserves_wire_commands() {
        for command in [
            "/logout all",
            "/account add",
            "/config edit",
            "/permissions",
            "/open /etc/passwd",
            "/selfdev status",
            "/fast default on",
            "/agents",
            "/transcript path",
            "/ssh connect host",
            "/fix",
            "/usage",
            "/overnight",
            "/hosted",
            "/subscribe",
            "/cache",
            "/stats",
        ] {
            assert!(super::ssh_unsupported_command(command), "{command}");
        }
        for command in [
            "/login",
            "hello",
            "explain !commands",
            "/cancel",
            "/resume",
            "/sessions",
            "/session",
            "/model remote-model",
            "/effort high",
            "/server-reload",
            "/compact",
            "/plan investigate",
            "/commit",
            "/rename title",
            "/fast on",
            "/help",
            "/diff",
            "/login-custom-skill",
            "/configurable-skill",
            // Wire-backed or client-rendered commands — not blocked.
            "/reload",
            "/client-reload",
            "/restart",
            "/rebuild",
            "/git",
            "/save name",
            "/unsave",
            "/todo",
            "/todos",
            "/review",
            "/subagent-model",
            "/subagent investigate",
            "/fork prompt",
            "/split",
            "/btw question",
            "/transfer",
            "/workspace",
            "/workspace split",
            "/catchup",
            "/catchup next",
            "/back",
            "/zstatus",
            "/z",
            "/active",
        ] {
            assert!(!super::ssh_unsupported_command(command), "{command}");
        }
    }

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
