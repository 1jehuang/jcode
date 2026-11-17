//! Native agent-state reporting for Herdr Desktop (herdr-core, `hc`).
//!
//! Herdr Desktop treats a terminal as an agent when the program inside it
//! reports status with OSC 7501, written straight to the terminal:
//!
//! ```text
//! ESC ] 7501 ; state=<word>:app=jcode[:title=<base64>][:msg=<base64>] BEL
//! ```
//!
//! Fields are `:` separated, `state` comes first, and free text (`title`,
//! `msg`) is base64 or the core drops the whole report. `state=clear` removes
//! the status when jcode quits. Detection is `HC_TERMINAL_ID` in the
//! environment, which herdr-core sets in every terminal it runs; set
//! `JCODE_HERDR_OSC=0` to opt out. This is independent of the classic herdr
//! CLI reporting in [`super::herdr`]; both may be active at once.
//!
//! Reports are written from the UI thread between frames, so they never
//! interleave with ratatui's output, and only when something changed.

use base64::Engine as _;
use std::io::Write;
use std::sync::{Mutex, OnceLock};

const APP: &str = "jcode";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Idle,
    Working,
}

impl State {
    fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Report {
    state: State,
    title: String,
    msg: String,
}

#[derive(Default)]
struct Tracker {
    last: Option<Report>,
    cleared: bool,
}

static TRACKER: Mutex<Tracker> = Mutex::new(Tracker {
    last: None,
    cleared: false,
});

fn enabled_from(get: impl Fn(&str) -> Option<String>) -> bool {
    let set = |key: &str| get(key).is_some_and(|value| !value.trim().is_empty());
    if get("JCODE_HERDR_OSC").is_some_and(|value| {
        matches!(value.trim(), "0" | "false" | "off" | "no")
    }) {
        return false;
    }
    set("HC_TERMINAL_ID")
}

/// Whether jcode runs inside a Herdr Desktop (herdr-core) terminal.
pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        let on = enabled_from(|key| std::env::var(key).ok());
        if on {
            crate::logging::info("herdr-core: reporting agent state via OSC 7501");
        }
        on
    })
}

fn b64(text: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(text.as_bytes())
}

/// Keep reported text short and on one line.
fn clean(text: &str, max_chars: usize) -> String {
    let one_line: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    one_line.trim().chars().take(max_chars).collect()
}

fn encode(report: &Report) -> String {
    let mut out = format!("\x1b]7501;state={}:app={APP}", report.state.as_str());
    if !report.title.is_empty() {
        out.push_str(":title=");
        out.push_str(&b64(&report.title));
    }
    if !report.msg.is_empty() {
        out.push_str(":msg=");
        out.push_str(&b64(&report.msg));
    }
    out.push('\x07');
    out
}

fn clear_sequence() -> &'static str {
    "\x1b]7501;state=clear\x07"
}

fn write_raw(sequence: &str) -> bool {
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(sequence.as_bytes()).is_ok() && stdout.flush().is_ok()
}

/// Returns the sequence to send, or `None` when nothing changed.
fn next_sequence(tracker: &mut Tracker, report: Report) -> Option<String> {
    if tracker.cleared || tracker.last.as_ref() == Some(&report) {
        return None;
    }
    let sequence = encode(&report);
    tracker.last = Some(report);
    Some(sequence)
}

/// Report state, title and activity. Call from the UI thread only; cheap
/// when nothing changed and a no-op outside Herdr Desktop.
pub fn sync(state: State, title: &str, msg: &str) {
    if !enabled() {
        return;
    }
    let report = Report {
        state,
        title: clean(title, 120),
        msg: clean(msg, 200),
    };
    let mut tracker = TRACKER.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(sequence) = next_sequence(&mut tracker, report)
        && !write_raw(&sequence)
    {
        // Let the next tick retry.
        tracker.last = None;
    }
}

/// Clear the status because the user is quitting jcode. Do not call this
/// when jcode is about to exec itself (reload/update).
pub fn release() {
    if !enabled() {
        return;
    }
    let mut tracker = TRACKER.lock().unwrap_or_else(|e| e.into_inner());
    if tracker.cleared || tracker.last.is_none() {
        return;
    }
    tracker.cleared = true;
    write_raw(clear_sequence());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(state: State, title: &str, msg: &str) -> Report {
        Report {
            state,
            title: title.into(),
            msg: msg.into(),
        }
    }

    #[test]
    fn encodes_minimal_report() {
        assert_eq!(
            encode(&report(State::Working, "", "")),
            "\x1b]7501;state=working:app=jcode\x07"
        );
    }

    #[test]
    fn encodes_text_fields_as_base64() {
        assert_eq!(
            encode(&report(State::Idle, "hi", "hello")),
            "\x1b]7501;state=idle:app=jcode:title=aGk=:msg=aGVsbG8=\x07"
        );
    }

    #[test]
    fn base64_keeps_separators_out_of_payload() {
        let seq = encode(&report(State::Idle, "a:b;c\x07", "x=y"));
        let body = &seq["\x1b]7501;".len()..seq.len() - 1];
        assert!(!body.contains(';'));
        assert!(!body.contains('\x07'));
        assert_eq!(body.matches(':').count(), 3);
    }

    #[test]
    fn clean_strips_control_and_truncates() {
        assert_eq!(clean("  a\nb\tc  ", 10), "a b c");
        assert_eq!(clean("abcdef", 3), "abc");
    }

    #[test]
    fn unchanged_reports_are_skipped() {
        let mut tracker = Tracker::default();
        assert!(next_sequence(&mut tracker, report(State::Idle, "t", "")).is_some());
        assert!(next_sequence(&mut tracker, report(State::Idle, "t", "")).is_none());
        assert!(next_sequence(&mut tracker, report(State::Working, "t", "")).is_some());
        tracker.cleared = true;
        assert!(next_sequence(&mut tracker, report(State::Idle, "t", "")).is_none());
    }

    #[test]
    fn detection_follows_env() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |key: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, v)| v.to_string())
            }
        };
        assert!(enabled_from(env(&[("HC_TERMINAL_ID", "t7")])));
        assert!(!enabled_from(env(&[])));
        assert!(!enabled_from(env(&[("HC_TERMINAL_ID", " ")])));
        assert!(!enabled_from(env(&[
            ("HC_TERMINAL_ID", "t7"),
            ("JCODE_HERDR_OSC", "0")
        ])));
    }
}
