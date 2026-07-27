//! What the agent is doing right now, and the spinner that says it is alive.
//!
//! A turn can run for minutes before a single token of the answer arrives. An
//! app that shows nothing until the final response is indistinguishable from an
//! app that dropped the message, so the composer carries a live line instead:
//! an animated spinner, the current phase ("thinking", or a tool's `intent`),
//! and the elapsed time.
//!
//! Like `Caret`, the animation is derived from (started, now) rather than a
//! timer thread, so a frame stays a pure function of the model and state-space
//! nodes can pin a phase for deterministic capture.

use std::time::{Duration, Instant};

/// One spinner step. Slow enough to read as deliberate motion rather than
/// flicker, fast enough to look alive.
pub const SPINNER_FRAME: Duration = Duration::from_millis(90);

/// Dots in the spinner ring. Drawn as vector circles rather than a braille
/// glyph: this app's whole visual language is halftone dots, and a text
/// spinner rendered thin and grey at caption size, which is the one thing an
/// "is it alive?" indicator cannot be.
pub const SPINNER_DOTS: usize = 8;

/// The phase shown when nothing more specific has been reported yet.
pub const DEFAULT_LABEL: &str = "thinking";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Activity {
    /// When the current turn began, or `None` when idle.
    started: Option<Instant>,
    /// The current phase label, e.g. `thinking` or a tool's intent.
    label: Option<String>,
    /// Pins the spinner frame and elapsed time, ignoring the clock, so
    /// captures render identically every run.
    pinned: Option<(usize, Duration)>,
}

impl Activity {
    /// Begin a turn. Idempotent: a second start keeps the original clock so the
    /// elapsed time measures the turn, not the latest event.
    pub fn start(&mut self, now: Instant) {
        if self.started.is_none() {
            self.started = Some(now);
            self.label = None;
        }
    }

    /// Report the current phase. Starts the clock if a phase arrives before we
    /// noticed the turn (e.g. after attaching to an already-busy session).
    pub fn set_label(&mut self, label: impl Into<String>, now: Instant) {
        self.start(now);
        let label = label.into();
        let label = label.trim();
        if !label.is_empty() {
            self.label = Some(label.to_string());
        }
    }

    /// The turn ended.
    pub fn finish(&mut self) {
        self.started = None;
        self.label = None;
    }

    /// A deterministic activity for state-space nodes and tests.
    pub fn pinned(frame: usize, elapsed: Duration, label: Option<&str>) -> Self {
        Self {
            started: Some(Instant::now()),
            label: label.map(str::to_string),
            pinned: Some((frame, elapsed)),
        }
    }

    pub fn is_running(&self) -> bool {
        self.started.is_some()
    }

    pub fn label(&self) -> &str {
        self.label.as_deref().unwrap_or(DEFAULT_LABEL)
    }

    pub fn elapsed(&self, now: Instant) -> Duration {
        if let Some((_, elapsed)) = self.pinned {
            return elapsed;
        }
        self.started
            .map(|started| now.saturating_duration_since(started))
            .unwrap_or_default()
    }

    /// Which dot of the ring leads at this instant, in `0..SPINNER_DOTS`.
    pub fn frame(&self, now: Instant) -> usize {
        let index = match self.pinned {
            Some((frame, _)) => frame,
            None => (self.elapsed(now).as_millis() / SPINNER_FRAME.as_millis()) as usize,
        };
        index % SPINNER_DOTS
    }

    /// When the spinner next changes cell, or `None` when it is not running or
    /// is pinned (a pinned spinner must never ask for wakes, or captures would
    /// race the clock).
    pub fn next_frame_at(&self, now: Instant) -> Option<Instant> {
        if self.pinned.is_some() {
            return None;
        }
        self.started.map(|_| now + SPINNER_FRAME)
    }

    /// The one-line status text, e.g. `running tests · 12s · esc to
    /// interrupt`. The spinner itself is drawn as vector dots beside it.
    ///
    /// Seconds only under a minute, then `m:ss`: a turn that has been going for
    /// four minutes should say so in the shape people read clocks in.
    pub fn line(&self, now: Instant) -> Option<String> {
        if !self.is_running() {
            return None;
        }
        let seconds = self.elapsed(now).as_secs();
        let elapsed = if seconds < 60 {
            format!("{seconds}s")
        } else {
            format!("{}m{:02}s", seconds / 60, seconds % 60)
        };
        Some(format!("{} · {elapsed} · esc to interrupt", self.label()))
    }
}

