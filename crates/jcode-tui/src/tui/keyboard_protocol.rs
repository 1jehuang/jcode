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
