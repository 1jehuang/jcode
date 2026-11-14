use std::hash::{DefaultHasher, Hash, Hasher};
use std::time::{Duration, Instant};

use crate::tui::app::App;

/// Burst window for seam-anchored bare tails (`00\`, `/8787\`) and `Alt+\` ST.
const OSC_SCRUB_BURST_WINDOW: Duration = Duration::from_millis(500);

#[derive(Debug, Default)]
pub(in crate::tui::app) struct OscScrubState {
    pub(super) awaiting_st_backslash: bool,
    pub(super) scrubbed_at: Option<Instant>,
    scrubbed_pos: Option<usize>,
    context_hash: u64,
}

impl OscScrubState {
    fn active_seam(&mut self, input: &str, cursor: usize) -> Option<usize> {
        let in_burst = self
            .scrubbed_at
            .is_some_and(|t| t.elapsed() < OSC_SCRUB_BURST_WINDOW);
        let valid = |p| p <= cursor && context_hash(input, p, cursor) == Some(self.context_hash);
        if !in_burst || !self.scrubbed_pos.is_some_and(valid) {
            *self = Self::default();
        }
        self.scrubbed_pos
    }
}

/// Swallow a lone `\` arriving right after an unterminated OSC reply (`Alt+\`).
pub(super) fn should_swallow_osc_st_backslash(app: &mut App, text: &str) -> bool {
    let awaiting = std::mem::replace(&mut app.osc_scrub.awaiting_st_backslash, false);
    let cursor = app.cursor_pos.min(app.input.len());
    let swallow =
        awaiting && text == "\\" && app.osc_scrub.active_seam(&app.input, cursor) == Some(cursor);
    if swallow {
        app.osc_scrub.scrubbed_at = Some(Instant::now());
    }
    swallow
}

/// Remove late or torn OSC 4 / 10..=19 color replies delivered as keystrokes.
pub(super) fn scrub_osc_color_replies(app: &mut App, raw_terminated: bool) {
    let cursor = app.cursor_pos.min(app.input.len());
    let Some(prefix) = app.input.get(..cursor) else {
        return;
    };
    let seam_in = app.osc_scrub.active_seam(&app.input, cursor);
    if !prefix.contains("gb") && !(seam_in.is_some() && prefix.contains('\\')) {
        return;
    }
    let Some((mut cleaned, new_cursor, seam, awaiting_st)) = strip_inner(prefix, cursor, seam_in)
    else {
        return;
    };
    cleaned.push_str(&app.input[cursor..]);
    app.input = cleaned;
    app.cursor_pos = new_cursor;
    if let Some(context_hash) = context_hash(&app.input, seam, new_cursor) {
        app.osc_scrub = OscScrubState {
            awaiting_st_backslash: awaiting_st && !raw_terminated,
            scrubbed_at: Some(Instant::now()),
            scrubbed_pos: Some(seam),
            context_hash,
        };
    }
}

#[cfg(test)]
type Stripped = (String, usize);

#[cfg(test)]
pub(super) fn strip_osc_color_replies(s: &str, c: usize, seam: Option<usize>) -> Option<Stripped> {
    strip_inner(s, c, seam).map(|(text, pos, _, _)| (text, pos))
}

type StripResult = (String, usize, usize, bool);

