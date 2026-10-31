//! Look-ahead margin profiles for widget docking.

/// Per-index minimum over `[i-window, i+window]`. Returns an empty vec for empty
/// input (callers treat empty reliable profiles as "no look-ahead").
pub(crate) fn windowed_min(widths: &[u16], window: usize) -> Vec<u16> {
    if widths.is_empty() {
        return Vec::new();
    }
    let n = widths.len();
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let lo = i.saturating_sub(window);
        let hi = (i + window).min(n - 1);
        out.push(widths[lo..=hi].iter().copied().min().unwrap_or(0));
    }
    out
}

/// Look-ahead window (in rows) used to compute the "reliable" margin profile that
/// gates where new info widgets may dock. Small by design: it only needs to cover
/// the distance content travels in the few frames between a widget being placed and
/// a nearby long line scrolling into its rows.
pub(crate) const INFO_WIDGET_LOOKAHEAD_ROWS: usize = 2;
