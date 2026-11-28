use super::*;
use ratatui::prelude::{Alignment, Color, Style};

fn transcript_rows() -> Vec<Line<'static>> {
    (0..240)
        .map(|i| {
            let text = format!(
                "row {i}: measured viewport layout keeps café 界面 👩‍💻 and copied transcript columns intact. "
            );
            match i % 4 {
                0 => Line::from(vec![
                    Span::styled("│ ", Style::default().fg(Color::Green)),
                    Span::raw(text),
                ]),
                1 => Line::from(text).alignment(Alignment::Center),
                2 => Line::from(text).alignment(Alignment::Right),
                _ => Line::from(text),
            }
        })
        .collect()
}

fn assert_same_split(actual: &BandSplit, fresh: &BandSplit) {
    assert_eq!(actual.lines, fresh.lines);
    assert_eq!(actual.wide_fit, fresh.wide_fit);
    assert_eq!(actual.band_rows, fresh.band_rows);
    assert_eq!(actual.straddled, fresh.straddled);
    assert_eq!(actual.real_rows, fresh.real_rows);
    assert_eq!(actual.bindings.len(), fresh.bindings.len());
    for (a, b) in actual.bindings.iter().zip(&fresh.bindings) {
        assert_eq!(
            (a.wide_line, a.shift, a.floor, a.ceil),
            (b.wide_line, b.shift, b.floor, b.ceil)
        );
        for col in 0..150 {
            assert_eq!(a.wide_column(col), b.wide_column(col));
        }
    }
}

#[test]
fn repeated_geometry_reuses_fixpoint_fit_and_split_wraps() {
    let rows = transcript_rows();
    let materialize = |start: usize, end: usize| rows[start..end.min(rows.len())].to_vec();
    let cache = BandWrapCache::default();
    let fresh = BandWrapCache {
        uncached: true,
        ..Default::default()
    };
    let band = PinnedBand {
        height: 12,
        reserve: 38,
    };
    let mut scroll = rows.len() - 28;
    for _ in 0..4 {
        let expected =
            bottom_anchor_max_scroll(rows.len(), scroll, 28, band, 2, 100, &materialize, &fresh);
        scroll =
            bottom_anchor_max_scroll(rows.len(), scroll, 28, band, 2, 100, &materialize, &cache);
        assert_eq!(scroll, expected);
    }
    let fits = wide_rows_that_fit(scroll, rows.len(), 28, band, 2, 100, &materialize, &cache);
    assert_eq!(
        fits,
        wide_rows_that_fit(scroll, rows.len(), 28, band, 2, 100, &materialize, &fresh)
    );
    let misses_before_split = cache.misses.get();
    let actual = split_visible_rows(
        materialize(scroll, scroll + fits),
        scroll,
        28,
        band,
        2,
        100,
        &|_| 2,
        &cache,
    );
    let expected = split_visible_rows(
        materialize(scroll, scroll + fits),
        scroll,
        28,
        band,
        2,
        100,
        &|_| 2,
        &fresh,
    );
    assert_same_split(&actual, &expected);
    assert_eq!(cache.misses.get(), misses_before_split);
    assert_eq!(cache.misses.get(), cache.rows.borrow().len());
    assert!(fresh.misses.get() > cache.misses.get());
    println!(
        "band wrap misses: uncached={} cached={} final_split_extra={}",
        fresh.misses.get(),
        cache.misses.get(),
        cache.misses.get() - misses_before_split
    );
}

