//! Streaming activity cue motion state for single_session_render.

use super::super::*;

pub(crate) const STREAMING_ACTIVITY_CUE_ENTRY_DURATION: Duration = Duration::from_millis(145);
pub(crate) const STREAMING_ACTIVITY_CUE_EXIT_DURATION: Duration = Duration::from_millis(130);
pub(crate) const STREAMING_ACTIVITY_CUE_ENTRY_OFFSET_PIXELS: f32 = 7.0;
pub(crate) const STREAMING_ACTIVITY_CUE_ENTRY_SCALE: f32 = 0.94;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct StreamingActivityCueVisual {
    pub(crate) opacity: f32,
    pub(crate) y_offset_pixels: f32,
    pub(crate) scale: f32,
}

impl StreamingActivityCueVisual {
    pub(crate) fn settled() -> Self {
        Self {
            opacity: 1.0,
            y_offset_pixels: 0.0,
            scale: 1.0,
        }
    }

    pub(crate) fn entry(progress: f32) -> Self {
        let eased = ease_out_cubic_local(progress);
        Self {
            opacity: eased,
            y_offset_pixels: STREAMING_ACTIVITY_CUE_ENTRY_OFFSET_PIXELS * (1.0 - eased),
            scale: lerp_f32(STREAMING_ACTIVITY_CUE_ENTRY_SCALE, 1.0, eased),
        }
    }

    pub(crate) fn exit(progress: f32) -> Self {
        let eased = ease_out_cubic_local(progress);
        Self {
            opacity: 1.0 - eased,
            y_offset_pixels: -STREAMING_ACTIVITY_CUE_ENTRY_OFFSET_PIXELS * 0.55 * eased,
            scale: lerp_f32(1.0, 0.975, eased),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct StreamingActivityCueMotionFrame {
    current: Option<StreamingActivityCueVisual>,
    exiting: Option<StreamingActivityCueVisual>,
    active: bool,
    cache_key: u64,
}

impl StreamingActivityCueMotionFrame {
    pub(crate) fn current(&self) -> Option<StreamingActivityCueVisual> {
        self.current
    }

    pub(crate) fn exiting(&self) -> Option<StreamingActivityCueVisual> {
        self.exiting
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active
    }

    pub(crate) fn cache_key(&self) -> u64 {
        self.cache_key
    }
}

#[derive(Default)]
pub(crate) struct StreamingActivityCueMotionRegistry {
    initialized: bool,
    visible: bool,
    entered_at: Option<Instant>,
    exiting_at: Option<Instant>,
}

impl StreamingActivityCueMotionRegistry {
    pub(crate) fn frame(
        &mut self,
        app: &SingleSessionApp,
        now: Instant,
    ) -> StreamingActivityCueMotionFrame {
        self.frame_for_visible(app.has_activity_indicator(), now)
    }

    pub(crate) fn frame_for_visible(
        &mut self,
        visible: bool,
        now: Instant,
    ) -> StreamingActivityCueMotionFrame {
        let reduced_motion = crate::animation::desktop_reduced_motion_enabled();
        if !self.initialized {
            self.initialized = true;
            self.visible = visible;
            self.entered_at = None;
            self.exiting_at = None;
        } else if self.visible != visible {
            if reduced_motion {
                self.entered_at = None;
                self.exiting_at = None;
            } else if visible {
                self.entered_at = Some(now);
                self.exiting_at = None;
            } else {
                self.exiting_at = Some(now);
                self.entered_at = None;
            }
            self.visible = visible;
        }

        if reduced_motion {
            self.entered_at = None;
            self.exiting_at = None;
        }

        let mut active = false;
        let current = if visible {
            let visual = if let Some(started_at) = self.entered_at {
                let (progress, running) = timed_animation_progress(
                    started_at,
                    now,
                    STREAMING_ACTIVITY_CUE_ENTRY_DURATION,
                );
                active |= running;
                if running {
                    StreamingActivityCueVisual::entry(progress)
                } else {
                    self.entered_at = None;
                    StreamingActivityCueVisual::settled()
                }
            } else {
                StreamingActivityCueVisual::settled()
            };
            Some(visual)
        } else {
            None
        };

        let exiting = if !visible {
            self.exiting_at.and_then(|started_at| {
                let (progress, running) =
                    timed_animation_progress(started_at, now, STREAMING_ACTIVITY_CUE_EXIT_DURATION);
                if running {
                    active = true;
                    Some(StreamingActivityCueVisual::exit(progress))
                } else {
                    self.exiting_at = None;
                    None
                }
            })
        } else {
            None
        };

        StreamingActivityCueMotionFrame {
            current,
            exiting,
            active,
            cache_key: streaming_activity_cue_motion_cache_key(current, exiting, active),
        }
    }

    pub(crate) fn clear(&mut self) {
        self.initialized = false;
        self.visible = false;
        self.entered_at = None;
        self.exiting_at = None;
    }
}

pub(crate) fn streaming_activity_cue_motion_cache_key(
    current: Option<StreamingActivityCueVisual>,
    exiting: Option<StreamingActivityCueVisual>,
    active: bool,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    active.hash(&mut hasher);
    current.is_some().hash(&mut hasher);
    if let Some(visual) = current {
        streaming_activity_cue_visual_hash(visual, &mut hasher);
    }
    exiting.is_some().hash(&mut hasher);
    if let Some(visual) = exiting {
        streaming_activity_cue_visual_hash(visual, &mut hasher);
    }
    hasher.finish()
}

pub(crate) fn streaming_activity_cue_visual_hash(
    visual: StreamingActivityCueVisual,
    hasher: &mut impl Hasher,
) {
    hash_f32(visual.opacity, hasher);
    hash_f32(visual.y_offset_pixels, hasher);
    hash_f32(visual.scale, hasher);
}
