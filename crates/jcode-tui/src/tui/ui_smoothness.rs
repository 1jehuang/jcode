//! Global anchor-stability (screen smoothness) recorder for the TUI.
//!
//! Feeds one [`AnchorFrame`] per rendered frame from the messages area of the
//! terminal buffer into an [`AnchorStabilityRecorder`], which classifies
//! jarring motion: content repositioning away from its transcript anchor,
//! insertions that push content down, large single-frame pops, blinks, and
//! mass reflows. Expected motion (user scrolls, resizes, uniform tail-follow
//! scrolling) is excluded by the recorder itself.
//!
//! Always on: the per-frame cost is one row-hash pass over the visible
//! messages area (tens of microseconds). Reports are served via the client
//! debug commands `smoothness` and `smoothness:reset`, so live sessions and
//! offscreen replays (`jcode replay`) can both be benchmarked.

use jcode_tui_core::anchor_stability::{AnchorFrame, AnchorStabilityRecorder, BLANK_ROW_HASH};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use std::sync::{Mutex, OnceLock};

fn recorder() -> &'static Mutex<AnchorStabilityRecorder> {
    static RECORDER: OnceLock<Mutex<AnchorStabilityRecorder>> = OnceLock::new();
    RECORDER.get_or_init(|| Mutex::new(AnchorStabilityRecorder::new()))
}

/// Hash one buffer row's cell symbols within `area`. Returns
/// [`BLANK_ROW_HASH`] for visually blank rows.
fn hash_row(buffer: &Buffer, area: Rect, y: u16) -> u64 {
    // FNV-1a over symbol bytes: this runs over every visible cell each frame,
    // and SipHash via `DefaultHasher` was a measurable share of release CPU.
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut blank = true;
    let buf_area = *buffer.area();
    if y < buf_area.top() || y >= buf_area.bottom() {
        return BLANK_ROW_HASH;
    }
    let left = area.left().max(buf_area.left());
    let right = area.right().min(buf_area.right());
    if left >= right {
        return BLANK_ROW_HASH;
    }
    let start = buffer.index_of(left, y);
    for cell in &buffer.content[start..start + usize::from(right - left)] {
        let symbol = cell.symbol();
        if blank && !symbol.trim().is_empty() {
            blank = false;
        }
        for &byte in symbol.as_bytes() {
            hash = (hash ^ u64::from(byte)).wrapping_mul(PRIME);
        }
        // Cell separator so "ab","c" differs from "a","bc".
        hash = (hash ^ 0xff).wrapping_mul(PRIME);
    }
    if blank { BLANK_ROW_HASH } else { hash }
}

/// Build an [`AnchorFrame`] from a rendered buffer region. Shared by the live
/// recorder hook and offscreen benchmark tests (which feed frames into their
/// own local [`AnchorStabilityRecorder`] for deterministic assertions).
pub(crate) fn frame_from_buffer(
    buffer: &Buffer,
    messages_area: Rect,
    scroll_offset: usize,
    following_tail: bool,
) -> Option<AnchorFrame> {
    if messages_area.width == 0 || messages_area.height == 0 {
        return None;
    }
    let area = messages_area.intersection(*buffer.area());
    if area.width == 0 || area.height == 0 {
        return None;
    }
    let rows: Vec<u64> = (area.top()..area.bottom())
        .map(|y| hash_row(buffer, area, y))
        .collect();
    Some(AnchorFrame {
        rows,
        width: area.width,
        scroll_offset,
        following_tail,
        at: std::time::Instant::now(),
    })
}

/// Observe the rendered messages area for this frame. Called once per draw.
pub(super) fn observe_frame(
    buffer: &Buffer,
    messages_area: Rect,
    scroll_offset: usize,
    following_tail: bool,
) {
    let Some(frame) = frame_from_buffer(buffer, messages_area, scroll_offset, following_tail)
    else {
        return;
    };
    let mut rec = recorder()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    rec.observe(frame);
}

/// Pretty JSON anchor-stability report for the debug socket.
pub(crate) fn report_json() -> String {
    let rec = recorder()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    serde_json::to_string_pretty(&rec.report()).unwrap_or_else(|_| "{}".to_string())
}

/// Reset the recorder (e.g. to measure one turn or one replay in isolation).
pub(crate) fn reset() {
    let mut rec = recorder()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *rec = AnchorStabilityRecorder::new();
}
