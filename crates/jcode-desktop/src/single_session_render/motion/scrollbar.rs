//! Scrollbar motion + geometry for single_session_render.

use super::super::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SingleSessionScrollbarGeometry {
    pub(crate) thumb_y: f32,
    pub(crate) thumb_height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SingleSessionScrollbarVisual {
    pub(crate) thumb_y: f32,
    pub(crate) thumb_height: f32,
    pub(crate) opacity: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct SingleSessionScrollbarMotionFrame {
    visual: Option<SingleSessionScrollbarVisual>,
    active: bool,
    cache_key: u64,
}

impl SingleSessionScrollbarMotionFrame {
    pub(crate) fn visual(&self) -> Option<SingleSessionScrollbarVisual> {
        self.visual
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active
    }

    pub(crate) fn cache_key(&self) -> u64 {
        self.cache_key
    }
}

#[derive(Default)]
pub(crate) struct SingleSessionScrollbarMotionRegistry {
    initialized: bool,
    start_geometry: Option<SingleSessionScrollbarGeometry>,
    current_geometry: Option<SingleSessionScrollbarGeometry>,
    target_geometry: Option<SingleSessionScrollbarGeometry>,
    transition_started_at: Option<Instant>,
    last_activity_at: Option<Instant>,
}

impl SingleSessionScrollbarMotionRegistry {
    pub(crate) fn frame(
        &mut self,
        app: &SingleSessionApp,
        size: PhysicalSize<u32>,
        total_lines: usize,
        smooth_scroll_lines: f32,
        now: Instant,
    ) -> SingleSessionScrollbarMotionFrame {
        let metrics = single_session_body_scroll_metrics_for_total_lines(app, size, total_lines);
        self.frame_for_metrics(size, smooth_scroll_lines, metrics, now)
    }

    pub(crate) fn frame_for_metrics(
        &mut self,
        size: PhysicalSize<u32>,
        smooth_scroll_lines: f32,
        metrics: Option<SingleSessionBodyScrollMetrics>,
        now: Instant,
    ) -> SingleSessionScrollbarMotionFrame {
        let Some(metrics) = metrics else {
            self.clear();
            return SingleSessionScrollbarMotionFrame::default();
        };
        let target_geometry = single_session_scrollbar_geometry(size, smooth_scroll_lines, metrics);

        if !self.initialized {
            self.initialized = true;
            self.start_geometry = Some(target_geometry);
            self.current_geometry = Some(target_geometry);
            self.target_geometry = Some(target_geometry);
            self.transition_started_at = None;
            self.last_activity_at = Some(now);
        } else if self
            .target_geometry
            .is_none_or(|previous| scrollbar_geometry_changed(previous, target_geometry))
        {
            let start_geometry = self.current_geometry.unwrap_or(target_geometry);
            self.start_geometry = Some(start_geometry);
            self.current_geometry = Some(start_geometry);
            self.target_geometry = Some(target_geometry);
            self.transition_started_at = Some(now);
            self.last_activity_at = Some(now);
        }

        let transition_active = self.update_transition(now);
        let (opacity, fade_active) = self.opacity_for_frame(now);
        let active = transition_active || fade_active;
        let visual = (opacity > 0.001 || transition_active).then(|| {
            let geometry = self.current_geometry.unwrap_or(target_geometry);
            SingleSessionScrollbarVisual {
                thumb_y: geometry.thumb_y,
                thumb_height: geometry.thumb_height,
                opacity,
            }
        });
        SingleSessionScrollbarMotionFrame {
            visual,
            active,
            cache_key: scrollbar_motion_cache_key(visual, active),
        }
    }

    pub(crate) fn clear(&mut self) {
        self.initialized = false;
        self.start_geometry = None;
        self.current_geometry = None;
        self.target_geometry = None;
        self.transition_started_at = None;
        self.last_activity_at = None;
    }

    fn update_transition(&mut self, now: Instant) -> bool {
        let Some(started_at) = self.transition_started_at else {
            return false;
        };
        let Some(start) = self.start_geometry else {
            self.transition_started_at = None;
            return false;
        };
        let Some(target) = self.target_geometry else {
            self.transition_started_at = None;
            return false;
        };
        let (progress, running) = timed_animation_progress(
            started_at,
            now,
            SINGLE_SESSION_SCROLLBAR_THUMB_TRANSITION_DURATION,
        );
        let eased = ease_out_cubic_local(progress);
        self.current_geometry = Some(SingleSessionScrollbarGeometry {
            thumb_y: lerp_f32(start.thumb_y, target.thumb_y, eased),
            thumb_height: lerp_f32(start.thumb_height, target.thumb_height, eased),
        });
        if !running {
            self.current_geometry = Some(target);
            self.transition_started_at = None;
        }
        running
    }

    fn opacity_for_frame(&self, now: Instant) -> (f32, bool) {
        let Some(last_activity_at) = self.last_activity_at else {
            return (0.0, false);
        };
        let elapsed = now.saturating_duration_since(last_activity_at);
        if crate::animation::desktop_reduced_motion_enabled() {
            let opacity = if elapsed <= SINGLE_SESSION_SCROLLBAR_FADE_IDLE_DURATION {
                1.0
            } else {
                0.0
            };
            return (opacity, false);
        }
        if elapsed <= SINGLE_SESSION_SCROLLBAR_FADE_IDLE_DURATION {
            return (1.0, true);
        }
        let fade_elapsed = elapsed - SINGLE_SESSION_SCROLLBAR_FADE_IDLE_DURATION;
        let (progress, running) = timed_animation_progress(
            last_activity_at + SINGLE_SESSION_SCROLLBAR_FADE_IDLE_DURATION,
            last_activity_at + SINGLE_SESSION_SCROLLBAR_FADE_IDLE_DURATION + fade_elapsed,
            SINGLE_SESSION_SCROLLBAR_FADE_DURATION,
        );
        let opacity = 1.0 - ease_out_cubic_local(progress);
        (opacity, running)
    }
}

pub(crate) fn scrollbar_geometry_changed(
    previous: SingleSessionScrollbarGeometry,
    next: SingleSessionScrollbarGeometry,
) -> bool {
    (previous.thumb_y - next.thumb_y).abs() > 0.25
        || (previous.thumb_height - next.thumb_height).abs() > 0.25
}

pub(crate) fn single_session_scrollbar_geometry(
    size: PhysicalSize<u32>,
    smooth_scroll_lines: f32,
    metrics: SingleSessionBodyScrollMetrics,
) -> SingleSessionScrollbarGeometry {
    let track_top = single_session_scrollbar_track_top();
    let track_bottom = single_session_scrollbar_track_bottom(size);
    let track_height = (track_bottom - track_top).max(1.0);
    let thumb_height = (metrics.visible_lines as f32 / metrics.total_lines as f32 * track_height)
        .clamp(28.0, track_height);
    let travel = (track_height - thumb_height).max(0.0);
    let smooth_scroll_lines =
        (metrics.scroll_lines + smooth_scroll_lines).clamp(0.0, metrics.max_scroll_lines as f32);
    let scroll_fraction = smooth_scroll_lines / metrics.max_scroll_lines.max(1) as f32;
    let thumb_y = track_top + (1.0 - scroll_fraction.clamp(0.0, 1.0)) * travel;
    SingleSessionScrollbarGeometry {
        thumb_y,
        thumb_height,
    }
}

pub(crate) fn scrollbar_motion_cache_key(
    visual: Option<SingleSessionScrollbarVisual>,
    active: bool,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    active.hash(&mut hasher);
    visual.is_some().hash(&mut hasher);
    if let Some(visual) = visual {
        hash_f32(visual.thumb_y, &mut hasher);
        hash_f32(visual.thumb_height, &mut hasher);
        hash_f32(visual.opacity, &mut hasher);
    }
    hasher.finish()
}
