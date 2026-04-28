use std::time::{Duration, Instant};

pub(crate) const VIEWPORT_ANIMATION_DURATION: Duration = Duration::from_millis(150);
pub(crate) const FOCUS_PULSE_DURATION: Duration = Duration::from_millis(180);
const VIEWPORT_ANIMATION_EPSILON: f32 = 0.5;

#[derive(Clone, Copy)]
pub(crate) struct VisibleColumnLayout {
    pub(crate) visible_columns: u32,
    pub(crate) first_visible_column: i32,
}

#[derive(Clone, Copy)]
pub(crate) struct WorkspaceRenderLayout {
    pub(crate) visible: VisibleColumnLayout,
    pub(crate) column_width: f32,
    pub(crate) scroll_offset: f32,
    pub(crate) vertical_scroll_offset: f32,
}

#[derive(Default)]
pub(crate) struct AnimatedViewport {
    initialized: bool,
    start_column_width: f32,
    start_scroll_offset: f32,
    start_vertical_scroll_offset: f32,
    current_column_width: f32,
    current_scroll_offset: f32,
    current_vertical_scroll_offset: f32,
    target_column_width: f32,
    target_scroll_offset: f32,
    target_vertical_scroll_offset: f32,
    started_at: Option<Instant>,
}

impl AnimatedViewport {
    pub(crate) fn frame(
        &mut self,
        target: WorkspaceRenderLayout,
        now: Instant,
    ) -> WorkspaceRenderLayout {
        if !self.initialized {
            self.initialized = true;
            self.current_column_width = target.column_width;
            self.current_scroll_offset = target.scroll_offset;
            self.current_vertical_scroll_offset = target.vertical_scroll_offset;
            self.target_column_width = target.column_width;
            self.target_scroll_offset = target.scroll_offset;
            self.target_vertical_scroll_offset = target.vertical_scroll_offset;
            return target;
        }

        if has_layout_target_changed(self.target_column_width, target.column_width)
            || has_layout_target_changed(self.target_scroll_offset, target.scroll_offset)
            || has_layout_target_changed(
                self.target_vertical_scroll_offset,
                target.vertical_scroll_offset,
            )
        {
            self.start_column_width = self.current_column_width;
            self.start_scroll_offset = self.current_scroll_offset;
            self.start_vertical_scroll_offset = self.current_vertical_scroll_offset;
            self.target_column_width = target.column_width;
            self.target_scroll_offset = target.scroll_offset;
            self.target_vertical_scroll_offset = target.vertical_scroll_offset;
            self.started_at = Some(now);
        }

        if let Some(started_at) = self.started_at {
            let progress =
                (now - started_at).as_secs_f32() / VIEWPORT_ANIMATION_DURATION.as_secs_f32();
            let progress = progress.clamp(0.0, 1.0);
            let eased = ease_out_cubic(progress);
            self.current_column_width =
                lerp(self.start_column_width, self.target_column_width, eased);
            self.current_scroll_offset =
                lerp(self.start_scroll_offset, self.target_scroll_offset, eased);
            self.current_vertical_scroll_offset = lerp(
                self.start_vertical_scroll_offset,
                self.target_vertical_scroll_offset,
                eased,
            );

            if progress >= 1.0 {
                self.current_column_width = self.target_column_width;
                self.current_scroll_offset = self.target_scroll_offset;
                self.current_vertical_scroll_offset = self.target_vertical_scroll_offset;
                self.started_at = None;
            }
        }

        WorkspaceRenderLayout {
            visible: target.visible,
            column_width: self.current_column_width,
            scroll_offset: self.current_scroll_offset,
            vertical_scroll_offset: self.current_vertical_scroll_offset,
        }
    }

    pub(crate) fn is_animating(&self) -> bool {
        self.started_at.is_some()
    }
}

#[derive(Default)]
pub(crate) struct FocusPulse {
    last_focused_id: Option<u64>,
    started_at: Option<Instant>,
}

impl FocusPulse {
    pub(crate) fn frame(&mut self, focused_id: u64, now: Instant) -> f32 {
        match self.last_focused_id {
            None => {
                self.last_focused_id = Some(focused_id);
                return 0.0;
            }
            Some(last_focused_id) if last_focused_id != focused_id => {
                self.last_focused_id = Some(focused_id);
                self.started_at = Some(now);
            }
            Some(_) => {}
        }

        let Some(started_at) = self.started_at else {
            return 0.0;
        };
        let progress =
            ((now - started_at).as_secs_f32() / FOCUS_PULSE_DURATION.as_secs_f32()).clamp(0.0, 1.0);
        if progress >= 1.0 {
            self.started_at = None;
            return 0.0;
        }

        1.0 - ease_out_cubic(progress)
    }

    pub(crate) fn is_animating(&self) -> bool {
        self.started_at.is_some()
    }
}

fn has_layout_target_changed(previous: f32, next: f32) -> bool {
    (previous - next).abs() > VIEWPORT_ANIMATION_EPSILON
}

pub(crate) fn ease_out_cubic(progress: f32) -> f32 {
    1.0 - (1.0 - progress).powi(3)
}

pub(crate) fn lerp(start: f32, end: f32, progress: f32) -> f32 {
    start + (end - start) * progress
}
