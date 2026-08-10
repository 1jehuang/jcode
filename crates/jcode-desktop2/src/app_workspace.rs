//! App-side clock and coordinate bridge for the niri-style workspace.

use crate::{App, workspace};

impl App {
    /// Start a horizontal camera slide toward the session the strip just
    /// focused.
    pub(crate) fn begin_workspace_transition(&mut self, direction: workspace::Direction) {
        self.model.workspace.begin(direction);
        self.workspace_frame = Some(std::time::Instant::now());
        self.request_redraw();
    }

    /// Start a vertical row slide. `prev_row` and `prev_focused` describe the
    /// group being left, captured *before* the strip moved, so the departing
    /// workspace exits exactly as it stood.
    pub(crate) fn begin_row_transition(
        &mut self,
        direction: workspace::Direction,
        prev_row: Vec<String>,
        prev_focused: usize,
    ) {
        self.model
            .workspace
            .begin_row_change(direction, prev_row, prev_focused);
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

    /// Size used to lay out the focused model. The GPU surface stays
    /// full-window; only the child scene uses this narrower native-scale page.
    pub(crate) fn workspace_render_size(&self, viewport: (u32, u32)) -> (u32, u32) {
        if !self.workspace_active() {
            return viewport;
        }
        let row_len = self
            .model
            .strips
            .focused_strip()
            .map(|group| group.panels.len())
            .unwrap_or(1);
        let scale = self.effective_scale();
        let inset = (workspace::VERTICAL_INSET * scale * 2.0).round() as u32;
        (
            self.model.workspace.column_width(viewport.0, row_len),
            viewport.1.saturating_sub(inset).max(1),
        )
    }

    /// Convert the window-space pointer to focused-column coordinates.
    /// Inactive columns therefore never reach any of the existing hit-testing
    /// paths.
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
        let origin = workspace::placement(
            &self.model.strips,
            &self.model.workspace,
            self.model.session_id.as_deref(),
            (f64::from(size.0), f64::from(size.1)),
            workspace::GAP * scale,
        )
        .into_iter()
        .find(|column| column.focused)
        .map_or((0.0, 0.0), |column| {
            (
                column.x / scale,
                (column.y + workspace::VERTICAL_INSET * scale) / scale,
            )
        });
        (self.pointer.0 - origin.0, self.pointer.1 - origin.1)
    }

    /// The focused group's session ids and focused position, for capturing a
    /// row about to be left by a vertical slide.
    pub(crate) fn focused_row_snapshot(&self) -> (Vec<String>, usize) {
        let ids = self
            .model
            .strips
            .focused_strip()
            .map(|group| {
                group
                    .panels
                    .iter()
                    .map(|entry| entry.session_id.clone())
                    .collect()
            })
            .unwrap_or_default();
        (ids, self.model.strips.panel_index())
    }

    /// Whether the workspace chrome (gutters, page rings, camera) is in play:
    /// the focused row has neighbors, or a slide is still running.
    pub(crate) fn workspace_active(&self) -> bool {
        if self.model.overview.is_visible() {
            return false;
        }
        if self.model.workspace.is_animating() {
            return true;
        }
        self.model
            .strips
            .focused_strip()
            .map(|group| group.panels.len() > 1)
            .unwrap_or(false)
    }
}