#[test]
fn cached_output_and_copy_bindings_match_fresh_across_geometry() {
    let rows = transcript_rows();
    let materialize = |start: usize, end: usize| rows[start..end.min(rows.len())].to_vec();
    let cache = BandWrapCache::default();
    let fresh = BandWrapCache {
        uncached: true,
        ..Default::default()
    };
    for width in [40, 62, 100, 150] {
        for height in [0, 1, 3, 12, 28] {
            for scroll in [0, 17, 220, 239] {
                let band = PinnedBand {
                    height,
                    reserve: 28,
                };
                let fits = wide_rows_that_fit(
                    scroll,
                    rows.len(),
                    28,
                    band,
                    0,
                    width,
                    &materialize,
                    &cache,
                );
                assert_eq!(
                    fits,
                    wide_rows_that_fit(
                        scroll,
                        rows.len(),
                        28,
                        band,
                        0,
                        width,
                        &materialize,
                        &fresh
                    )
                );
                let actual = split_visible_rows(
                    materialize(scroll, scroll + fits),
                    scroll,
                    28,
                    band,
                    0,
                    width,
                    &|_| 2,
                    &cache,
                );
                let expected = split_visible_rows(
                    materialize(scroll, scroll + fits),
                    scroll,
                    28,
                    band,
                    0,
                    width,
                    &|_| 2,
                    &fresh,
                );
                assert_same_split(&actual, &expected);
            }
        }
    }
}

#[test]
fn width_and_new_draw_do_not_reuse_stale_wraps() {
    let line = Line::from("café 界面 transcript words ".repeat(5));
    let cache = BandWrapCache::default();
    let first = cache.wrapped(7, &line, 30);
    assert!(Rc::ptr_eq(&first, &cache.wrapped(7, &line, 30)));
    let narrower = cache.wrapped(7, &line, 15);
    assert!(!Rc::ptr_eq(&first, &narrower));
    assert_eq!(cache.misses.get(), 2);
    let next_draw = BandWrapCache::default();
    let changed = Line::from("new frame content ".repeat(7));
    let next = next_draw.wrapped(7, &changed, 30);
    assert!(!Rc::ptr_eq(&first, &next));
    let expected = wrap_line_columns(&changed, 30);
    assert_eq!(next.prefix, expected.prefix);
    for (actual, expected) in next.fragments.iter().zip(&expected.fragments) {
        assert_eq!(actual.line, expected.line);
        assert_eq!((actual.start, actual.end), (expected.start, expected.end));
    }
    assert_eq!(next.fragments.len(), expected.fragments.len());
    assert_eq!(next_draw.misses.get(), 1);
}

#[test]
fn source_equality_guard_preserves_decorations_prefixes_and_blanking() {
    let mut rows = transcript_rows();
    let cache = BandWrapCache::default();
    let preserved = cache.wrapped(8, &rows[8], 62);
    let mut old = Vec::new();
    for (index, row) in rows.iter().take(5).enumerate() {
        old.push(cache.wrapped(index, row, 62));
    }
    rows[0].spans[0].style = Style::default().fg(Color::Red);
    rows[1].spans.insert(0, Span::raw("→ edit#4 "));
    rows[2] = crate::tui::ui::selection_highlight::highlight_line_selection(&rows[2], 5, 20);
    rows[3] = Line::default();
    rows[4].alignment = Some(Alignment::Right);
    for (index, row) in rows.iter().take(5).enumerate() {
        let replaced = cache.wrapped(index, row, 62);
        assert!(!Rc::ptr_eq(&old[index], &replaced));
        assert!(Rc::ptr_eq(&replaced, &cache.wrapped(index, row, 62)));
        let expected = wrap_line_columns(row, 62);
        assert_eq!(replaced.prefix, expected.prefix);
        assert_eq!(replaced.fragments.len(), expected.fragments.len());
        for (a, b) in replaced.fragments.iter().zip(&expected.fragments) {
            assert_eq!(a.line, b.line);
            assert_eq!((a.start, a.end), (b.start, b.end));
        }
    }
    assert_eq!(cache.misses.get(), 11);
    assert!(Rc::ptr_eq(&preserved, &cache.wrapped(8, &rows[8], 62)));
    let band = PinnedBand {
        height: 12,
        reserve: 38,
    };
    let fresh = BandWrapCache {
        uncached: true,
        ..Default::default()
    };
    let actual = split_visible_rows(rows[..20].to_vec(), 0, 20, band, 0, 100, &|_| 2, &cache);
    let expected = split_visible_rows(rows[..20].to_vec(), 0, 20, band, 0, 100, &|_| 2, &fresh);
    assert_same_split(&actual, &expected);
}
