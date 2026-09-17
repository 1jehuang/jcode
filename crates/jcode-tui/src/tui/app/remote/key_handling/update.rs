use super::*;
use std::cell::Cell;

pub(in crate::tui::app) fn handle_remote_char_input(app: &mut App, c: char) {
    if is_osc133_leak(c) {
        return;
    }
    input::handle_text_input(app, &c.to_string());
}

/// State for the OSC 133 sequence filter.
///
/// When a HERDR PTY delivers a partial or chunked OSC 133 sequence from
/// pwsh's `[Console]::Write` (or previously `Write-Host`), crossterm may
/// parse trailing bytes as literal key-press characters. This state machine
/// detects the start of such a sequence (`ESC]133;...`) and consumes all
/// subsequent characters until the terminator (`BEL` or `ESC\`) is seen,
/// preventing them from reaching the input buffer.
struct Osc133Filter {
    /// 0 = idle, 1 = saw ESC, 2 = inside `ESC]133;...`
    state: Cell<u8>,
}

impl Osc133Filter {
    const fn new() -> Self {
        Self {
            state: Cell::new(0),
        }
    }

    /// Returns `true` if `c` should be **consumed** (dropped) rather than
    /// forwarded to the input handler.
    fn filter(&self, c: char) -> bool {
        let byte = c as u32 as u8;
        match self.state.get() {
            0 => {
                // Idle - look for ESC.
                if byte == 0x1b {
                    self.state.set(1);
                }
                false
            }
            1 => {
                // Saw ESC - expect `]`.
                if byte == b']' {
                    self.state.set(2);
                } else {
                    // Not a CSI/OSC; reset and don't consume the char.
                    self.state.set(0);
                }
                false
            }
            _ => {
                // Inside a potential `ESC]133;...` sequence. Consume until BEL
                // (0x07) or ESC-terminated ST (`ESC\`).
                if byte == 0x07 || byte == 0x1b {
                    self.state.set(0);
                }
                true
            }
        }
    }
}

thread_local! {
    static OSC133_FILTER: Osc133Filter = Osc133Filter::new();
}

/// Returns `true` if `c` is part of a leaked OSC 133 sequence and should be
/// dropped. Thread-local state tracks the multi-character sequence across
/// successive calls.
fn is_osc133_leak(c: char) -> bool {
    OSC133_FILTER.with(|f| f.filter(c))
}


pub(in crate::tui::app) async fn send_interleave_now(
    app: &mut App,
    content: String,
    images: Vec<(String, String)>,
    remote: &mut RemoteConnection,
) {
    if content.trim().is_empty() {
        return;
    }
    let msg_clone = content.clone();
    match remote.soft_interrupt(content, images, false).await {
        Err(e) => {
            app.push_display_message(DisplayMessage::error(format!(
                "Failed to send interleave: {}",
                e
            )));
        }
        Ok(request_id) => {
            app.track_pending_soft_interrupt(request_id, msg_clone);
            app.set_status_notice("⏭ Interleave sent");
        }
    }
}

pub(in crate::tui::app) async fn handle_remote_update_command(
    app: &mut App,
    remote: &mut RemoteConnection,
) -> Result<()> {
    if crate::tui::is_ssh_remote() {
        app.set_status_notice("Update the client and SSH server separately, then reconnect");
        return Ok(());
    }
    reload_stale_remote_server_before_update(app, remote).await?;

    let session_id = app
        .remote_session_id
        .clone()
        .unwrap_or_else(|| crate::id::new_id("ses"));
    app.start_background_client_update(session_id);
    Ok(())
}

pub(in crate::tui::app) async fn reload_stale_remote_server_before_update(
    app: &mut App,
    remote: &mut RemoteConnection,
) -> Result<bool> {
    if app.remote_server_has_update != Some(true) {
        return Ok(false);
    }

    app.append_reload_message("Reloading stale server before checking for client updates...");
    remote.reload().await?;
    Ok(true)
}
