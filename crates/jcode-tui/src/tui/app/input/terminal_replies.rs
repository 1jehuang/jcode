//! Strict, bounded OSC color response parsing, separate from composer text.
use std::borrow::Cow;

#[derive(Default)]
pub(in crate::tui::app) struct OscColorReplyFilter {
    pending: String,
}

impl OscColorReplyFilter {
    pub(in crate::tui::app) fn reset(&mut self) {
        self.pending.clear();
    }

    pub(in crate::tui::app) fn is_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub(in crate::tui::app) fn filter<'a>(&mut self, text: &'a str) -> Cow<'a, str> {
        if self.pending.is_empty() && !text.contains(['\x1b', '\u{9d}']) {
            return Cow::Borrowed(text);
        }
        let mut cleaned = String::with_capacity(text.len());
        for ch in text.chars() {
            if self.pending.is_empty() {
                if matches!(ch, '\x1b' | '\u{9d}') {
                    self.pending.push(ch);
                } else {
                    cleaned.push(ch);
                }
                continue;
            }
            self.pending.push(ch);
            if strip_osc_color_replies(&self.pending, self.pending.len())
                .is_some_and(|(text, _)| text.is_empty())
            {
                self.pending.clear();
            } else if !is_color_reply_prefix(&self.pending) {
                // A malformed frame must not arm continuation indefinitely.
                // Retain its printable body; the shared sanitizer removes any
                // remaining control bytes before this reaches the composer.
                let candidate = std::mem::take(&mut self.pending);
                let body = candidate
                    .strip_prefix("\x1b]")
                    .or_else(|| candidate.strip_prefix('\u{9d}'))
                    .unwrap_or(&candidate);
                cleaned.push_str(body);
            }
        }
        Cow::Owned(cleaned)
    }
}

fn is_color_reply_prefix(text: &str) -> bool {
    // Maximum valid response is under 40 bytes. Bound even malformed input.
    if text.len() > 64 {
        return false;
    }
    if text == "\x1b" {
        return true;
    }
    let Some(body) = text
        .strip_prefix("\x1b]")
        .or_else(|| text.strip_prefix('\u{9d}'))
    else {
        return false;
    };
    // ST can arrive as two separate reads.
    let body = body.strip_suffix('\x1b').unwrap_or(body);
    for command in 10..=19 {
        for (kind, components) in [("rgb", 3), ("rgba", 4)] {
            let header = format!("{command};{kind}:");
            if header.starts_with(body) {
                return true;
            }
            if let Some(color) = body.strip_prefix(&header) {
                let parts: Vec<_> = color.split('/').collect();
                if parts.len() <= components
                    && parts.iter().enumerate().all(|(index, part)| {
                        part.len() <= 4
                            && (index + 1 == parts.len() || !part.is_empty())
                            && part.bytes().all(|byte| byte.is_ascii_hexdigit())
                    })
                {
                    return true;
                }
            }
        }
    }
    false
}

/// Strip BEL/ST-terminated OSC color replies, remapping the byte cursor.
/// Neither a bare RGB body nor printable `]`/`\` characters are framing.
pub(in crate::tui::app) fn strip_osc_color_replies(
    input: &str,
    cursor: usize,
) -> Option<(String, usize)> {
    let bytes = input.as_bytes();
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut search = 0usize;
    while let Some(found) = input[search..].find(";rgb") {
        let semi = search + found;
        search = semi + 1;
        if semi < 2 || bytes[semi - 2] != b'1' || !bytes[semi - 1].is_ascii_digit() {
            continue;
        }
        let body_start = semi - 2;
        let start = if input[..body_start].ends_with("\x1b]") {
            body_start - 2
        } else if input[..body_start].ends_with('\u{9d}') {
            body_start - '\u{9d}'.len_utf8()
        } else {
            continue;
        };
        let mut pos = semi + 4;
        let mut wanted = 3;
        if bytes.get(pos) == Some(&b'a') {
            pos += 1;
            wanted = 4;
        }
        if bytes.get(pos) != Some(&b':') {
            continue;
        }
        pos += 1;
        let hex_run = |from: usize| {
            bytes[from..]
                .iter()
                .take_while(|byte| byte.is_ascii_hexdigit())
                .count()
        };
        let width = hex_run(pos);
        if !(1..=4).contains(&width) {
            continue;
        }
        pos += width;
        let mut components = 1;
        while components < wanted && bytes.get(pos) == Some(&b'/') && hex_run(pos + 1) >= width {
            pos += 1 + width;
            components += 1;
        }
        if components < wanted {
            continue;
        }
        if bytes.get(pos) == Some(&0x07) {
            pos += 1;
        } else if input[pos..].starts_with("\x1b\\") {
            pos += 2;
        } else if input[pos..].starts_with('\u{9c}') {
            pos += '\u{9c}'.len_utf8();
        } else {
            continue;
        }
        ranges.push((start, pos));
        search = pos;
    }
    if ranges.is_empty() {
        return None;
    }
    let mut cleaned = String::with_capacity(input.len());
    let mut new_cursor = cursor;
    let mut last = 0usize;
    for (start, end) in ranges {
        cleaned.push_str(&input[last..start]);
        if cursor >= end {
            new_cursor -= end - start;
        } else if cursor > start {
            new_cursor -= cursor - start;
        }
        last = end;
    }
    cleaned.push_str(&input[last..]);
    Some((cleaned, new_cursor))
}
