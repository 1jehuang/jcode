//! First-frame header hints for remote clients.
//!
//! A remote client paints its first frame long before the server's History
//! event arrives. Header rows that only appear once History lands (the `mcp:`
//! line, the self-dev badge) used to pop in ~100ms later and shove the whole
//! layout down a row or two. We seed them from facts the client already knows
//! (it will request self-dev) or saw last time it talked to this server (its
//! MCP inventory), and History overwrites them with the authoritative values.

use super::App;
use serde::{Deserialize, Serialize};

const HINT_FILE: &str = "remote_header_hint.json";

#[derive(Serialize, Deserialize)]
struct RemoteHeaderHint {
    /// Socket path of the server these facts came from.
    origin: String,
    mcp_servers: Vec<(String, usize)>,
}

fn hint_path() -> Option<std::path::PathBuf> {
    crate::storage::jcode_dir()
        .ok()
        .map(|dir| dir.join(HINT_FILE))
}

fn origin() -> String {
    crate::server::socket_path().to_string_lossy().into_owned()
}

impl App {
    /// Seed header state before the first frame so it matches the layout the
    /// History event will settle on.
    pub(super) fn apply_remote_header_hint(&mut self) {
        if crate::tui::is_ssh_remote() {
            return;
        }
        // The subscribe request carries `selfdev` exactly when this resolves
        // true, and the server marks such sessions canary.
        if self.remote_is_canary.is_none() && crate::tui::subscribe_metadata(None).1 == Some(true) {
            self.remote_is_canary = Some(true);
        }
        if self.mcp_server_names.is_empty()
            && let Some(path) = hint_path()
            && let Ok(hint) = crate::storage::read_json::<RemoteHeaderHint>(&path)
            && hint.origin == origin()
        {
            self.mcp_server_names = hint.mcp_servers;
        }
    }

    /// Remember the header facts History just delivered, for the next launch.
    pub(super) fn persist_remote_header_hint(&self) {
        if crate::tui::is_ssh_remote() {
            return;
        }
        let Some(path) = hint_path() else {
            return;
        };
        let hint = RemoteHeaderHint {
            origin: origin(),
            mcp_servers: self.mcp_server_names.clone(),
        };
        if let Err(error) = crate::storage::write_json_fast(&path, &hint) {
            crate::logging::warn(&format!("Failed to persist remote header hint: {error}"));
        }
    }
}
