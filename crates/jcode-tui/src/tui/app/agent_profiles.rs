//! Server-authoritative additive agent profile controls.
use super::{App, DisplayMessage};
use crate::tui::backend::RemoteConnection;

/// Use token boundaries so `/agents` keeps its existing role-model picker.
pub(super) fn agent_profile_args(input: &str) -> Option<Vec<String>> {
    let mut words = input.split_whitespace();
    (words.next()? == "/agent").then(|| words.map(str::to_string).collect())
}

pub(super) async fn send_agent_profile_command(
    app: &mut App,
    remote: &mut RemoteConnection,
    args: Vec<String>,
) {
    match remote.runtime_command("agent", args).await {
        Ok(id) => {
            app.agent_profile_request_ids.insert(id);
        }
        Err(error) => app.push_display_message(DisplayMessage::error(format!(
            "Failed to reach the jcode server for /agent: {error}"
        ))),
    }
}
