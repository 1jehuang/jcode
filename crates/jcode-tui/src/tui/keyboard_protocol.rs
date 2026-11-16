//! Match keyboard-protocol negotiation to the active input decoder.
//!
//! Crossterm 0.29 reads key input through `ReadConsoleInputW` on Windows, not
//! its CSI-u parser, so the kitty progressive-enhancement and tmux
//! `modifyOtherKeys` sequences cannot be consumed there. A half-supporting
//! ConPTY host (herdr, VS Code) then surfaces release and escape artifacts as
//! prompt text and keys can act on the wrong record (#1064, #1527, #1812).
//!
//! Every keyboard-protocol write in the TUI goes through [`write_mode`] so the
//! decision is made once: a `Push` reports that no enhancement was negotiated
//! when the native decoder is active, and the follow-up `Set`/`Pop` writes
//! emit nothing at all.
use std::io::{self, Write};

#[cfg(windows)]
use std::cell::RefCell;
#[cfg(windows)]
use std::collections::VecDeque;
#[cfg(windows)]
use std::time::{Duration, Instant};

/// A burst of typed characters spanning longer than this is human typing, not
/// one keyboard report.
#[cfg(windows)]
const REPORT_BURST_WINDOW: Duration = Duration::from_secs(2);

/// Largest pause allowed inside a single report.
#[cfg(windows)]
const REPORT_MAX_GAP: Duration = Duration::from_millis(750);

/// Longest report shape worth remembering: `[119;1:3u` and `[27;3;97~` fit.
#[cfg(windows)]
const REPORT_BUFFER_CAPACITY: usize = 16;

#[cfg(windows)]
thread_local! {
    /// Characters typed from key events, newest last. Only used to decide
    /// whether a literal report fragment was typed as one contiguous burst.
    static TYPED: RefCell<VecDeque<(Instant, char)>> = RefCell::new(VecDeque::new());
}

/// Record composer text that arrived from a key event.
///
/// Only single characters are remembered: a paste, an IME commit or a dropped
/// path clears the buffer, because none of those can be a keyboard report.
#[cfg(windows)]
pub(super) fn note_typed_text(text: &str) {
    TYPED.with(|typed| {
        let mut typed = typed.borrow_mut();
        let mut chars = text.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) => {
                if typed.len() == REPORT_BUFFER_CAPACITY {
                    typed.pop_front();
                }
                typed.push_back((Instant::now(), c));
            }
            _ => typed.clear(),
        }
    });
}

/// Remove a keyboard-protocol report that reached the composer as text.
///
/// Returns the stripped fragment so the caller can report it. This is the last
/// line of defence for issue #224: negotiation and focus handling no longer ask
/// for reporting the console decoder cannot consume, but a host that arms its
/// own keyboard mode still delivers `[119;1:3u` one character at a time.
#[cfg(windows)]
pub(super) fn scrub_leaked_report(input: &mut String, cursor: &mut usize) -> Option<String> {
    let burst: Vec<(Instant, char)> = TYPED.with(|typed| typed.borrow().iter().copied().collect());
    let stripped = scrub_matching_report(input, cursor, &burst)?;
    TYPED.with(|typed| typed.borrow_mut().clear());
    Some(stripped)
}

/// Core of [`scrub_leaked_report`] with the typed burst supplied, so tests can
/// drive it without a terminal.
#[cfg(windows)]
fn scrub_matching_report(
    input: &mut String,
    cursor: &mut usize,
    burst: &[(Instant, char)],
) -> Option<String> {
    if *cursor != input.len() {
        return None;
    }
    let start = input.rfind('[')?;
    let fragment = input[start..].to_string();
    if !is_report_fragment(&fragment) {
        return None;
    }
    let len = fragment.chars().count();
    if burst.len() < len {
        return None;
    }
    let tail = &burst[burst.len() - len..];
    if !tail.iter().map(|(_, c)| *c).eq(fragment.chars()) {
        return None;
    }
    let first = tail.first()?.0;
    let last = tail.last()?.0;
    if last.duration_since(first) > REPORT_BURST_WINDOW {
        return None;
    }
    if tail
        .windows(2)
        .any(|pair| pair[1].0.duration_since(pair[0].0) > REPORT_MAX_GAP)
    {
        return None;
    }
    input.truncate(start);
    *cursor = start;
    Some(fragment)
}

