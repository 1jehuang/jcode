use std::collections::HashMap;
use std::time::{Duration, Instant};

pub(crate) const VIEWPORT_ANIMATION_DURATION: Duration = Duration::from_millis(150);
pub(crate) const FOCUS_PULSE_DURATION: Duration = Duration::from_millis(180);
pub(crate) const SURFACE_TRANSITION_DURATION: Duration = Duration::from_millis(180);
pub(crate) const APP_MODE_TRANSITION_DURATION: Duration = Duration::from_millis(180);
pub(crate) const STATUS_COLOR_TRANSITION_DURATION: Duration = Duration::from_millis(140);
pub(crate) const STATUS_TEXT_TRANSITION_DURATION: Duration = Duration::from_millis(150);
pub(crate) const DESKTOP_REDUCED_MOTION_ENV: &str = "JCODE_DESKTOP_REDUCED_MOTION";
const VIEWPORT_ANIMATION_EPSILON: f32 = 0.5;
const SURFACE_TRANSITION_EPSILON: f32 = 0.5;
const SURFACE_ENTRY_OFFSET_PIXELS: f32 = 24.0;
const SURFACE_EXIT_OFFSET_PIXELS: f32 = 18.0;
const SURFACE_ENTRY_SCALE: f32 = 0.965;
const STATUS_TEXT_TRANSITION_OFFSET_PIXELS: f32 = 7.0;

pub(crate) fn desktop_reduced_motion_enabled_for_env_value(
    value: Option<std::ffi::OsString>,
) -> bool {
    value.is_some_and(crate::desktop_config::env_flag_enabled)
}

pub(crate) fn desktop_reduced_motion_enabled() -> bool {
    #[cfg(test)]
    if let Some(enabled) = DESKTOP_REDUCED_MOTION_TEST_OVERRIDE.with(|override_| override_.get()) {
        return enabled;
    }

    desktop_reduced_motion_enabled_for_env_value(std::env::var_os(DESKTOP_REDUCED_MOTION_ENV))
}

#[cfg(test)]
thread_local! {
    static DESKTOP_REDUCED_MOTION_TEST_OVERRIDE: std::cell::Cell<Option<bool>> = const {
        std::cell::Cell::new(None)
    };
}

#[cfg(test)]
pub(crate) struct DesktopReducedMotionEnvGuard {
    previous: Option<bool>,
}

#[cfg(test)]
impl DesktopReducedMotionEnvGuard {
    pub(crate) fn set(enabled: bool) -> Self {
        let previous = DESKTOP_REDUCED_MOTION_TEST_OVERRIDE.with(|override_| {
            let previous = override_.get();
            override_.set(Some(enabled));
            previous
        });
        Self { previous }
    }
}

#[cfg(test)]
impl Drop for DesktopReducedMotionEnvGuard {
    fn drop(&mut self) {
        DESKTOP_REDUCED_MOTION_TEST_OVERRIDE.with(|override_| override_.set(self.previous));
    }
}

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

        if desktop_reduced_motion_enabled() {
            self.start_column_width = target.column_width;
            self.start_scroll_offset = target.scroll_offset;
            self.start_vertical_scroll_offset = target.vertical_scroll_offset;
            self.current_column_width = target.column_width;
            self.current_scroll_offset = target.scroll_offset;
            self.current_vertical_scroll_offset = target.vertical_scroll_offset;
            self.target_column_width = target.column_width;
            self.target_scroll_offset = target.scroll_offset;
            self.target_vertical_scroll_offset = target.vertical_scroll_offset;
            self.started_at = None;
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct AnimatedRect {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) width: f32,
    pub(crate) height: f32,
}

impl AnimatedRect {
    fn shifted(self, dx: f32, dy: f32) -> Self {
        Self {
            x: self.x + dx,
            y: self.y + dy,
            ..self
        }
    }

