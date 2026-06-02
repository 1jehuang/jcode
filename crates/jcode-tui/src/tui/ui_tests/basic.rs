use super::*;
use crate::tui::ui::{
    FlickerFrameSample, FramePerfStats, SlowFrameSample, clear_flicker_frame_history_for_tests,
    clear_slow_frame_history_for_tests, copy_badge_reserved_width, record_flicker_frame_sample,
    record_slow_frame_sample, reserve_copy_badge_margins, truncate_copy_badge_line_to_width,
};

include!("basic/frame_flicker.rs");
include!("basic/interaction_links.rs");
include!("basic/body_cache.rs");
include!("basic/input_layout.rs");