/// Whether `fragment` has the shape of a keyboard report rather than text.
///
/// Accepts kitty `CSI code;mods:event u` (the `:event` sub-parameter is what a
/// decoder that cannot parse it leaves behind) and tmux/xterm
/// `CSI 27;mod;code ~`. Everything else is treated as ordinary text.
#[cfg(windows)]
fn is_report_fragment(fragment: &str) -> bool {
    if fragment.chars().count() < 5 {
        return false;
    }
    let Some(body) = fragment.strip_prefix('[') else {
        return false;
    };
    if let Some(body) = body.strip_suffix('u') {
        let mut saw_event_type = false;
        let mut first_code = None;
        for (index, part) in body.split(';').enumerate() {
            let (code, event) = match part.split_once(':') {
                Some((code, event)) => {
                    saw_event_type = true;
                    (code, Some(event))
                }
                None => (part, None),
            };
            if !is_decimal(code) || event.is_some_and(|event| !is_decimal(event)) {
                return false;
            }
            if index == 0 {
                first_code = code.parse::<u32>().ok();
            }
        }
        // A release/repeat report always carries `:event`. A modified key may
        // not, but its code is a printable/keypad code (at least space) rather
        // than a control code, which is what keeps `[1;5u` typed by a human
        // from being treated as a report.
        return saw_event_type || first_code.is_some_and(|code| code >= 32);
    }
    if let Some(body) = body.strip_suffix('~') {
        let parts: Vec<&str> = body.split(';').collect();
        return parts.len() >= 3 && parts[0] == "27" && parts.iter().all(|part| is_decimal(part));
    }
    false
}

#[cfg(windows)]
fn is_decimal(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

/// One operation on the keyboard-protocol state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    /// Startup negotiation, stack-based (`CSI > flags u`).
    Push,
    /// Reassert the flags after a terminal mode reset (`CSI = flags u`).
    Set,
    /// Balance the startup push (`CSI < u`).
    Pop,
}

/// Write one keyboard-protocol operation for the current platform's decoder.
pub(super) fn write_mode(
    writer: &mut impl Write,
    action: Action,
    inside_tmux: bool,
    flags: u8,
) -> io::Result<()> {
    write_mode_for_decoder(writer, action, inside_tmux, flags, cfg!(windows))
}

fn write_mode_for_decoder(
    writer: &mut impl Write,
    action: Action,
    inside_tmux: bool,
    flags: u8,
    native_console: bool,
) -> io::Result<()> {
    if native_console {
        // Startup must report that no enhancement was negotiated. Focus,
        // resume and teardown are harmless even when handed configured=true,
        // but they must still write nothing: the console input decoder would
        // never produce the sequences the terminal starts sending.
        //
        // The one write that is always safe is a clear: `CSI = 0 u` sets the
        // flags without touching stack depth, so it cannot pop an entry an
        // ancestor owns. A previous process, a cancelled or killed client, or
        // the resume picker that spawns a child can leave enhancement armed.
        // That inherited state is pure noise for a decoder that cannot consume
        // CSI-u, and it is the intermittent, worse-on-resume half of #224.
        return match action {
            Action::Push => {
                write_clear_inherited_reporting(writer)?;
                Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "kitty keyboard reporting is incompatible with the Windows console input decoder",
                ))
            }
            Action::Set | Action::Pop => write_clear_inherited_reporting(writer),
        };
    }
    if inside_tmux {
        writer.write_all(match action {
            Action::Pop => b"\x1b[>4;0m",
            Action::Push | Action::Set => b"\x1b[>4;2m",
        })?;
    }
    match action {
        Action::Push => write!(writer, "\x1b[>{flags}u")?,
        Action::Set => write!(writer, "\x1b[={flags}u")?,
        Action::Pop => writer.write_all(b"\x1b[<1u")?,
    }
    writer.flush()
}