    pub(crate) fn scaled_about_center(self, scale: f32) -> Self {
        let scale = scale.max(0.01);
        let width = self.width * scale;
        let height = self.height * scale;
        Self {
            x: self.x + (self.width - width) * 0.5,
            y: self.y + (self.height - height) * 0.5,
            width,
            height,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SurfaceVisualTarget {
    pub(crate) id: u64,
    pub(crate) rect: AnimatedRect,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SurfaceVisualFrame {
    pub(crate) id: u64,
    pub(crate) rect: AnimatedRect,
    pub(crate) opacity: f32,
    pub(crate) exiting: bool,
    scale: f32,
}

impl SurfaceVisualFrame {
    pub(crate) fn visual_rect(self) -> AnimatedRect {
        self.rect.scaled_about_center(self.scale)
    }
}

#[derive(Clone, Copy, Debug)]
struct SurfaceAnimationValues {
    rect: AnimatedRect,
    opacity: f32,
    scale: f32,
}

impl SurfaceAnimationValues {
    fn from_target(target: SurfaceVisualTarget) -> Self {
        Self {
            rect: target.rect,
            opacity: 1.0,
            scale: 1.0,
        }
    }

    fn entering_from(target: SurfaceVisualTarget) -> Self {
        Self {
            rect: target.rect.shifted(0.0, SURFACE_ENTRY_OFFSET_PIXELS),
            opacity: 0.0,
            scale: SURFACE_ENTRY_SCALE,
        }
    }

    fn exiting_from(current: Self) -> Self {
        Self {
            rect: current.rect.shifted(0.0, -SURFACE_EXIT_OFFSET_PIXELS),
            opacity: 0.0,
            scale: SURFACE_ENTRY_SCALE,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct SurfaceAnimationState {
    start: SurfaceAnimationValues,
    current: SurfaceAnimationValues,
    target: SurfaceAnimationValues,
    started_at: Option<Instant>,
    exiting: bool,
    last_seen_generation: u64,
}

#[derive(Default)]
pub(crate) struct SurfaceTransitionAnimator {
    initialized: bool,
    generation: u64,
    states: HashMap<u64, SurfaceAnimationState>,
}

impl SurfaceTransitionAnimator {
    pub(crate) fn frame(
        &mut self,
        targets: impl IntoIterator<Item = SurfaceVisualTarget>,
        now: Instant,
    ) -> Vec<SurfaceVisualFrame> {
        let targets = targets.into_iter().collect::<Vec<_>>();

        if desktop_reduced_motion_enabled() {
            self.initialized = true;
            self.generation = self.generation.wrapping_add(1).max(1);
            self.states.clear();
            return targets
                .into_iter()
                .map(|target| SurfaceVisualFrame {
                    id: target.id,
                    rect: target.rect,
                    opacity: 1.0,
                    exiting: false,
                    scale: 1.0,
                })
                .collect();
        }

        self.generation = self.generation.wrapping_add(1).max(1);
        let generation = self.generation;
        let animate_new_surfaces = self.initialized;
        self.initialized = true;

        let mut frames = Vec::new();
        for target in targets {
            let target_values = SurfaceAnimationValues::from_target(target);
            let state = self.states.entry(target.id).or_insert_with(|| {
                let start = if animate_new_surfaces {
                    SurfaceAnimationValues::entering_from(target)
                } else {
                    target_values
                };
                SurfaceAnimationState {
                    start,
                    current: start,
                    target: target_values,
                    started_at: animate_new_surfaces.then_some(now),
                    exiting: false,
                    last_seen_generation: generation,
                }
            });

            state.last_seen_generation = generation;
            update_surface_animation_state(state, now);
            if state.exiting {
                state.start = state.current;
                state.target = target_values;
                state.started_at = Some(now);
                state.exiting = false;
            } else if surface_animation_target_changed(state.target, target_values) {
                state.start = state.current;
                state.target = target_values;
                state.started_at = Some(now);
            }

            frames.push(surface_visual_frame_from_state(target.id, state));
        }

        let mut exiting_frames = Vec::new();
        for (&id, state) in &mut self.states {
            if state.last_seen_generation == generation {
                continue;
            }

            update_surface_animation_state(state, now);
            if !state.exiting {
                state.start = state.current;
                state.target = SurfaceAnimationValues::exiting_from(state.current);
                state.started_at = Some(now);
                state.exiting = true;
            }

            if state.exiting && state.started_at.is_none() && state.current.opacity <= 0.001 {
                continue;
            }

            state.last_seen_generation = generation;
            exiting_frames.push(surface_visual_frame_from_state(id, state));
        }

        exiting_frames.sort_by_key(|frame| frame.id);
        frames.extend(exiting_frames);

        self.states
            .retain(|_, state| state.last_seen_generation == generation);
        frames
    }

    pub(crate) fn clear(&mut self) {
        self.initialized = false;
        self.generation = 0;
        self.states.clear();
    }

    pub(crate) fn is_animating(&self) -> bool {
        self.states.values().any(|state| state.started_at.is_some())
    }
}

fn surface_visual_frame_from_state(id: u64, state: &SurfaceAnimationState) -> SurfaceVisualFrame {
    SurfaceVisualFrame {
        id,
        rect: state.current.rect,
        opacity: state.current.opacity,
        exiting: state.exiting,
        scale: state.current.scale,
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ColorTransition {
    initialized: bool,
    start: [f32; 4],
    current: [f32; 4],
    target: [f32; 4],
    started_at: Option<Instant>,
    duration: Duration,
}

impl Default for ColorTransition {
    fn default() -> Self {
        Self::new(STATUS_COLOR_TRANSITION_DURATION)
    }
}

impl ColorTransition {
    pub(crate) fn new(duration: Duration) -> Self {
        Self {
            initialized: false,
            start: [0.0; 4],
            current: [0.0; 4],
            target: [0.0; 4],
            started_at: None,
            duration,
        }
    }

    pub(crate) fn frame(&mut self, target: [f32; 4], now: Instant) -> [f32; 4] {
        if !self.initialized {
            self.initialized = true;
            self.start = target;
            self.current = target;
            self.target = target;
            return target;
        }

        if desktop_reduced_motion_enabled() {
            self.start = target;
            self.current = target;
            self.target = target;
            self.started_at = None;
            return target;
        }

        if color_target_changed(self.target, target) {
            self.start = self.current;
            self.target = target;
            self.started_at = Some(now);
        }

        let Some(started_at) = self.started_at else {
            return self.current;
        };
        let progress = (now.saturating_duration_since(started_at).as_secs_f32()
            / self.duration.as_secs_f32())
        .clamp(0.0, 1.0);
        let eased = ease_out_cubic(progress);
        for index in 0..self.current.len() {
            self.current[index] = lerp(self.start[index], self.target[index], eased);
        }
        if progress >= 1.0 {
            self.current = self.target;
            self.started_at = None;
        }
        self.current
    }

    pub(crate) fn clear(&mut self) {
        self.initialized = false;
        self.started_at = None;
    }

    pub(crate) fn is_animating(&self) -> bool {
        self.started_at.is_some()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StatusTextVisualFrame {
    pub(crate) text: String,
    pub(crate) opacity: f32,
    pub(crate) y_offset_pixels: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StatusTextTransitionFrame {
    pub(crate) current: StatusTextVisualFrame,
    pub(crate) previous: Option<StatusTextVisualFrame>,
    active: bool,
}

impl StatusTextTransitionFrame {
    pub(crate) fn settled(text: String) -> Self {
        Self {
            current: StatusTextVisualFrame {
                text,
                opacity: 1.0,
                y_offset_pixels: 0.0,
            },
            previous: None,
            active: false,
        }
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active
    }
}

#[derive(Default)]
pub(crate) struct StatusTextTransition {
    initialized: bool,
    current_text: String,
    previous_text: Option<String>,
    started_at: Option<Instant>,
}

impl StatusTextTransition {
    pub(crate) fn frame(&mut self, target: String, now: Instant) -> StatusTextTransitionFrame {
        if !self.initialized {
            self.initialized = true;
            self.current_text = target;
            self.previous_text = None;
            self.started_at = None;
            return StatusTextTransitionFrame::settled(self.current_text.clone());
        }

        if desktop_reduced_motion_enabled() {
            self.current_text = target;
            self.previous_text = None;
            self.started_at = None;
            return StatusTextTransitionFrame::settled(self.current_text.clone());
        }

        if self.current_text != target {
            self.previous_text = Some(self.current_text.clone());
            self.current_text = target;
            self.started_at = Some(now);
        }

        let Some(started_at) = self.started_at else {
            return StatusTextTransitionFrame::settled(self.current_text.clone());
        };
        let progress = (now.saturating_duration_since(started_at).as_secs_f32()
            / STATUS_TEXT_TRANSITION_DURATION.as_secs_f32())
        .clamp(0.0, 1.0);
        let eased = ease_out_cubic(progress);
        if progress >= 1.0 {
            self.previous_text = None;
            self.started_at = None;
            return StatusTextTransitionFrame::settled(self.current_text.clone());
        }

        StatusTextTransitionFrame {
            current: StatusTextVisualFrame {
                text: self.current_text.clone(),
                opacity: eased,
                y_offset_pixels: STATUS_TEXT_TRANSITION_OFFSET_PIXELS * (1.0 - eased),
            },
            previous: self
                .previous_text
                .as_ref()
                .map(|text| StatusTextVisualFrame {
                    text: text.clone(),
                    opacity: 1.0 - eased,
                    y_offset_pixels: -STATUS_TEXT_TRANSITION_OFFSET_PIXELS * eased,
                }),
            active: true,
        }
    }

    pub(crate) fn clear(&mut self) {
        self.initialized = false;
        self.current_text.clear();
        self.previous_text = None;
        self.started_at = None;
    }
}

#[derive(Default)]
pub(crate) struct FocusPulse {
    last_focused_id: Option<u64>,
    started_at: Option<Instant>,
}

impl FocusPulse {
    pub(crate) fn frame(&mut self, focused_id: u64, now: Instant) -> f32 {
        if desktop_reduced_motion_enabled() {
            self.last_focused_id = Some(focused_id);
            self.started_at = None;
            return 0.0;
        }

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

fn update_surface_animation_state(state: &mut SurfaceAnimationState, now: Instant) {
    let Some(started_at) = state.started_at else {
        return;
    };

    let progress = (now.saturating_duration_since(started_at).as_secs_f32()
        / SURFACE_TRANSITION_DURATION.as_secs_f32())
    .clamp(0.0, 1.0);
    let eased = ease_out_cubic(progress);
    state.current = interpolate_surface_values(state.start, state.target, eased);
    if progress >= 1.0 {
        state.current = state.target;
        state.started_at = None;
    }
}

fn interpolate_surface_values(
    start: SurfaceAnimationValues,
    end: SurfaceAnimationValues,
    progress: f32,
) -> SurfaceAnimationValues {
    SurfaceAnimationValues {
        rect: AnimatedRect {
            x: lerp(start.rect.x, end.rect.x, progress),
            y: lerp(start.rect.y, end.rect.y, progress),
            width: lerp(start.rect.width, end.rect.width, progress),
            height: lerp(start.rect.height, end.rect.height, progress),
        },
        opacity: lerp(start.opacity, end.opacity, progress),
        scale: lerp(start.scale, end.scale, progress),
    }
}

fn surface_animation_target_changed(
    previous: SurfaceAnimationValues,
    next: SurfaceAnimationValues,
) -> bool {
    rect_target_changed(previous.rect, next.rect)
        || (previous.opacity - next.opacity).abs() > 0.01
        || (previous.scale - next.scale).abs() > 0.001
}

fn rect_target_changed(previous: AnimatedRect, next: AnimatedRect) -> bool {
    has_surface_target_changed(previous.x, next.x)
        || has_surface_target_changed(previous.y, next.y)
        || has_surface_target_changed(previous.width, next.width)
        || has_surface_target_changed(previous.height, next.height)
}

fn has_surface_target_changed(previous: f32, next: f32) -> bool {
    (previous - next).abs() > SURFACE_TRANSITION_EPSILON
}

fn color_target_changed(previous: [f32; 4], next: [f32; 4]) -> bool {
    previous
        .iter()
        .zip(next.iter())
        .any(|(previous, next)| (previous - next).abs() > 0.001)
}

pub(crate) fn ease_out_cubic(progress: f32) -> f32 {
    1.0 - (1.0 - progress).powi(3)
}

pub(crate) fn lerp(start: f32, end: f32, progress: f32) -> f32 {
    start + (end - start) * progress
}
