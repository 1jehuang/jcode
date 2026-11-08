/// Strip terminal escape-sequence remnants from text headed for the composer.
///
/// Mouse reporting, bracketed paste, and focus events all arrive as CSI
/// sequences. When a read is torn mid-sequence (a fast wheel flick split across
/// two reads, a loaded machine, a slow SSH link), the terminal-event parser can
/// resynchronize partway through and hand the tail of the sequence back as
/// ordinary text, which then lands in the draft as noise like `[<65;50;24M`
/// (issue #540). Whether that happens depends on the terminal and on timing, so
/// rather than rely on the parser never mis-syncing, drop the remnants at the
/// single insertion boundary every input path shares.
///
/// Deliberately conservative: only ESC-introduced sequences and bare CSI-shaped
/// runs are removed, plus C0 control characters other than tab and newline.
///
/// A *bare* run (one whose ESC introducer was consumed by the torn read) is only
/// stripped when it looks unmistakably like a terminal report: `[`, at least one
/// parameter byte, and a final byte that terminals actually emit for the
/// sequences we enable (`M`/`m` mouse, `~` bracketed paste and special keys,
/// `R` cursor position, `I`/`O` focus). This is what keeps ordinary typed text
/// such as `array[0]`, `list[1]`, or `[TODO]` intact, since `]` and letters like
/// `O` only qualify with a preceding numeric parameter for the specific finals
/// listed. Anything else is left alone: a missed remnant is cosmetic, whereas
/// eating a user's real characters is not.
pub(in crate::tui::app) fn strip_terminal_control_sequences(
    text: &str,
) -> std::borrow::Cow<'_, str> {
    let looks_suspicious = text
        .chars()
        .any(|ch| ch == '\x1b' || ch == '\u{9b}' || (ch.is_control() && ch != '\t' && ch != '\n'));
    let has_csi_run = text.contains("\x1b[") || text.contains('\u{9b}') || {
        let bytes = text.as_bytes();
        bytes.iter().enumerate().any(|(index, byte)| {
            *byte == b'[' && bare_terminal_report_length(&bytes[index..]).is_some()
        })
    };
    if !looks_suspicious && !has_csi_run {
        return std::borrow::Cow::Borrowed(text);
    }

    let bytes = text.as_bytes();
    let mut cleaned = String::with_capacity(text.len());
    let mut index = 0usize;
    while index < bytes.len() {
        let byte = bytes[index];
        // ESC-introduced sequence: skip the introducer, then the body.
        if byte == 0x1b {
            index += 1;
            if index < bytes.len() && bytes[index] == b'[' {
                match csi_run_length(&bytes[index..]) {
                    Some(len) => index += len,
                    // Unterminated: the rest is a truncated sequence, drop it.
                    None => index = bytes.len(),
                }
            } else if index < bytes.len() {
                // Two-character escape such as ESC O or ESC ].
                index += 1;
            }
            continue;
        }
        // 8-bit CSI introducer (UTF-8 encoded U+009B).
        if byte == 0xc2 && bytes.get(index + 1) == Some(&0x9b) {
            index += 2;
            if bytes.get(index) == Some(&b'[') {
                match csi_run_length(&bytes[index..]) {
                    Some(len) => index += len,
                    None => index = bytes.len(),
                }
            }
            continue;
        }
        // A bare report-shaped run left behind by a torn read.
        if byte == b'['
            && let Some(len) = bare_terminal_report_length(&bytes[index..])
        {
            index += len;
            continue;
        }
        // Drop stray C0 controls; keep tab and newline, which are meaningful.
        if byte < 0x20 && byte != b'\t' && byte != b'\n' {
            index += 1;
            continue;
        }
        // Copy one whole UTF-8 character.
        let char_len = text[index..]
            .chars()
            .next()
            .map(char::len_utf8)
            .unwrap_or(1);
        cleaned.push_str(&text[index..index + char_len]);
        index += char_len;
    }

    std::borrow::Cow::Owned(cleaned)
}

/// Length of a CSI run starting at `bytes[0]` (the `[`), including its final
/// byte, or `None` when the run is unterminated.
///
/// A CSI body is parameter bytes `0x30..=0x3f`, then intermediate bytes
/// `0x20..=0x2f`, then one final byte `0x40..=0x7e`.
fn csi_run_length(bytes: &[u8]) -> Option<usize> {
    debug_assert_eq!(bytes.first(), Some(&b'['));
    let mut index = 1usize;
    while index < bytes.len() && (0x30..=0x3f).contains(&bytes[index]) {
        index += 1;
    }
    while index < bytes.len() && (0x20..=0x2f).contains(&bytes[index]) {
        index += 1;
    }
    let final_byte = *bytes.get(index)?;
    if (0x40..=0x7e).contains(&final_byte) {
        Some(index + 1)
    } else {
        None
    }
}

/// Length of a bare (ESC-less) run that is unmistakably a terminal report, or
/// `None` when the run could plausibly be text the user typed.
///
/// Requires at least one parameter byte and one of the final bytes emitted by
/// the reporting modes jcode enables, so `array[0]` and `[TODO]` are left alone
/// while `[<65;50;24M` and `[200~` are recognized.
fn bare_terminal_report_length(bytes: &[u8]) -> Option<usize> {
    debug_assert_eq!(bytes.first(), Some(&b'['));
    let len = csi_run_length(bytes)?;
    let params = &bytes[1..len - 1];
    if params.is_empty() {
        return None;
    }
    // Mouse/paste/cursor/focus reports carry digits, `;`, and an optional
    // leading `<`. Reject anything with other parameter bytes.
    if !params
        .iter()
        .all(|byte| byte.is_ascii_digit() || *byte == b';' || *byte == b'<')
    {
        return None;
    }
    const REPORT_FINALS: [u8; 6] = *b"Mm~RIO";
    REPORT_FINALS.contains(&bytes[len - 1]).then_some(len)
}