fn strip_inner(input: &str, cursor: usize, burst_seam: Option<usize>) -> Option<StripResult> {
    let bytes = input.as_bytes();
    let floor_at = |rs: &[(usize, usize)], p| {
        let seed = burst_seam.map_or(0, |s| usize::from(s <= p) * s);
        rs.iter()
            .fold(seed, |m, r| m.max(usize::from(r.1 <= p) * r.1))
    };
    let follows = |rs: &[(usize, usize)], p| burst_seam == Some(p) || rs.iter().any(|r| r.1 == p);
    let overlaps = |rs: &[(usize, usize)], s, e| rs.iter().any(|r| s < r.1 && e > r.0);
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut search = 0usize;
    while let Some(found) = input[search..].find("gb") {
        let gb_pos = search + found;
        search = gb_pos + 1;
        let is_rgba = bytes.get(gb_pos + 2) == Some(&b'a');
        let mut pos = gb_pos + 2 + usize::from(is_rgba);
        let wanted = 3 + usize::from(is_rgba);
        if bytes.get(pos) != Some(&b':') {
            continue;
        }
        pos += 1;
        let Some((start, has_osc_num)) =
            parse_osc_header_before_gb(input, floor_at(&ranges, gb_pos), gb_pos)
        else {
            continue;
        };
        let width = hex_len(&bytes[pos..]);
        if !(1..=4).contains(&width) {
            continue;
        }
        let color_start = pos;
        pos += width;
        let mut components = 1;
        while components < wanted
            && bytes.get(pos) == Some(&b'/')
            && hex_len(&bytes[pos + 1..]) >= width
        {
            pos += 1 + width;
            components += 1;
        }
        if components < wanted {
            continue;
        }
        let is_plain_rgb = start == gb_pos.saturating_sub(1) && bytes.get(start) == Some(&b'r');
        let repeated = width == 4
            && bytes[color_start..pos]
                .split(|&b| b == b'/')
                .all(repeated_pair);
        if !has_osc_num && bytes.get(pos) != Some(&b'\\') && (is_plain_rgb || !repeated) {
            continue;
        }
        pos += usize::from(bytes.get(pos) == Some(&b'\\'));
        ranges.push((start, pos));
        search = pos;
    }

    // Remove bare `gb:\` remnants only when explicitly `\`-terminated.
    let mut header_search = 0usize;
    while let Some(found) = input[header_search..].find("gb:\\") {
        let gb_pos = header_search + found;
        let end = gb_pos + 4;
        header_search = end;
        let floor = floor_at(&ranges, gb_pos);
        let start = gb_pos - usize::from(gb_pos > floor && bytes[gb_pos - 1] == b']');
        if overlaps(&ranges, start, end)
            || (end < bytes.len() && !bytes[end].is_ascii_whitespace())
            || (!follows(&ranges, start) && has_word_char_before(input, floor, start))
        {
            continue;
        }
        ranges.push((start, end));
    }

    // Scrub duplicated-byte or 2-hex ST tails anchored to a header, range, or seam.
    for start in 0..bytes.len() {
        let Some(len) = osc_tail_len(&bytes[start..]) else {
            continue;
        };
        let end = start + len;
        let floor = floor_at(&ranges, start);
        if (end != bytes.len()
            && bytes.get(end) != Some(&b'/')
            && osc_tail_len(&bytes[end..]).is_none())
            || (!follows(&ranges, start) && blocks_tail(input, floor, start, bytes[start] == b'/'))
        {
            continue;
        }
        let Some(actual_start) = rewind_torn_osc_header(input, floor, start) else {
            continue;
        };
        let anchored = actual_start < start || ranges.iter().any(|&(_, re)| re == start);
        if !(anchored || (burst_seam == Some(start) && len != 5))
            || overlaps(&ranges, actual_start, end)
        {
            continue;
        }
        ranges.push((actual_start, end));
    }
    if ranges.is_empty() {
        return None;
    }

    ranges.sort_unstable_by_key(|&(start, _)| start);
    let clamped = cursor.min(input.len());
    let mut cleaned = String::with_capacity(input.len());
    let mut new_cursor = clamped;
    let mut last = 0usize;
    for (start, end) in ranges {
        if start > last {
            cleaned.push_str(&input[last..start]);
        }
        let eff = start.max(last);
        if end > eff {
            new_cursor = new_cursor.saturating_sub(clamped.min(end).saturating_sub(eff));
            last = end;
        }
    }
    let seam = cleaned.len();
    cleaned.push_str(&input[last..]);
    let awaiting_st = last == bytes.len() && last > 0 && bytes[last - 1] != b'\\';
    Some((cleaned, new_cursor, seam, awaiting_st))
}

