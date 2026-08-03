//! App-side clock and coordinate bridge for the horizontal workspace.

use crate::{App, workspace};

impl App {
    pub(crate) fn begin_workspace_transition(&mut self, direction: workspace::Direction) {
        self.model.workspace.begin(direction);
        self.workspace_frame = Some(std::time::Instant::now());
        self.request_redraw();
    }

    pub(crate) fn tick_workspace(&mut self, now: std::time::Instant) {
        if !self.model.workspace.is_animating() {
            self.workspace_frame = None;
            return;
        }
        let dt = self
            .workspace_frame
            .map(|last| now.duration_since(last).as_secs_f32().min(0.1))
            .unwrap_or(crate::WORKSPACE_FRAME.as_secs_f32());
        self.workspace_frame = Some(now);
        if self.model.workspace.advance(dt) {
            self.request_redraw();
        }
        if !self.model.workspace.is_animating() {
            self.workspace_frame = None;
        }
    }

    /// Size used to lay out the focused model. The GPU surface stays full-window;
    /// only the child scene uses this narrower native-scale page.
    pub(crate) fn workspace_render_size(&self, viewport: (u32, u32)) -> (u32, u32) {
        if !self.workspace_active() {
            return viewport;
        }
        (
            workspace::column_width(viewport.0, self.model.strip.len()),
            viewport.1,
        )
    }

    /// Convert the window-space pointer to focused-column coordinates. Inactive
    /// columns therefore never reach any of the existing hit-testing paths.
    pub(crate) fn focused_pointer(&self) -> (f64, f64) {
        let Some(state) = self.state.as_ref() else {
            // Unit tests construct the app without a surface and set pointer
            // coordinates in frame space directly.
            return self.pointer;
        };
        if !self.workspace_active() {
            return self.pointer;
        }
        let size = state.size();
        let scale = self.effective_scale();
        let entries = self.model.strip.entries();
        let focused_index = entries
            .iter()
            .position(|entry| Some(entry.session_id.as_str()) == self.model.session_id.as_deref())
            .or_else(|| {
                let focused = self.model.strip.focused_session()?;
                entries.iter().position(|entry| entry.session_id == focused)
            })
            .unwrap_or(0);
        let width = workspace::column_width(size.0, entries.len());
        let origin = self
            .model
            .workspace
            .layout(
                entries.len(),
                focused_index,
                f64::from(size.0),
                f64::from(width),
                workspace::GAP * scale,
            )
            .into_iter()
            .find(|column| column.focused)
            .map_or(0.0, |column| column.x / scale);
        (self.pointer.0 - origin, self.pointer.1)
    }

    fn workspace_active(&self) -> bool {
        self.model.strip.len() > 1 && !self.model.overview.is_visible()
    }
}
