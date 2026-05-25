use super::*;
use crate::tui::ui::{
    FlickerFrameSample, FramePerfStats, SlowFrameSample, clear_flicker_frame_history_for_tests,
    clear_slow_frame_history_for_tests, record_flicker_frame_sample, record_slow_frame_sample,
};

include!("basic/frame_flicker.rs");
include!("basic/interaction_links.rs");
include!("basic/body_cache.rs");
include!("basic/input_layout.rs");
