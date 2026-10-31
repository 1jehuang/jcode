use super::App;

impl App {
    pub(super) fn refresh_justrust_status(&mut self) -> bool {
        let session = self.active_client_session_id().map(str::to_owned);
        // The regular local-daemon client reads the same run files as Waybar.
        // SSH clients cannot attribute local laptop telemetry to the remote host.
        self.justrust_status.refresh(
            session.as_deref(),
            self.session.working_dir.as_deref(),
            !crate::tui::is_ssh_remote(),
        )
    }
}
