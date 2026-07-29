//! Turning an edit tool's output into a transcript card.
//!
//! A tool call shows as one live line that disappears when the call ends, which
//! is right for `bash` or `read`: the work is transient. An *edit* is not
//! transient. It changed the user's files, and the record of what changed is
//! the single most important thing the agent produced in that step. So edits
//! get their own permanent card in the transcript: the intent the model wrote
//! ("why"), the file it touched ("where"), and the added and removed lines
//! ("what").
//!
//! The diff is recovered from the tool's own output rather than recomputed
//! here: every edit tool already prints `42- old` / `42+ new` lines, and
//! parsing what the agent actually reported cannot drift from what it did.

/// A completed edit, ready to be pushed into the transcript.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditCard {
    /// The `intent` the model wrote for the call, when it wrote one. This is
    /// the line a reader wants: "why did this change happen".
    pub intent: Option<String>,
    /// Files touched, in the order the tool reported them.
    pub files: Vec<String>,
    /// The diff body, one `42- old` / `42+ new` line per change.
    pub diff: String,
    /// Added and removed line counts, for the card's `+n -m` summary.
    pub added: usize,
    pub removed: usize,
}

/// Whether a tool name is one that writes to files. Only these are inspected
/// for a diff: a `bash` call whose output happens to contain a `-` line must
/// not be dressed up as an edit.
pub fn is_edit_tool(name: &str) -> bool {
    matches!(
        name,
        "edit" | "write" | "multiedit" | "patch" | "apply_patch" | "notebook_edit"
    )
}

/// Extract the edit card from a finished tool call.
///
/// Returns `None` when the tool is not an edit tool, or when it reported no
/// changed lines at all (a failed edit, or a write of identical content): a
/// card with an empty diff would claim a change that did not happen.
pub fn parse(name: &str, input_json: Option<&str>, output: &str) -> Option<EditCard> {
    if !is_edit_tool(name) {
        return None;
    }
    let mut diff = String::new();
    let mut added = 0usize;
    let mut removed = 0usize;
    for line in output.lines() {
        match classify(line) {
            Some(Change::Added) => added += 1,
            Some(Change::Removed) => removed += 1,
            None => continue,
        }
        diff.push_str(line.trim_end());
        diff.push('\n');
    }
    if diff.is_empty() {
        return None;
    }
    Some(EditCard {
        intent: input_json.and_then(crate::activity::intent_from_partial_json),
        files: input_json.map(files_in).unwrap_or_default(),
        diff,
        added,
        removed,
    })
}

/// Which side of a diff a line is, if it is one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Change {
    Added,
    Removed,
}

/// Classify one line of an edit tool's output.
///
/// The shared shape across every edit tool is `<line number><+|-> <content>`.
/// Requiring the number is what keeps prose out: a summary line like
/// "- wrote 3 files" is not a diff line, and neither is a bare `---` rule.
pub fn classify(line: &str) -> Option<Change> {
    let line = line.trim_start();
    let digits = line.len() - line.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    if digits == 0 {
        return None;
    }
    let rest = &line[digits..];
    let mut chars = rest.chars();
    let sign = chars.next()?;
    // A space after the sign, or nothing at all: `42+` on its own is a blank
    // line that was added, which is still a change worth showing.
    match chars.next() {
        Some(' ') | None => {}
        _ => return None,
    }
    match sign {
        '+' => Some(Change::Added),
        '-' => Some(Change::Removed),
        _ => None,
    }
}

/// File paths named in a tool's arguments.
///
/// Read out of the raw JSON rather than deserialized per tool: the argument is
/// spelled `file_path` by `edit`/`write`/`multiedit` and appears once per file
/// in a patch, so one scan covers every edit tool and cannot go stale when a
/// new one is added.
fn files_in(input: &str) -> Vec<String> {
    let mut files = Vec::new();
    let mut rest = input;
    while let Some(at) = rest.find("\"file_path\"") {
        rest = &rest[at + "\"file_path\"".len()..];
        let Some(open) = rest.find('"') else { break };
        rest = &rest[open + 1..];
        let Some(close) = rest.find('"') else { break };
        let value = &rest[..close];
        if !value.is_empty() && !files.iter().any(|seen| seen == value) {
            files.push(value.to_string());
        }
        rest = &rest[close + 1..];
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The common case: one `edit` call, so the card carries the intent the
    /// model wrote, the file, and the changed lines with their counts.
    #[test]
    fn an_edit_becomes_a_card() {
        let card = parse(
            "edit",
            Some(r#"{"file_path":"src/lib.rs","intent":"rename the field"}"#),
            "Edited src/lib.rs: replaced 1 occurrence(s)\n10- let old = 1;\n10+ let new = 1;\n\nContext after edit (lines 8-12):\nfn main() {",
        )
        .expect("an edit with changed lines is a card");
        assert_eq!(card.intent.as_deref(), Some("rename the field"));
        assert_eq!(card.files, vec!["src/lib.rs".to_string()]);
        assert_eq!((card.added, card.removed), (1, 1));
        assert_eq!(card.diff, "10- let old = 1;\n10+ let new = 1;\n");
    }

    /// Context lines and prose from the tool's own report must not leak into
    /// the diff: the card is the change, not the tool's transcript.
    #[test]
    fn only_diff_lines_land_in_the_card() {
        let card = parse(
            "write",
            None,
            "Created a.rs (2 lines):\n1+ fn main() {}\n2+ \n",
        )
        .expect("a create is all additions");
        // Trailing whitespace is trimmed: a diff line's own indentation is
        // meaningful, but trailing blanks only widen the card's measure.
        assert_eq!(card.diff, "1+ fn main() {}\n2+\n");
        assert_eq!((card.added, card.removed), (2, 0));
    }

    /// A tool that did not touch files never produces a card, however its
    /// output is shaped.
    #[test]
    fn a_non_edit_tool_is_not_an_edit() {
        assert!(parse("bash", None, "1- looks like a diff\n").is_none());
        assert!(parse("edit", None, "old_string not found").is_none());
    }

    /// Prose that merely starts with a sign, and rules, are not diff lines.
    #[test]
    fn prose_is_not_mistaken_for_a_diff() {
        assert_eq!(classify("- wrote three files"), None);
        assert_eq!(classify("---"), None);
        assert_eq!(classify("42+ added"), Some(Change::Added));
        assert_eq!(classify("  7- removed"), Some(Change::Removed));
        assert_eq!(classify("42+"), Some(Change::Added));
        assert_eq!(classify("42x nope"), None);
    }

    /// A multi-file patch names every file it touched, once each.
    #[test]
    fn every_touched_file_is_named_once() {
        let card = parse(
            "multiedit",
            Some(r#"{"file_path":"a.rs","edits":[{"file_path":"a.rs"}]}"#),
            "3- a\n3+ b\n",
        )
        .expect("card");
        assert_eq!(card.files, vec!["a.rs".to_string()]);
    }
}