/// Write `CSI = 0 u`, which sets the kitty keyboard flags to zero without
/// changing the stack depth.
///
/// Only used on the Windows console decoder, which can never consume CSI-u
/// input, so any inherited enhancement is noise. Unlike `CSI < u` this cannot
/// pop a stack entry owned by an ancestor process.
fn write_clear_inherited_reporting(writer: &mut impl Write) -> io::Result<()> {
    writer.write_all(b"\x1b[=0u")?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_never_switches_native_key_records_to_csi_u() {
        for tmux in [false, true] {
            let mut output = Vec::new();
            let error =
                write_mode_for_decoder(&mut output, Action::Push, tmux, 7, true).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::Unsupported);
            // Focus/resume and teardown are handed configured state even after
            // Push failed; they must not write the sequences either.
            for _ in 0..3 {
                write_mode_for_decoder(&mut output, Action::Set, tmux, 7, true).unwrap();
            }
            write_mode_for_decoder(&mut output, Action::Pop, tmux, 7, true).unwrap();
            let mut expected = Vec::new();
            for _ in 0..5 {
                expected.extend_from_slice(b"\x1b[=0u");
            }
            assert_eq!(
                output, expected,
                "only the stack-neutral clear may be written, never CSI-u, \
                 modifyOtherKeys, or a stack pop"
            );
        }
    }

    #[test]
    fn windows_clear_never_pops_a_stack_entry_it_may_not_own() {
        let mut output = Vec::new();
        write_mode_for_decoder(&mut output, Action::Pop, false, 7, true).unwrap();
        assert_eq!(output, b"\x1b[=0u".as_slice());
        assert!(
            !output.windows(4).any(|window| window == b"\x1b[<1"),
            "a pop could remove an entry an ancestor owns"
        );
    }

    #[test]
    fn unix_never_writes_the_windows_clear() {
        let mut output = Vec::new();
        for action in [Action::Push, Action::Set, Action::Pop] {
            write_mode_for_decoder(&mut output, action, false, 7, false).unwrap();
        }
        assert!(
            !output.windows(5).any(|window| window == b"\x1b[=0u"),
            "non-Windows decoders consume CSI-u, so their negotiated flags stay intact"
        );
    }

    #[test]
    fn unix_keeps_balanced_kitty_stack_and_tmux_reporting() {
        for tmux in [false, true] {
            let mut output = Vec::new();
            for action in [Action::Push, Action::Set, Action::Pop] {
                write_mode_for_decoder(&mut output, action, tmux, 7, false).unwrap();
            }
            let expected: &[u8] = if tmux {
                b"\x1b[>4;2m\x1b[>7u\x1b[>4;2m\x1b[=7u\x1b[>4;0m\x1b[<1u"
            } else {
                b"\x1b[>7u\x1b[=7u\x1b[<1u"
            };
            assert_eq!(output, expected);
        }
    }

    #[test]
    fn platform_entry_matches_native_decoder() {
        let mut output = Vec::new();
        let result = write_mode(&mut output, Action::Push, false, 7);
        assert_eq!(result.is_err(), cfg!(windows));
        // The native console decoder never negotiates CSI-u, so the only bytes
        // it may write are the stack-neutral clear.
        let expected: &[u8] = if cfg!(windows) {
            b"\x1b[=0u"
        } else {
            b"\x1b[>7u"
        };
        assert_eq!(output, expected);
    }
}

#[cfg(all(test, windows))]
mod leaked_report_tests {
    use super::*;

    fn burst(text: &str, gap_ms: u64) -> Vec<(Instant, char)> {
        let start = Instant::now();
        text.chars()
            .enumerate()
            .map(|(index, c)| (start + Duration::from_millis(gap_ms * index as u64), c))
            .collect()
    }

    #[test]
    fn recognises_the_fragments_a_console_decoder_cannot_parse() {
        // Release report, exactly what #224 captured.
        assert!(is_report_fragment("[119;1:3u"));
        // Modified key without an event sub-parameter.
        assert!(is_report_fragment("[97;5u"));
        // tmux/xterm modifyOtherKeys.
        assert!(is_report_fragment("[27;3;97~"));
    }

    #[test]
    fn leaves_ordinary_text_alone() {
        for text in [
            "[1;5u", // control code, which a human can type
            "[hello]",
            "[1234]",
            "array[32]",
            "[27;3;97",
            "u",
            "",
        ] {
            assert!(!is_report_fragment(text), "{text} must stay text");
        }
    }

    #[test]
    fn strips_a_fragment_typed_as_one_burst() {
        let mut input = String::from("hi[119;1:3u");
        let mut cursor = input.len();
        let stripped = scrub_matching_report(&mut input, &mut cursor, &burst("[119;1:3u", 20));
        assert_eq!(stripped.as_deref(), Some("[119;1:3u"));
        assert_eq!(input, "hi");
        assert_eq!(cursor, 2);
    }

    #[test]
    fn keeps_a_fragment_typed_slowly_by_a_human() {
        let mut input = String::from("hi[119;1:3u");
        let mut cursor = input.len();
        // 3 s total span: human typing, not one report.
        assert!(scrub_matching_report(&mut input, &mut cursor, &burst("[119;1:3u", 400)).is_none());
        assert_eq!(input, "hi[119;1:3u");
    }

    #[test]
    fn keeps_a_fragment_with_a_long_pause_inside() {
        let mut typed = burst("[119", 10);
        // A 1.5 s pause in the middle of the shape.
        let resume = typed.last().unwrap().0 + Duration::from_millis(1500);
        for (index, c) in ";1:3u".chars().enumerate() {
            typed.push((resume + Duration::from_millis(10 * index as u64), c));
        }
        let mut input = String::from("[119;1:3u");
        let mut cursor = input.len();
        assert!(scrub_matching_report(&mut input, &mut cursor, &typed).is_none());
    }

    #[test]
    fn ignores_a_fragment_that_is_not_at_the_cursor() {
        let mut input = String::from("[119;1:3u tail");
        let mut cursor = input.len();
        assert!(scrub_matching_report(&mut input, &mut cursor, &burst("[119;1:3u", 20)).is_none());
        assert_eq!(input, "[119;1:3u tail");
    }

    #[test]
    fn ignores_a_shape_whose_characters_were_not_the_typed_burst() {
        let mut input = String::from("[119;1:3u");
        let mut cursor = input.len();
        assert!(scrub_matching_report(&mut input, &mut cursor, &burst("[97;1;3u", 20)).is_none());
    }
}