/// Pull a tool call's `intent` out of streamed JSON input.
///
/// Every tool takes an `intent`: a short human sentence about why the call is
/// happening, which is exactly the label this UI wants. The arguments arrive as
/// a partial JSON string, so this scans for the field rather than parsing: a
/// half-streamed object is not valid JSON, and waiting for the close brace
/// would show the intent only after the call is already running.
pub fn intent_from_partial_json(input: &str) -> Option<String> {
    let key = "\"intent\"";
    let start = input.find(key)? + key.len();
    let rest = input[start..].trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let mut value = String::new();
    let mut chars = rest.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => return (!value.trim().is_empty()).then(|| value.trim().to_string()),
            '\\' => match chars.next() {
                Some('n') => value.push(' '),
                Some('t') => value.push(' '),
                Some(escaped) => value.push(escaped),
                None => break,
            },
            _ => value.push(ch),
        }
    }
    // Still streaming: show what we have so far rather than nothing, so a long
    // intent appears as it is typed instead of snapping in at the end.
    (!value.trim().is_empty()).then(|| value.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_idle_activity_has_no_line_and_no_wakes() {
        let activity = Activity::default();
        let now = Instant::now();
        assert!(!activity.is_running());
        assert_eq!(activity.line(now), None);
        assert_eq!(activity.next_frame_at(now), None);
    }

    #[test]
    fn starting_twice_keeps_the_original_turn_clock() {
        let mut activity = Activity::default();
        let start = Instant::now();
        activity.start(start);
        activity.start(start + Duration::from_secs(5));
        assert_eq!(
            activity.elapsed(start + Duration::from_secs(5)).as_secs(),
            5,
            "a later event restarted the elapsed clock"
        );
    }

    #[test]
    fn a_phase_arriving_first_still_starts_the_clock() {
        let mut activity = Activity::default();
        activity.set_label("running tests", Instant::now());
        assert!(activity.is_running());
        assert_eq!(activity.label(), "running tests");
    }

    #[test]
    fn an_empty_phase_never_replaces_a_real_one() {
        let mut activity = Activity::default();
        let now = Instant::now();
        activity.set_label("reading files", now);
        activity.set_label("   ", now);
        assert_eq!(activity.label(), "reading files");
    }

    #[test]
    fn the_spinner_advances_with_time_and_wraps() {
        let mut activity = Activity::default();
        let start = Instant::now();
        activity.start(start);
        let first = activity.frame(start);
        let second = activity.frame(start + SPINNER_FRAME);
        assert_ne!(first, second, "the spinner did not move between frames");
        assert_eq!(
            first,
            activity.frame(start + SPINNER_FRAME * SPINNER_DOTS as u32),
            "the spinner did not wrap around its dots"
        );
    }

    #[test]
    fn a_pinned_activity_renders_deterministically_and_sleeps() {
        let activity = Activity::pinned(3, Duration::from_secs(12), Some("reading files"));
        let now = Instant::now();
        assert_eq!(activity.frame(now), 3);
        assert_eq!(
            activity.line(now).as_deref(),
            Some("reading files · 12s · esc to interrupt")
        );
        assert_eq!(
            activity.next_frame_at(now),
            None,
            "a pinned spinner asked for a wake, so captures would race the clock"
        );
    }

    #[test]
    fn long_turns_read_as_minutes() {
        let activity = Activity::pinned(0, Duration::from_secs(245), None);
        let line = activity.line(Instant::now()).expect("running");
        assert!(line.contains("4m05s"), "{line}");
        assert!(line.contains(DEFAULT_LABEL), "{line}");
    }

    #[test]
    fn finishing_clears_the_phase_so_the_next_turn_does_not_inherit_it() {
        let mut activity = Activity::default();
        activity.set_label("running tests", Instant::now());
        activity.finish();
        activity.start(Instant::now());
        assert_eq!(activity.label(), DEFAULT_LABEL);
    }

    #[test]
    fn an_intent_is_read_out_of_partial_tool_json() {
        assert_eq!(
            intent_from_partial_json(r#"{"command":"ls","intent":"list the crate"}"#).as_deref(),
            Some("list the crate")
        );
        assert_eq!(
            intent_from_partial_json(r#"{"intent" : "check the build"#).as_deref(),
            Some("check the build"),
            "a still-streaming intent should show as it arrives"
        );
        assert_eq!(
            intent_from_partial_json(r#"{"intent":"say \"hi\" twice"}"#).as_deref(),
            Some("say \"hi\" twice"),
            "an escaped quote ended the value early"
        );
        assert_eq!(intent_from_partial_json(r#"{"command":"ls"}"#), None);
        assert_eq!(intent_from_partial_json(r#"{"intent":""}"#), None);
        assert_eq!(intent_from_partial_json(r#"{"intent"#), None);
    }
}
