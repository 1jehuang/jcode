//! Live frame timing in the real window.
//!
//! Everything else that measures scrolling here runs headless, because that is
//! what makes the numbers deterministic: [`crate::scroll_bench`] replays
//! gestures through the real handlers and [`crate::scroll_profile`] attributes a
//! frame's cost to its phases, both without a compositor. Between them they say
//! the CPU side of a scroll frame is a fraction of a millisecond and lays out
//! nothing, which is the *whole* answer only if the CPU side is where the time
//! goes.
//!
//! It is not necessarily. A frame is also a Vello compute pass, a blit, a queue
//! submit, and a `present` that blocks on the compositor's release of a
//! swapchain image. None of that exists in a headless replay, and on an
//! integrated GPU it is routinely the larger half. So "scrolling feels laggy"
//! while the headless numbers look perfect points *here*, and there was no
//! instrument for it: this module is that instrument.
//!
//! It reports the three spans that can each independently ruin the feel, kept
//! separate because they have different causes and different fixes:
//!
//! - **build**: scene encoding on the CPU. What the headless tools measure, and
//!   the only span a code change to layout or painting can move.
//! - **gpu**: `render_to_texture` plus the blit and submit. Grows with window
//!   *area* and antialiasing, not with the conversation.
//! - **present**: acquiring and presenting the swapchain image. This is where
//!   vsync back-pressure lands, so a large number here with small numbers above
//!   means the frames are fine and the pacing is not.
//!
//! Plus **interval**, the wall time between presented frames, which is the only
//! span the hand actually feels: a 16ms interval on a 120Hz display is a
//! dropped frame no matter how fast the three spans above were.

use std::time::{Duration, Instant};

/// Frames accumulated before a line is printed. About a quarter second of a
/// 120Hz scroll, so a gesture produces a handful of lines rather than a wall.
const WINDOW: usize = 30;

/// One frame's spans, in microseconds.
#[derive(Clone, Copy, Default)]
struct Frame {
    build_us: u64,
    gpu_us: u64,
    present_us: u64,
    interval_us: u64,
}

/// Rolling frame timer for the live window. Silent unless switched on, so the
/// normal path pays one branch and no output.
#[derive(Default)]
pub struct FrameMeter {
    enabled: bool,
    frames: Vec<Frame>,
    last_present: Option<Instant>,
    /// Start of the span currently being timed, if any.
    mark: Option<Instant>,
    pending: Frame,
}

impl FrameMeter {
    /// Enabled by `JCODE_DESKTOP2_FRAME_METER=1`, rather than a flag, so the
    /// window it measures is the one the user was already running when they
    /// said it felt slow: a separate profiling build would not be.
    pub fn from_env() -> Self {
        let enabled = std::env::var("JCODE_DESKTOP2_FRAME_METER")
            .is_ok_and(|value| value != "0" && !value.is_empty());
        if enabled {
            eprintln!(
                "frame meter on: build = scene encoding, gpu = vello + blit + submit, \
                 present = swapchain acquire and present, interval = wall time between \
                 presented frames (8.3ms at 120Hz, 16.7ms at 60Hz)"
            );
        }
        Self {
            enabled,
            ..Self::default()
        }
    }

    /// Begin timing a span.
    pub fn start(&mut self) {
        if self.enabled {
            self.mark = Some(Instant::now());
        }
    }

    fn take(&mut self) -> u64 {
        self.mark
            .take()
            .map_or(0, |at| at.elapsed().as_micros() as u64)
    }

    pub fn end_build(&mut self) {
        if self.enabled {
            self.pending.build_us = self.take();
        }
    }

    pub fn end_gpu(&mut self) {
        if self.enabled {
            self.pending.gpu_us = self.take();
        }
    }

    /// Close the frame: records the present span and the interval since the
    /// previous presented frame, then prints once a window has accumulated.
    pub fn end_present(&mut self) {
        if !self.enabled {
            return;
        }
        self.pending.present_us = self.take();
        let now = Instant::now();
        self.pending.interval_us = self
            .last_present
            .map_or(0, |at| now.duration_since(at).as_micros() as u64);
        self.last_present = Some(now);
        let frame = std::mem::take(&mut self.pending);
        self.frames.push(frame);
        if self.frames.len() >= WINDOW {
            self.flush();
        }
    }

    /// A gap in the frames means the window went idle, so the next interval is
    /// a wait rather than a dropped frame and must not be reported as one.
    pub fn note_idle(&mut self) {
        if self.enabled {
            self.flush();
            self.last_present = None;
        }
    }

    fn flush(&mut self) {
        if self.frames.is_empty() {
            return;
        }
        let count = self.frames.len();
        let stat = |mut values: Vec<u64>| -> (u64, u64) {
            values.sort_unstable();
            (values[values.len() / 2], *values.last().unwrap_or(&0))
        };
        let (build, build_max) = stat(self.frames.iter().map(|f| f.build_us).collect());
        let (gpu, gpu_max) = stat(self.frames.iter().map(|f| f.gpu_us).collect());
        let (present, present_max) = stat(self.frames.iter().map(|f| f.present_us).collect());
        // The first frame after an idle has no predecessor, so it has no
        // interval and would otherwise read as an instant frame.
        let intervals: Vec<u64> = self
            .frames
            .iter()
            .map(|f| f.interval_us)
            .filter(|us| *us > 0)
            .collect();
        let (interval, interval_max) = if intervals.is_empty() {
            (0, 0)
        } else {
            stat(intervals)
        };
        let ms = |us: u64| us as f64 / 1000.0;
        eprintln!(
            "frames {count:>3}  build {:>5.2}/{:<5.2}  gpu {:>5.2}/{:<5.2}  \
             present {:>5.2}/{:<5.2}  interval {:>5.2}/{:<5.2} ms (median/worst)",
            ms(build),
            ms(build_max),
            ms(gpu),
            ms(gpu_max),
            ms(present),
            ms(present_max),
            ms(interval),
            ms(interval_max),
        );
        self.frames.clear();
    }
}

/// A 120Hz frame's budget. Exposed so callers can compare an interval against
/// it without restating the constant.
#[cfg_attr(not(test), allow(dead_code))]
pub const FRAME_120HZ: Duration = Duration::from_micros(8_333);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_meter_records_nothing() {
        let mut meter = FrameMeter::default();
        meter.start();
        meter.end_build();
        meter.end_present();
        assert!(meter.frames.is_empty(), "a disabled meter must be inert");
    }

    #[test]
    fn spans_are_recorded_per_frame() {
        let mut meter = FrameMeter {
            enabled: true,
            ..FrameMeter::default()
        };
        meter.start();
        meter.end_build();
        meter.start();
        meter.end_gpu();
        meter.start();
        meter.end_present();
        assert_eq!(meter.frames.len(), 1, "one closed frame expected");
        // The first presented frame has no predecessor, so it reports no
        // interval rather than a misleading zero-length one.
        assert_eq!(meter.frames[0].interval_us, 0);
    }

    /// The frames vector must not grow without bound during a long scroll.
    #[test]
    fn a_full_window_flushes() {
        let mut meter = FrameMeter {
            enabled: true,
            ..FrameMeter::default()
        };
        for _ in 0..WINDOW + 5 {
            meter.start();
            meter.end_build();
            meter.start();
            meter.end_present();
        }
        assert!(
            meter.frames.len() < WINDOW,
            "a full window should have flushed, held {}",
            meter.frames.len()
        );
    }
}
