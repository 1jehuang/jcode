//! Native OSC 7501 (Program Status Protocol) reporting for herdr-core.
//!
//! herdr-core derives a terminal's agent status only from OSC 7501 reports
//! written by the program that owns the pane. Jcode's TUI client owns the
//! pane's PTY (the turn itself runs in the shared server), so the client
//! emits the report here. See <https://herdr.dev/core/> and the protocol at
//! <https://www.superlogical.com/rex/docs/build/program-status>.
//!
//! Unknown OSCs must be ignored by terminals, so this is safe outside herdr.
//! Reports are small, state changes only, and never block the UI.

use base64::Engine as _;

/// Stable program id used for grouping, filtering, and icons in herdr.
const APP: &str = "jcode";
/// The protocol caps a decoded title at 192 bytes.
const MAX_TITLE_BYTES: usize = 192;
/// Keep one report well under the protocol's 4096-byte whole-sequence cap.
const MAX_MESSAGE_BYTES: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProgramState {
    Idle,
    Working,
    Done,
    Error,
}

impl ProgramState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Done => "done",
            Self::Error => "error",
        }
    }
}

/// Build one OSC 7501 report.
///
/// `title` and `msg` are optional and standard-base64 encoded as the protocol
/// requires. Control characters are replaced before encoding because a report
/// that decodes to text containing them is discarded whole.
pub(crate) fn report(state: ProgramState, title: Option<&str>, message: Option<&str>) -> String {
    let mut report = format!("\x1b]7501;state={}:app={}", state.as_str(), APP);
    if let Some(title) = encoded_field(title, MAX_TITLE_BYTES) {
        report.push_str(":title=");
        report.push_str(&title);
    }
    if let Some(message) = encoded_field(message, MAX_MESSAGE_BYTES) {
        report.push_str(":msg=");
        report.push_str(&message);
    }
    report.push_str("\x1b\\");
    report
}

/// Write a report to the pane. The TUI client's stdout is the pane PTY.
pub(crate) fn emit(report: &str) {
    use std::io::Write as _;

    let mut stdout = std::io::stdout();
    if stdout
        .write_all(report.as_bytes())
        .and_then(|()| stdout.flush())
        .is_err()
    {
        crate::logging::debug("program status: failed to write OSC 7501 report");
    }
}

fn encoded_field(value: Option<&str>, max_bytes: usize) -> Option<String> {
    let value = value.filter(|value| !value.trim().is_empty())?;
    let value = sanitized_message(value, max_bytes);
    (!value.is_empty()).then(|| base64::engine::general_purpose::STANDARD.encode(value.as_bytes()))
}

fn sanitized_message(message: &str, max_bytes: usize) -> String {
    let mut sanitized = String::new();
    for character in message.chars() {
        let character = if character.is_control() {
            ' '
        } else {
            character
        };
        if sanitized.len() + character.len_utf8() > max_bytes {
            break;
        }
        sanitized.push(character);
    }
    sanitized.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_field(report: &str, key: &str) -> String {
        let marker = format!(":{key}=");
        let encoded = report
            .split_once(marker.as_str())
            .unwrap_or_else(|| panic!("report carries {key}"))
            .1
            .split_once('\x1b')
            .expect("report is terminated")
            .0;
        String::from_utf8(
            base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .expect("message is valid base64"),
        )
        .expect("message is valid UTF-8")
    }

    #[test]
    fn reports_are_well_formed_osc_7501() {
        assert_eq!(
            report(ProgramState::Working, None, None),
            "\x1b]7501;state=working:app=jcode\x1b\\"
        );
        assert_eq!(
            report(ProgramState::Done, None, None),
            "\x1b]7501;state=done:app=jcode\x1b\\"
        );
    }

    #[test]
    fn error_messages_are_base64_and_control_free() {
        let osc = report(ProgramState::Error, None, Some("line one\nline two\t\u{7}"));
        assert!(osc.starts_with("\x1b]7501;state=error:app=jcode:msg="));
        assert!(osc.ends_with("\x1b\\"));
        assert_eq!(decode_field(&osc, "msg"), "line one line two");
    }

    #[test]
    fn empty_and_control_only_messages_are_omitted() {
        assert_eq!(
            report(ProgramState::Error, None, Some(" \n\t ")),
            "\x1b]7501;state=error:app=jcode\x1b\\"
        );
        assert_eq!(
            report(ProgramState::Error, None, None),
            "\x1b]7501;state=error:app=jcode\x1b\\"
        );
    }

    #[test]
    fn long_messages_are_truncated_before_encoding() {
        let osc = report(
            ProgramState::Error,
            None,
            Some(&"x".repeat(MAX_MESSAGE_BYTES * 2)),
        );
        assert!(osc.len() < 4096);
        assert_eq!(decode_field(&osc, "msg").len(), MAX_MESSAGE_BYTES);
    }

    #[test]
    fn session_titles_are_base64_and_clipped() {
        let osc = report(
            ProgramState::Working,
            Some("Session title\nwith a control"),
            None,
        );
        assert!(osc.contains(":title="));
        assert_eq!(
            decode_field(&osc, "title"),
            "Session title with a control"
        );

        let long = "x".repeat(MAX_TITLE_BYTES * 2);
        let osc = report(ProgramState::Working, Some(&long), None);
        assert_eq!(decode_field(&osc, "title").len(), MAX_TITLE_BYTES);
    }
}
