//! Plain-stdout rendering of streamed reasoning for `jcode run` and the
//! line-based REPL.
//!
//! Providers deliver reasoning as many small deltas, often only a few words
//! each. Printing every delta on its own line (with its own prefix) chopped the
//! reasoning into a narrow column regardless of terminal width. Instead, the
//! prefix is printed once when a reasoning block starts, deltas are appended
//! in place so the terminal soft-wraps them at its real width, and the block is
//! terminated with a newline as soon as any other output needs the line.

use std::io::{self, Write};

const REASONING_PREFIX: &str = "💭 ";

#[derive(Debug, Default)]
pub(super) struct CliReasoningLine {
    /// A reasoning block has been started and not yet terminated.
    open: bool,
    /// The last byte written for the open block was a newline, so closing the
    /// block must not add another one.
    at_line_start: bool,
}

impl CliReasoningLine {
    /// Append a reasoning delta, starting a new prefixed block if needed.
    pub(super) fn push(&mut self, out: &mut impl Write, delta: &str) -> io::Result<()> {
        let delta = if self.open {
            delta
        } else {
            // Leading whitespace in the first delta would double the gap after
            // the prefix; skip empty deltas so they don't open a bare prefix.
            let trimmed = delta.trim_start();
            if trimmed.is_empty() {
                return Ok(());
            }
            write!(
                out,
                "{}",
                crate::output_style::terminal_text(REASONING_PREFIX)
            )?;
            self.open = true;
            trimmed
        };
        if delta.is_empty() {
            return Ok(());
        }
        write!(out, "{}", crate::output_style::terminal_text(delta))?;
        self.at_line_start = delta.ends_with('\n');
        out.flush()
    }

    /// Terminate the current reasoning block, if any, so following output
    /// starts on a fresh line.
    pub(super) fn close(&mut self, out: &mut impl Write) -> io::Result<()> {
        if self.open {
            if !self.at_line_start {
                writeln!(out)?;
            }
            self.open = false;
            self.at_line_start = false;
            out.flush()?;
        }
        Ok(())
    }

    /// Convenience wrapper for the stdout call sites in the turn loop.
    pub(super) fn close_stdout(&mut self) -> io::Result<()> {
        self.close(&mut io::stdout().lock())
    }

    pub(super) fn push_stdout(&mut self, delta: &str) -> io::Result<()> {
        self.push(&mut io::stdout().lock(), delta)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(steps: &[Option<&str>]) -> String {
        crate::output_style::set_emoji_enabled(true);
        let mut out = Vec::new();
        let mut line = CliReasoningLine::default();
        for step in steps {
            match step {
                Some(delta) => line.push(&mut out, delta).unwrap(),
                None => line.close(&mut out).unwrap(),
            }
        }
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn deltas_share_one_prefixed_line() {
        let rendered = render(&[
            Some("I'm starting the"),
            Some(" investigation now"),
            Some(", keeping it read-only."),
            None,
        ]);
        assert_eq!(
            rendered,
            "💭 I'm starting the investigation now, keeping it read-only.\n"
        );
    }

    #[test]
    fn leading_whitespace_and_empty_first_deltas_do_not_open_block() {
        assert_eq!(render(&[Some(""), Some("  "), None]), "");
        assert_eq!(render(&[Some("\n Hello"), None]), "💭 Hello\n");
    }

    #[test]
    fn close_does_not_double_trailing_newline() {
        assert_eq!(render(&[Some("para one\n"), None]), "💭 para one\n");
    }

    #[test]
    fn close_is_idempotent_and_new_block_gets_new_prefix() {
        let rendered = render(&[Some("first"), None, None, Some("second"), None]);
        assert_eq!(rendered, "💭 first\n💭 second\n");
    }
}
