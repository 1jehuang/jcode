/// Terminal/window-manager environment variables that identify *which*
/// terminal, multiplexer, or display a client is attached to.
///
/// The jcode server process is long-lived and captures these at *its* startup,
/// so once a client connects from a different terminal/tmux/zellij session the
/// server's copies are stale. Spawn and focus hooks executed by the server then
/// target the wrong terminal (see issue #405). To fix this, clients snapshot
/// these vars from their own environment and send them to the server, which
/// re-exports them to spawn/focus hooks so the hook places windows in the
/// terminal the user is actually looking at.
///
/// This intentionally covers terminal multiplexers (tmux, screen, zellij),
/// terminal emulators (kitty, wezterm, ghostty, iTerm, ...), and the display
/// server (X11 `DISPLAY`, Wayland `WAYLAND_DISPLAY`) so window placement and
/// routing all follow the connecting client.
pub const CLIENT_TERMINAL_ENV_VARS: &[&str] = &[
    // Terminal multiplexers
    "ZELLIJ",
    "ZELLIJ_SESSION_NAME",
    "ZELLIJ_PANE_ID",
    "TMUX",
    "TMUX_PANE",
    "STY",
    // herdr terminal multiplexer (https://herdr.dev), see issue #405
    "HERDR_ENV",
    "HERDR_SOCKET_PATH",
    "HERDR_PANE_ID",
    "HERDR_TAB_ID",
    "HERDR_WORKSPACE_ID",
    "HERDR_BIN_PATH",
    "HERDR_SESSION",
    "HERDR_AGENT",
    // Terminal emulators
    "TERM",
    "TERM_PROGRAM",
    "TERM_PROGRAM_VERSION",
    "COLORTERM",
    "KITTY_PID",
    "KITTY_WINDOW_ID",
    "KITTY_LISTEN_ON",
    "WEZTERM_PANE",
    "WEZTERM_EXECUTABLE",
    "WEZTERM_UNIX_SOCKET",
    "ALACRITTY_WINDOW_ID",
    "ALACRITTY_SOCKET",
    "GHOSTTY_RESOURCES_DIR",
    "GHOSTTY_BIN_DIR",
    "ITERM_SESSION_ID",
    "WINDOWID",
    "HANDTERM_SESSION",
    "HANDTERM_PID",
    "WT_SESSION",
    "WT_PROFILE_ID",
    // Display / window manager
    "DISPLAY",
    "WAYLAND_DISPLAY",
    // Agent orchestrators that name the pane's agent. `yaran peer send` and
    // `amq send` read these to say who is speaking, so a shared server must
    // not hand every client the agent of the pane that started it.
    "YARAN_SESSION_ID",
    "YARAN_AMQ_HANDLE",
    "DUX_SESSION_ID",
    "DUX_AMQ_HANDLE",
    "AM_ME",
];

#[cfg(test)]
mod tests {
    #[test]
    fn agent_identity_snapshot_replaces_native_and_alias_values_and_empty_clears_them() {
        let keys = [
            "YARAN_SESSION_ID",
            "YARAN_AMQ_HANDLE",
            "DUX_SESSION_ID",
            "DUX_AMQ_HANDLE",
            "AM_ME",
        ];
        let snapshot = keys.map(|key| (key.to_owned(), "client-agent".to_owned()));
        let mut command = std::process::Command::new("unused");
        for key in keys {
            command
                .env(key, "daemon-agent")
                .env(format!("JCODE_CLIENT_{key}"), "daemon-agent");
        }
        crate::apply_client_terminal_env(&mut command, &snapshot);
        for key in keys
            .into_iter()
            .flat_map(|key| [key.to_owned(), format!("JCODE_CLIENT_{key}")])
        {
            let value = command
                .get_envs()
                .find(|(name, _)| *name == std::ffi::OsStr::new(&key))
                .unwrap()
                .1;
            assert_eq!(value, Some(std::ffi::OsStr::new("client-agent")), "{key}");
        }
        crate::apply_client_terminal_env(&mut command, &[]);
        for key in keys
            .into_iter()
            .flat_map(|key| [key.to_owned(), format!("JCODE_CLIENT_{key}")])
        {
            let value = command
                .get_envs()
                .find(|(name, _)| *name == std::ffi::OsStr::new(&key))
                .unwrap()
                .1;
            assert_eq!(value, None, "{key}");
        }
    }
}
