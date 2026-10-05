use super::*;

impl App {
    pub(super) fn restore_remote_startup_history(&mut self, session_id: &str) {
        let load_start = Instant::now();
        let Ok(mut session) = Session::load_for_remote_startup(session_id) else {
            return;
        };

        let render_start = Instant::now();
        // Narrow scope so render intermediates (rendered messages, display
        // message buffers) drop before we strip and retain the session.
        {
            let (rendered_messages, rendered_images) =
                crate::session::render_messages_and_images(&session);
            let display_messages =
                jcode_tui_messages::display_messages_from_rendered_messages(rendered_messages);
            self.replace_display_messages(display_messages);
            self.remote_side_pane_images = rendered_images;
            self.invalidate_side_pane_images_signature();
        }
        let render_ms = render_start.elapsed().as_millis();

        let image_ms = 0;
        self.set_side_panel_snapshot(
            crate::side_panel::snapshot_for_session(session_id).unwrap_or_default(),
        );
        self.remote_session_id = Some(session_id.to_string());
        session.strip_transcript_for_remote_client();
        // Strip clears transcript vectors but keeps capacity; free buffers.
        session.messages.shrink_to_fit();
        session.env_snapshots.shrink_to_fit();
        session.memory_injections.shrink_to_fit();
        session.replay_events.shrink_to_fit();
        self.session = session;
        // The full deserialized transcript (raw file + Session structs) was a
        // large transient; return the freed arena pages to the OS now instead
        // of waiting for the post-connect client_history_loaded release.
        crate::process_memory::release_retained_heap("remote_startup_history_stripped");
        self.autoreview_enabled = self
            .session
            .autoreview_enabled
            .unwrap_or(crate::config::config().autoreview.enabled);
        self.autojudge_enabled = self
            .session
            .autojudge_enabled
            .unwrap_or(crate::config::config().autojudge.enabled);
        if let Some(model) = self.session.model.clone() {
            self.update_context_limit_for_model(&model, None);
        }
        self.follow_chat_bottom();
        crate::logging::info(&format!(
            "Remote startup fast restore: session={}, display_messages={}, images={}, load={}ms, render={}ms, images_render={}ms, total={}ms",
            session_id,
            self.display_messages.len(),
            self.remote_side_pane_images.len(),
            load_start
                .elapsed()
                .as_millis()
                .saturating_sub(render_ms + image_ms),
            render_ms,
            image_ms,
            load_start.elapsed().as_millis()
        ));
    }
}