fn parse_osc_header_before_gb(input: &str, floor: usize, gb_pos: usize) -> Option<(usize, bool)> {
    let bytes = input.as_bytes();
    let trim_bracket = |s: usize| s - usize::from(s > floor && bytes[s - 1] == b']');
    let plain =
        |p: usize| (!has_word_char_before(input, floor, p)).then(|| (trim_bracket(p), false));
    if gb_pos == floor || bytes[gb_pos - 1] != b'r' {
        return plain(gb_pos);
    }
    if gb_pos - 1 == floor || bytes[gb_pos - 2] != b';' {
        return plain(gb_pos - 1);
    }
    let semi_pos = gb_pos - 2;
    let rev_digits = bytes[floor..semi_pos].iter().rev();
    let digit_len = rev_digits.take_while(|b| b.is_ascii_digit()).count();
    if !(1..=3).contains(&digit_len) {
        return plain(semi_pos);
    }
    let num_start = semi_pos - digit_len;
    let (candidate, has_osc_num) =
        if num_start >= floor + 2 && &bytes[num_start - 2..num_start] == b"4;" {
            (num_start - 2, true)
        } else if num_start > floor && bytes[num_start - 1] == b';' {
            (num_start - 1, true)
        } else {
            (num_start, digit_len == 2 && bytes[num_start] == b'1')
        };
    let follows_reply = floor > 0 && candidate == floor;
    if !follows_reply
        && ((candidate > 0 && bytes[candidate - 1].is_ascii_digit())
            || (!has_osc_num && has_word_char_before(input, 0, candidate)))
    {
        return None;
    }
    Some((trim_bracket(candidate), has_osc_num))
}

fn rewind_torn_osc_header(input: &str, floor: usize, mut pos: usize) -> Option<usize> {
    while pos >= floor + 3 && &input.as_bytes()[pos - 3..pos] == b"gb:" {
        pos = parse_osc_header_before_gb(input, floor, pos - 3)?.0;
    }
    Some(pos)
}

fn context_hash(input: &str, seam: usize, cursor: usize) -> Option<u64> {
    let mut h = DefaultHasher::new();
    (input.get(..seam)?, input.get(cursor..)?).hash(&mut h);
    Some(h.finish())
}

fn hex_len(bytes: &[u8]) -> usize {
    bytes.iter().take_while(|b| b.is_ascii_hexdigit()).count()
}

fn char_before(input: &str, floor: usize, pos: usize) -> Option<char> {
    input.get(floor..pos)?.chars().next_back()
}

fn has_word_char_before(input: &str, floor: usize, pos: usize) -> bool {
    char_before(input, floor, pos).is_some_and(|c| c.is_alphanumeric() || matches!(c, '_' | '-'))
}

fn blocks_tail(input: &str, floor: usize, pos: usize, slash_tail: bool) -> bool {
    char_before(input, floor, pos).is_some_and(|c| {
        c.is_alphanumeric()
            || matches!(c, '_' | '-' | '\\')
            || matches!((slash_tail, c), (true, '.' | '~') | (false, '/'))
    })
}

fn repeated_pair(component: &[u8]) -> bool {
    component.len() == 4 && hex_len(component) == 4 && component[..2] == component[2..]
}

fn osc_tail_len(tail: &[u8]) -> Option<usize> {
    if tail.len() >= 3 && hex_len(&tail[..2]) == 2 && tail[2] == b'\\' {
        return Some(3);
    }
    if tail.starts_with(b"/") && tail.get(1..5).is_some_and(repeated_pair) {
        return (tail.get(5) == Some(&b'\\'))
            .then_some(6)
            .or((tail.len() == 5).then_some(5));
    }
    for w in [4, 2] {
        let end = w + 11;
        if tail.len() >= end
            && hex_len(&tail[..w]) == w
            && (w != 4 || repeated_pair(&tail[..4]))
            && osc_tail_len(&tail[w..w + 5]) == Some(5)
            && osc_tail_len(&tail[w + 5..end]) == Some(6)
        {
            return Some(end);
        }
    }
    None
}
